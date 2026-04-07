/// Taffy-based CSS layout engine for Vulturine.
///
/// Converts a DOM tree into absolutely-positioned boxes.  Each box carries
/// a pre-computed (x, y, w, h) so the rasteriser just paints at the given
/// coordinates instead of tracking a running cursor.
///
/// Improvements over the old vertical-stack approach:
///   • Width is inherited from parent containers (no more hardcoded max_w)
///   • Text wraps at the actual available width computed by Taffy
///   • display:flex rows / columns work correctly
///   • Padding and margin from inline `style` attributes are respected
///   • background-color / color / font-size from `style` attributes cascade

use std::collections::HashMap;
use taffy::prelude::*;
use taffy::TaffyError;

use crate::dom::{Document, NodeData};

// ── Public output types ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PositionedBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub kind: BoxContent,
    pub bg_color: Option<[u8; 4]>,
}

#[derive(Debug, Clone)]
pub enum BoxContent {
    Empty,
    Text    { content: String, bold: bool, size: f32, color: [u8; 4] },
    Heading { content: String, level: u8 },
    Link    { text: String, href: String, size: f32 },
    HRule,
    Image   { alt: String },
}

// ── Private leaf context (used by Taffy measure function) ─────────────────────

#[derive(Clone)]
struct LeafData {
    kind:     BoxContent,
    bg_color: Option<[u8; 4]>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Compute positioned boxes for `doc` given a viewport width (physical pixels).
/// Falls back to a simple vertical stack on any Taffy error.
pub fn compute_layout(doc: &Document, viewport_w: f32) -> Vec<PositionedBox> {
    const PAD: f32 = 12.0;
    let content_w = (viewport_w - PAD * 2.0).max(80.0);
    match try_compute(doc, content_w, PAD) {
        Ok(b)  => b,
        Err(e) => {
            log::debug!("Taffy layout error: {:?}; falling back to simple layout", e);
            simple_fallback(doc, content_w, PAD)
        }
    }
}

// ── Taffy layout ──────────────────────────────────────────────────────────────

fn try_compute(doc: &Document, content_w: f32, pad: f32) -> Result<Vec<PositionedBox>, TaffyError> {
    let mut tree: TaffyTree<LeafData> = TaffyTree::new();

    // bg_color for non-leaf (container) nodes; Taffy only stores context for leaves
    let mut container_bg: HashMap<NodeId, [u8; 4]> = HashMap::new();

    // Find the body (start node)
    let body_dom_id = doc.nodes.iter()
        .find(|n| n.tag_name() == Some("body"))
        .map(|n| n.id)
        .unwrap_or(0);

    // Build the Taffy tree from the DOM
    let root = build_node(&mut tree, &mut container_bg, doc, body_dom_id, false, 14.0)?;

    // Compute layout with fontdue-based text measurement
    tree.compute_layout_with_measure(
        root,
        Size {
            width:  AvailableSpace::Definite(content_w),
            height: AvailableSpace::MaxContent,
        },
        |known_dims, available, _id, ctx, _style| {
            measure_leaf(known_dims, available, ctx)
        },
    )?;

    // Walk the computed tree and emit PositionedBox list
    let mut result = Vec::new();
    collect_boxes(&tree, &container_bg, root, pad, pad, &mut result);
    Ok(result)
}

// ── DOM → Taffy tree ──────────────────────────────────────────────────────────

fn build_node(
    tree:         &mut TaffyTree<LeafData>,
    bg_map:       &mut HashMap<NodeId, [u8; 4]>,
    doc:          &Document,
    dom_id:       usize,
    parent_bold:  bool,
    parent_size:  f32,
) -> Result<NodeId, TaffyError> {
    let node = match doc.nodes.get(dom_id) {
        Some(n) => n,
        None    => return tree.new_leaf(Style::default()),
    };

    match &node.data {
        // ── Text node ────────────────────────────────────────────────────────
        NodeData::Text { content } => {
            let text = collapse_ws(content);
            if text.trim().is_empty() {
                return tree.new_leaf(Style::default());
            }
            let color = if parent_bold { [0, 90, 180, 255] } else { [30, 30, 40, 255] };
            tree.new_leaf_with_context(
                Style { size: Size { width: Dimension::Auto, height: Dimension::Auto }, ..Default::default() },
                LeafData {
                    kind:     BoxContent::Text { content: text, bold: parent_bold, size: parent_size, color },
                    bg_color: None,
                },
            )
        }

        // ── Element node ─────────────────────────────────────────────────────
        NodeData::Element { tag_name, attrs, .. } => {
            let tag = tag_name.as_str();

            // Invisible elements
            if matches!(tag, "script" | "style" | "head" | "meta" | "link"
                           | "noscript" | "template" | "svg" | "canvas") {
                return tree.new_leaf(Style::default());
            }

            // Parse inline style
            let css = attrs.iter()
                .find(|a| a.name == "style")
                .map(|a| parse_inline(a.value.as_str()))
                .unwrap_or_default();

            let font_size  = css_font_size(&css, parent_size, tag);
            let bold       = parent_bold || matches!(tag, "b" | "strong" | "th");
            let _text_color = css_color(&css, if bold || tag == "a" { [0, 90, 180, 255] } else { [30, 30, 40, 255] });

            // ── <h1>–<h6> ────────────────────────────────────────────────────
            if let Some(level) = heading_level(tag) {
                let content = collect_text(doc, &node.children).trim().to_string();
                if content.is_empty() { return tree.new_leaf(Style::default()); }
                let mt = if level == 1 { 16.0_f32 } else { 10.0 };
                return tree.new_leaf_with_context(
                    Style {
                        size:   Size { width: Dimension::Auto, height: Dimension::Auto },
                        margin: Rect {
                            top:    LengthPercentageAuto::Length(mt),
                            bottom: LengthPercentageAuto::Length(4.0),
                            left:   LengthPercentageAuto::Length(0.0),
                            right:  LengthPercentageAuto::Length(0.0),
                        },
                        ..Default::default()
                    },
                    LeafData { kind: BoxContent::Heading { content, level }, bg_color: None },
                );
            }

            // ── <a> ──────────────────────────────────────────────────────────
            if tag == "a" {
                let href = attrs.iter().find(|a| a.name == "href")
                    .map(|a| a.value.clone()).unwrap_or_default();
                if !href.is_empty() && !href.starts_with("javascript") {
                    let text = collect_text(doc, &node.children).trim().to_string();
                    if !text.is_empty() {
                        return tree.new_leaf_with_context(
                            Style { size: Size { width: Dimension::Auto, height: Dimension::Auto }, ..Default::default() },
                            LeafData { kind: BoxContent::Link { text, href, size: font_size }, bg_color: None },
                        );
                    }
                }
                // <a> with no useful text → fall through to container
            }

            // ── <img> ────────────────────────────────────────────────────────
            if tag == "img" {
                let alt = attrs.iter().find(|a| a.name == "alt")
                    .map(|a| a.value.trim().to_string()).unwrap_or_default();
                return tree.new_leaf_with_context(
                    Style {
                        size:   Size { width: Dimension::Auto, height: Dimension::Length(44.0) },
                        margin: Rect {
                            top:    LengthPercentageAuto::Length(2.0),
                            bottom: LengthPercentageAuto::Length(0.0),
                            left:   LengthPercentageAuto::Length(0.0),
                            right:  LengthPercentageAuto::Length(0.0),
                        },
                        ..Default::default()
                    },
                    LeafData { kind: BoxContent::Image { alt }, bg_color: None },
                );
            }

            // ── <hr> ─────────────────────────────────────────────────────────
            if tag == "hr" {
                return tree.new_leaf_with_context(
                    Style {
                        size:   Size { width: Dimension::Auto, height: Dimension::Length(10.0) },
                        margin: Rect {
                            top:    LengthPercentageAuto::Length(6.0),
                            bottom: LengthPercentageAuto::Length(6.0),
                            left:   LengthPercentageAuto::Length(0.0),
                            right:  LengthPercentageAuto::Length(0.0),
                        },
                        ..Default::default()
                    },
                    LeafData { kind: BoxContent::HRule, bg_color: None },
                );
            }

            // ── <br> ─────────────────────────────────────────────────────────
            if tag == "br" {
                return tree.new_leaf(Style {
                    size: Size { width: Dimension::Auto, height: Dimension::Length(14.0) },
                    ..Default::default()
                });
            }

            // ── Container element ─────────────────────────────────────────────
            let display = css.get("display").map(|s| s.as_str())
                .unwrap_or_else(|| default_display(tag));
            if display == "none" {
                return tree.new_leaf(Style::default());
            }

            let taffy_display = if display == "flex" { Display::Flex } else { Display::Block };
            let flex_dir = if css.get("flex-direction").map(|s| s.as_str()) == Some("row") {
                FlexDirection::Row
            } else {
                FlexDirection::Column
            };

            let bg = css_bg(&css, tag);
            let padding = css_padding(&css, tag);
            let margin  = css_margin(&css, tag);

            // Recurse children
            let mut children = Vec::new();
            for &child_id in &node.children {
                let child = build_node(tree, bg_map, doc, child_id, bold, font_size)?;
                children.push(child);
            }

            // Empty container
            if children.is_empty() {
                let id = tree.new_leaf(Style { display: taffy_display, flex_direction: flex_dir, padding, margin, ..Default::default() })?;
                if let Some(c) = bg { bg_map.insert(id, c); }
                return Ok(id);
            }

            let id = tree.new_with_children(
                Style { display: taffy_display, flex_direction: flex_dir, padding, margin, ..Default::default() },
                &children,
            )?;
            if let Some(c) = bg { bg_map.insert(id, c); }
            Ok(id)
        }

        // ── Other (doctype, comments, etc.) ──────────────────────────────────
        _ => {
            let mut children = Vec::new();
            for &child_id in &node.children {
                let child = build_node(tree, bg_map, doc, child_id, parent_bold, parent_size)?;
                children.push(child);
            }
            if children.is_empty() {
                return tree.new_leaf(Style::default());
            }
            tree.new_with_children(Style::default(), &children)
        }
    }
}

// ── Taffy measure function ────────────────────────────────────────────────────

fn measure_leaf(
    known:     Size<Option<f32>>,
    available: Size<AvailableSpace>,
    ctx:       Option<&mut LeafData>,
) -> Size<f32> {
    let data = match ctx {
        Some(d) => d,
        None    => return Size::ZERO,
    };

    let avail_w = match available.width {
        AvailableSpace::Definite(w) => w,
        AvailableSpace::MaxContent  => f32::INFINITY,
        AvailableSpace::MinContent  => 0.0,
    };

    match &data.kind {
        BoxContent::Text { content, size, .. }
        | BoxContent::Link { text: content, size, .. } => {
            compute_text_size(content, *size, avail_w, known)
        }
        BoxContent::Heading { content, level } => {
            let size = heading_font_size(*level);
            compute_text_size(content, size, avail_w, known)
        }
        BoxContent::Image { .. } => {
            let w = known.width.unwrap_or_else(|| avail_w.min(600.0));
            Size { width: w, height: known.height.unwrap_or(44.0) }
        }
        BoxContent::HRule => Size { width: avail_w, height: 10.0 },
        BoxContent::Empty => Size::ZERO,
    }
}

fn compute_text_size(text: &str, size: f32, avail_w: f32, known: Size<Option<f32>>) -> Size<f32> {
    use super::renderer::{font_stack, glyph_advance, word_width};

    let fonts = &font_stack().regular;
    if fonts.is_empty() || text.trim().is_empty() {
        let h = size + 4.0;
        return Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(h) };
    }

    let line_h   = size + 4.0;
    let space_w  = glyph_advance(fonts, ' ', size);

    if avail_w <= 0.0 || avail_w.is_infinite() {
        // MaxContent mode: report natural width (no wrapping)
        let nat_w: f32 = text.split_whitespace()
            .map(|w| word_width(fonts, w, size))
            .sum::<f32>()
            + space_w * (text.split_whitespace().count().saturating_sub(1)) as f32;
        return Size { width: known.width.unwrap_or(nat_w), height: known.height.unwrap_or(line_h) };
    }

    // Definite width: word-wrap to count lines
    let mut line_w = 0.0_f32;
    let mut lines  = 1usize;
    for word in text.split_whitespace() {
        let ww = word_width(fonts, word, size);
        if line_w + ww > avail_w && line_w > 0.0 {
            lines  += 1;
            line_w  = ww + space_w;
        } else {
            if line_w > 0.0 { line_w += space_w; }
            line_w += ww;
        }
    }
    let h = lines as f32 * line_h;
    Size {
        width:  known.width.unwrap_or(avail_w),
        height: known.height.unwrap_or(h),
    }
}

// ── Walk Taffy results → PositionedBox list ───────────────────────────────────

fn collect_boxes(
    tree:       &TaffyTree<LeafData>,
    bg_map:     &HashMap<NodeId, [u8; 4]>,
    node:       NodeId,
    parent_x:   f32,
    parent_y:   f32,
    out:        &mut Vec<PositionedBox>,
) {
    let layout = match tree.layout(node) {
        Ok(l)  => l,
        Err(_) => return,
    };
    let abs_x = parent_x + layout.location.x;
    let abs_y = parent_y + layout.location.y;
    let w     = layout.size.width;
    let h     = layout.size.height;

    // Leaf node: emit a box
    if let Some(data) = tree.get_node_context(node) {
        // skip zero-size boxes (invisible nodes)
        if matches!(data.kind, BoxContent::Empty) && bg_map.get(&node).is_none() {
            return;
        }
        out.push(PositionedBox {
            x: abs_x, y: abs_y, w, h,
            kind:     data.kind.clone(),
            bg_color: data.bg_color.or_else(|| bg_map.get(&node).copied()),
        });
        return;
    }

    // Container: emit a background rect if needed, then recurse
    if let Some(&bg) = bg_map.get(&node) {
        out.push(PositionedBox { x: abs_x, y: abs_y, w, h, kind: BoxContent::Empty, bg_color: Some(bg) });
    }

    let children = match tree.children(node) {
        Ok(c)  => c,
        Err(_) => return,
    };
    for child in children {
        collect_boxes(tree, bg_map, child, abs_x, abs_y, out);
    }
}

// ── CSS inline style helpers ──────────────────────────────────────────────────

fn parse_inline(style: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for decl in style.split(';') {
        if let Some((k, v)) = decl.split_once(':') {
            map.insert(k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
        }
    }
    map
}

fn default_display(tag: &str) -> &'static str {
    match tag {
        "span" | "a" | "strong" | "em" | "b" | "i" | "u" | "s"
        | "code" | "label" | "small" | "sup" | "sub" | "abbr" => "inline",
        _ => "block",
    }
}

fn css_font_size(css: &HashMap<String, String>, parent: f32, tag: &str) -> f32 {
    if let Some(v) = css.get("font-size") {
        if let Some(px) = parse_px(v) { return px; }
        if let Some(em) = parse_em(v) { return parent * em; }
        match v.as_str() {
            "small"   => return 12.0,
            "medium"  => return 14.0,
            "large"   => return 18.0,
            "x-large" => return 22.0,
            _         => {}
        }
    }
    // Tag-level defaults
    match tag {
        "small"  => parent * 0.85,
        "sub"|"sup" => parent * 0.75,
        _ => parent,
    }
}

fn css_color(css: &HashMap<String, String>, default: [u8; 4]) -> [u8; 4] {
    css.get("color").and_then(|v| parse_css_color(v)).unwrap_or(default)
}

fn css_bg(css: &HashMap<String, String>, tag: &str) -> Option<[u8; 4]> {
    if let Some(v) = css.get("background-color").or_else(|| css.get("background")) {
        if let Some(c) = parse_css_color(v) { return Some(c); }
    }
    // Tag-level defaults
    match tag {
        "blockquote" => Some([240, 242, 248, 255]),
        "pre" | "code" => Some([245, 245, 250, 255]),
        _ => None,
    }
}

fn css_padding(css: &HashMap<String, String>, tag: &str) -> Rect<LengthPercentage> {
    let tag_pad = match tag {
        "blockquote" | "pre" => LengthPercentage::Length(8.0),
        "li" => LengthPercentage::Length(4.0),
        _ => LengthPercentage::Length(0.0),
    };
    let all = css.get("padding").and_then(|v| parse_px(v))
        .map(LengthPercentage::Length).unwrap_or(tag_pad);
    let top    = css.get("padding-top").and_then(|v| parse_px(v)).map(LengthPercentage::Length).unwrap_or(all);
    let bottom = css.get("padding-bottom").and_then(|v| parse_px(v)).map(LengthPercentage::Length).unwrap_or(all);
    let left   = css.get("padding-left").and_then(|v| parse_px(v)).map(LengthPercentage::Length).unwrap_or(all);
    let right  = css.get("padding-right").and_then(|v| parse_px(v)).map(LengthPercentage::Length).unwrap_or(all);
    Rect { top, bottom, left, right }
}

fn css_margin(css: &HashMap<String, String>, tag: &str) -> Rect<LengthPercentageAuto> {
    let tag_margin = match tag {
        "p" | "div" | "section" | "article" | "header" | "footer"
        | "blockquote" | "pre" => LengthPercentageAuto::Length(6.0),
        "li" => LengthPercentageAuto::Length(2.0),
        _ => LengthPercentageAuto::Length(0.0),
    };
    let all = css.get("margin").and_then(|v| parse_px(v))
        .map(LengthPercentageAuto::Length).unwrap_or(tag_margin);
    let top    = css.get("margin-top").and_then(|v| parse_px(v)).map(LengthPercentageAuto::Length).unwrap_or(all);
    let bottom = css.get("margin-bottom").and_then(|v| parse_px(v)).map(LengthPercentageAuto::Length).unwrap_or(all);
    let left   = css.get("margin-left").and_then(|v| parse_px(v)).map(LengthPercentageAuto::Length).unwrap_or(LengthPercentageAuto::Length(0.0));
    let right  = css.get("margin-right").and_then(|v| parse_px(v)).map(LengthPercentageAuto::Length).unwrap_or(LengthPercentageAuto::Length(0.0));
    Rect { top, bottom, left, right }
}

fn parse_px(v: &str) -> Option<f32> {
    let s = v.trim().trim_end_matches("px").trim();
    s.parse::<f32>().ok()
}

fn parse_em(v: &str) -> Option<f32> {
    let s = v.trim().trim_end_matches("em").trim();
    s.parse::<f32>().ok()
}

fn parse_css_color(s: &str) -> Option<[u8; 4]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let hex = hex.trim();
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some([r, g, b, 255]);
        }
        if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            return Some([r, g, b, 255]);
        }
    }
    // rgb(r,g,b) / rgba(r,g,b,a)
    if let Some(inner) = s.strip_prefix("rgb").and_then(|s| {
        s.trim_start_matches('a').trim().strip_prefix('(').and_then(|s| s.strip_suffix(')'))
    }) {
        let parts: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if parts.len() >= 3 {
            let a = parts.get(3).copied().unwrap_or(1.0);
            return Some([parts[0] as u8, parts[1] as u8, parts[2] as u8, (a * 255.0) as u8]);
        }
    }
    // Named colors (common subset)
    match s {
        "white"   | "#fff" | "#ffffff" => Some([255, 255, 255, 255]),
        "black"   | "#000" | "#000000" => Some([0, 0, 0, 255]),
        "red"                           => Some([220, 50, 50, 255]),
        "green"                         => Some([50, 180, 50, 255]),
        "blue"                          => Some([50, 100, 220, 255]),
        "gray" | "grey"                 => Some([128, 128, 128, 255]),
        "lightgray" | "lightgrey"       => Some([211, 211, 211, 255]),
        "darkgray" | "darkgrey"         => Some([64, 64, 64, 255]),
        "orange"                        => Some([255, 165, 0, 255]),
        "yellow"                        => Some([255, 220, 0, 255]),
        "transparent"                   => Some([0, 0, 0, 0]),
        _ => None,
    }
}

// ── DOM helpers ───────────────────────────────────────────────────────────────

fn heading_level(tag: &str) -> Option<u8> {
    match tag { "h1"=>Some(1),"h2"=>Some(2),"h3"=>Some(3),"h4"=>Some(4),"h5"=>Some(5),"h6"=>Some(6), _=>None }
}

pub(super) fn heading_font_size(level: u8) -> f32 {
    match level { 1=>26.0, 2=>22.0, 3=>18.0, _=>16.0 }
}

fn collect_text(doc: &Document, children: &[usize]) -> String {
    let mut out = String::new();
    for &id in children {
        let Some(n) = doc.nodes.get(id) else { continue };
        match &n.data {
            NodeData::Text { content } => { out.push_str(content); out.push(' '); }
            NodeData::Element { tag_name, .. }
                if !matches!(tag_name.as_str(), "script" | "style") => {
                out.push_str(&collect_text(doc, &n.children));
            }
            _ => {}
        }
    }
    out
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for ch in s.replace(['\n', '\r', '\t'], " ").chars() {
        if ch == ' ' { if !space { out.push(' '); space = true; } }
        else         { out.push(ch); space = false; }
    }
    out
}

// ── Simple fallback (old vertical-stack, used if Taffy fails) ─────────────────

fn simple_fallback(doc: &Document, content_w: f32, pad: f32) -> Vec<PositionedBox> {
    let mut out  = Vec::new();
    let mut y    = pad;

    let body_id = doc.nodes.iter()
        .find(|n| n.tag_name() == Some("body"))
        .map(|n| n.id)
        .unwrap_or(0);

    walk_fallback(doc, body_id, content_w, pad, 0.0, false, 14.0, &mut y, &mut out);
    out
}

fn walk_fallback(
    doc:       &Document,
    dom_id:    usize,
    content_w: f32,
    pad:       f32,
    indent:    f32,
    bold:      bool,
    size:      f32,
    y:         &mut f32,
    out:       &mut Vec<PositionedBox>,
) {
    use super::renderer::{font_stack, glyph_advance, word_width};

    let node = match doc.nodes.get(dom_id) { Some(n) => n, None => return };
    let avail = content_w - indent;

    match &node.data {
        NodeData::Text { content } => {
            let text = collapse_ws(content);
            if text.trim().is_empty() { return; }
            // quick text-height estimate
            let fonts  = &font_stack().regular;
            let line_h = size + 4.0;
            let space_w = if fonts.is_empty() { size * 0.3 } else { glyph_advance(fonts, ' ', size) };
            let mut lw = 0.0_f32;
            let mut lines = 1usize;
            for word in text.split_whitespace() {
                let ww = if fonts.is_empty() { word.len() as f32 * size * 0.6 } else { word_width(fonts, word, size) };
                if lw + ww > avail && lw > 0.0 { lines += 1; lw = ww + space_w; }
                else { if lw > 0.0 { lw += space_w; } lw += ww; }
            }
            let color = if bold { [0, 90, 180, 255] } else { [30, 30, 40, 255] };
            out.push(PositionedBox { x: pad + indent, y: *y, w: avail, h: lines as f32 * line_h,
                kind: BoxContent::Text { content: text, bold, size, color }, bg_color: None });
            *y += lines as f32 * line_h;
        }

        NodeData::Element { tag_name, attrs, .. } => {
            let tag = tag_name.as_str();
            if matches!(tag, "script"|"style"|"head"|"meta"|"link"|"noscript"|"template"|"svg"|"canvas") { return; }

            if let Some(level) = heading_level(tag) {
                let content = collect_text(doc, &node.children).trim().to_string();
                if content.is_empty() { return; }
                *y += if level == 1 { 16.0 } else { 10.0 };
                let h = heading_font_size(level) + 4.0;
                out.push(PositionedBox { x: pad + indent, y: *y, w: avail, h,
                    kind: BoxContent::Heading { content, level }, bg_color: None });
                *y += h + 4.0;
                return;
            }

            if tag == "a" {
                let href = attrs.iter().find(|a| a.name == "href").map(|a| a.value.clone()).unwrap_or_default();
                if !href.is_empty() && !href.starts_with("javascript") {
                    let text = collect_text(doc, &node.children).trim().to_string();
                    if !text.is_empty() {
                        out.push(PositionedBox { x: pad + indent, y: *y, w: avail, h: 18.0,
                            kind: BoxContent::Link { text, href, size: 14.0 }, bg_color: None });
                        *y += 18.0;
                        return;
                    }
                }
            }

            if tag == "img" {
                let alt = attrs.iter().find(|a| a.name == "alt").map(|a| a.value.trim().to_string()).unwrap_or_default();
                out.push(PositionedBox { x: pad + indent, y: *y, w: avail, h: 44.0,
                    kind: BoxContent::Image { alt }, bg_color: None });
                *y += 46.0;
                return;
            }

            if tag == "hr" {
                *y += 6.0;
                out.push(PositionedBox { x: pad + indent, y: *y, w: avail, h: 10.0,
                    kind: BoxContent::HRule, bg_color: None });
                *y += 10.0;
                return;
            }

            let new_indent = if tag == "li" { indent + 16.0 } else { indent };
            if matches!(tag, "p"|"div"|"section"|"article"|"blockquote"|"pre") { *y += 6.0; }

            let css = attrs.iter().find(|a| a.name == "style")
                .map(|a| parse_inline(&a.value)).unwrap_or_default();
            let new_bold = bold || matches!(tag, "b"|"strong");
            let new_size = css_font_size(&css, size, tag);

            for &child in &node.children {
                walk_fallback(doc, child, content_w, pad, new_indent, new_bold, new_size, y, out);
            }
        }

        _ => {
            for &child in &node.children {
                walk_fallback(doc, child, content_w, pad, indent, bold, size, y, out);
            }
        }
    }
}
