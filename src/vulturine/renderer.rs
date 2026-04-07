/// Vulturine Software Renderer
///
/// Pipeline:
///   1. Build an absolutely-positioned box list via the Taffy layout engine
///      (vulturine/layout.rs).  Each box already has its final (x, y, w, h).
///   2. Rasterise boxes at those absolute coordinates using tiny-skia + fontdue.
///   3. Return an `RgbaImage` + link hitboxes.
use fontdue::{Font, FontSettings};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tiny_skia::{Paint, PathBuilder, Pixmap, Rect as SkRect, Stroke, Transform};

use super::layout::{heading_font_size, BoxContent, PositionedBox};
use super::{RenderOutput, RgbaImage};
use crate::dom::Document;

#[derive(Clone)]
pub struct HitBox {
    pub x:   f32,
    pub y:   f32,
    pub w:   f32,
    pub h:   f32,
    pub url: String,
}

// ── Font stack ────────────────────────────────────────────────────────────────

static FONT_BYTES:      &[u8] = include_bytes!("../../assets/fonts/NotoSans-Regular.ttf");
static FONT_BOLD_BYTES: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Bold.ttf");

#[derive(Default)]
pub(crate) struct FontStack {
    pub regular: Vec<Font>,
    pub bold:    Vec<Font>,
}

static FONT_STACK: OnceLock<FontStack> = OnceLock::new();

#[cfg(target_os = "windows")]
const SYSTEM_FALLBACK_REGULAR: &[&str] = &[
    r"C:\Windows\Fonts\seguiemj.ttf",
    r"C:\Windows\Fonts\seguisym.ttf",
    r"C:\Windows\Fonts\segoeui.ttf",
    r"C:\Windows\Fonts\arialuni.ttf",
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\msjh.ttc",
    r"C:\Windows\Fonts\simsun.ttc",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\malgun.ttf",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\msgothic.ttc",
    r"C:\Windows\Fonts\nirmala.ttf",
    r"C:\Windows\Fonts\arial.ttf",
];
#[cfg(target_os = "windows")]
const SYSTEM_FALLBACK_BOLD: &[&str] = &[
    r"C:\Windows\Fonts\segoeuib.ttf",
    r"C:\Windows\Fonts\arialbd.ttf",
];

#[cfg(target_os = "linux")]
const SYSTEM_FALLBACK_REGULAR: &[&str] = &[
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];
#[cfg(target_os = "linux")]
const SYSTEM_FALLBACK_BOLD: &[&str] = &[
    "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
];

#[cfg(target_os = "macos")]
const SYSTEM_FALLBACK_REGULAR: &[&str] = &[
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/NotoSansCJK.ttc",
    "/System/Library/Fonts/Apple Color Emoji.ttc",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
];
#[cfg(target_os = "macos")]
const SYSTEM_FALLBACK_BOLD: &[&str] = &["/System/Library/Fonts/Supplemental/Arial Bold.ttf"];

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
const SYSTEM_FALLBACK_REGULAR: &[&str] = &[];
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
const SYSTEM_FALLBACK_BOLD: &[&str] = &[];

// ── Public entry point ────────────────────────────────────────────────────────

pub fn software_render(doc: &Document, _url: &str, width: u32, height: u32) -> RenderOutput {
    let boxes = super::layout::compute_layout(doc, width as f32);
    let (image, hitboxes) = rasterise(&boxes, width, height);
    RenderOutput { image, width, height, hitboxes }
}

// ── Rasteriser ────────────────────────────────────────────────────────────────

fn rasterise(boxes: &[PositionedBox], width: u32, height: u32) -> (RgbaImage, Vec<HitBox>) {
    let mut hitboxes = Vec::new();
    let mut pixmap   = Pixmap::new(width, height)
        .unwrap_or_else(|| Pixmap::new(800, 600).unwrap());

    pixmap.fill(tiny_skia::Color::from_rgba8(250, 250, 252, 255));

    let stack       = font_stack();
    let reg_fonts   = stack.regular.as_slice();
    let bold_fonts  = if stack.bold.is_empty() { reg_fonts } else { stack.bold.as_slice() };
    let h_limit     = height as f32;

    for pb in boxes {
        if pb.y > h_limit { break; }

        // Background fill (blockquote, pre, explicit bg-color)
        if let Some(bg) = pb.bg_color {
            let mut paint = Paint::default();
            paint.set_color_rgba8(bg[0], bg[1], bg[2], bg[3]);
            if let Some(r) = SkRect::from_xywh(pb.x, pb.y, pb.w.max(1.0), pb.h.max(1.0)) {
                pixmap.fill_rect(r, &paint, Transform::identity(), None);
            }
        }

        match &pb.kind {
            BoxContent::Empty => {}

            BoxContent::HRule => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(200, 200, 210, 255);
                let path = {
                    let mut b = PathBuilder::new();
                    b.move_to(pb.x, pb.y + 4.0);
                    b.line_to(pb.x + pb.w, pb.y + 4.0);
                    b.finish()
                };
                if let Some(p) = path {
                    let mut stroke = Stroke::default(); stroke.width = 1.0;
                    pixmap.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
                }
            }

            BoxContent::Heading { content, level } => {
                let size  = heading_font_size(*level);
                let color = match level { 1 => [15u8,15,30,255], 2 => [20,20,60,255], _ => [30,30,50,255] };
                draw_text_wrapped(&mut pixmap, content, pb.x, pb.y, pb.w, size, color, bold_fonts);
            }

            BoxContent::Link { text, href, size } => {
                let used = draw_text_wrapped(&mut pixmap, text, pb.x, pb.y, pb.w, *size, [0, 90, 180, 255], reg_fonts);

                let tw = word_width(reg_fonts, text, *size).min(pb.w);
                hitboxes.push(HitBox {
                    x:   pb.x,
                    y:   pb.y,
                    w:   tw,
                    h:   used.max(16.0),
                    url: href.clone(),
                });

                // Underline
                let mut paint = Paint::default();
                paint.set_color_rgba8(0, 90, 180, 160);
                let path = {
                    let mut b = PathBuilder::new();
                    b.move_to(pb.x, pb.y + used);
                    b.line_to((pb.x + pb.w).min(pb.x + pb.w), pb.y + used);
                    b.finish()
                };
                if let Some(p) = path {
                    let mut stroke = Stroke::default(); stroke.width = 1.0;
                    pixmap.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
                }
            }

            BoxContent::Image { alt } => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(230, 232, 238, 255);
                if let Some(r) = SkRect::from_xywh(pb.x, pb.y, pb.w.max(1.0), pb.h.max(1.0)) {
                    pixmap.fill_rect(r, &paint, Transform::identity(), None);
                }
                let label = format!("[img: {}]", alt);
                draw_text_wrapped(&mut pixmap, &label, pb.x + 4.0, pb.y + 12.0,
                    pb.w - 8.0, 12.0, [80, 90, 110, 255], reg_fonts);
            }

            BoxContent::Text { content, bold, size, color } => {
                let fonts = if *bold { bold_fonts } else { reg_fonts };
                draw_text_wrapped(&mut pixmap, content, pb.x, pb.y, pb.w, *size, *color, fonts);
            }
        }
    }

    let pixels: Vec<[u8; 4]> = pixmap.data().chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect();
    let img = RgbaImage { size: [pixmap.width() as usize, pixmap.height() as usize], pixels };
    (img, hitboxes)
}

// ── Text rasterisation (pub(crate) so layout.rs can call for measurement) ─────

/// Draw word-wrapped text; return total pixel height used.
pub(crate) fn draw_text_wrapped(
    pixmap: &mut Pixmap,
    text:   &str,
    x: f32, y: f32, max_w: f32, size: f32,
    color: [u8; 4],
    fonts: &[Font],
) -> f32 {
    if fonts.is_empty() { return size + 2.0; }
    if text.trim().is_empty() { return size * 0.5; }

    let line_h   = size + 4.0;
    let mut line_y = y;
    let mut line_buf = String::new();
    let mut line_w   = 0.0_f32;
    let space_w = glyph_advance(fonts, ' ', size);

    for word in text.split_whitespace() {
        let ww = word_width(fonts, word, size);
        if line_w + ww > max_w && !line_buf.is_empty() {
            draw_text_line(pixmap, &line_buf, x, line_y, size, color, fonts);
            line_buf.clear(); line_w = 0.0; line_y += line_h;
            if line_y > pixmap.height() as f32 { break; }
        }
        if !line_buf.is_empty() { line_buf.push(' '); line_w += space_w; }
        line_buf.push_str(word);
        line_w += ww;
    }
    if !line_buf.is_empty() {
        draw_text_line(pixmap, &line_buf, x, line_y, size, color, fonts);
        line_y += line_h;
    }
    line_y - y
}

fn draw_text_line(
    pixmap: &mut Pixmap,
    text: &str, x: f32, y: f32, size: f32,
    color: [u8; 4], fonts: &[Font],
) {
    let mut cx = x;
    let w = pixmap.width()  as i32;
    let h = pixmap.height() as i32;

    for ch in text.chars() {
        let Some(font) = pick_font_for_char(fonts, ch) else { cx += size * 0.5; continue; };
        let (metrics, bitmap) = font.rasterize(ch, size);
        let bx = (cx + metrics.xmin as f32) as i32;
        let by = (y + size - metrics.height as f32 - metrics.ymin as f32) as i32;

        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let px = bx + col as i32;
                let py = by + row as i32;
                if px < 0 || py < 0 || px >= w || py >= h { continue; }
                let alpha = bitmap[row * metrics.width + col];
                if alpha == 0 { continue; }
                let idx  = (py as u32 * pixmap.width() + px as u32) as usize * 4;
                let data = pixmap.data_mut();
                if idx + 3 >= data.len() { continue; }
                let a = alpha as u32;
                data[idx]   = ((color[0] as u32 * a + data[idx]   as u32 * (255 - a)) / 255) as u8;
                data[idx+1] = ((color[1] as u32 * a + data[idx+1] as u32 * (255 - a)) / 255) as u8;
                data[idx+2] = ((color[2] as u32 * a + data[idx+2] as u32 * (255 - a)) / 255) as u8;
                data[idx+3] = 255;
            }
        }
        cx += metrics.advance_width;
    }
}

pub(crate) fn word_width(fonts: &[Font], word: &str, size: f32) -> f32 {
    word.chars().map(|c| glyph_advance(fonts, c, size)).sum()
}

pub(crate) fn glyph_advance(fonts: &[Font], ch: char, size: f32) -> f32 {
    pick_font_for_char(fonts, ch)
        .map(|f| f.rasterize(ch, size).0.advance_width)
        .unwrap_or(size * 0.5)
}

pub(crate) fn pick_font_for_char(fonts: &[Font], ch: char) -> Option<&Font> {
    if fonts.is_empty() { return None; }
    if ch.is_whitespace() { return fonts.first(); }
    fonts.iter().find(|f| f.has_glyph(ch)).or_else(|| fonts.first())
}

pub(crate) fn font_stack() -> &'static FontStack {
    FONT_STACK.get_or_init(build_font_stack)
}

fn build_font_stack() -> FontStack {
    let mut regular      = Vec::new();
    let mut bold         = Vec::new();
    let mut seen_regular = HashSet::new();
    let mut seen_bold    = HashSet::new();

    if let Some(f) = load_font_bytes("NotoSans-Regular", FONT_BYTES) {
        push_unique(&mut regular, &mut seen_regular, f);
    }
    if let Some(f) = load_font_bytes("NotoSans-Bold", FONT_BOLD_BYTES) {
        push_unique(&mut bold, &mut seen_bold, f);
    }

    for extra in list_asset_fonts() {
        if let Some(f) = load_font_path(&extra) { push_unique(&mut regular, &mut seen_regular, f); }
    }
    for p in SYSTEM_FALLBACK_REGULAR {
        if let Some(f) = load_font_path(Path::new(p)) { push_unique(&mut regular, &mut seen_regular, f); }
    }
    load_os_discovered(&mut regular, &mut seen_regular);
    for p in SYSTEM_FALLBACK_BOLD {
        if let Some(f) = load_font_path(Path::new(p)) { push_unique(&mut bold, &mut seen_bold, f); }
    }
    // Use regular fonts as bold fallback
    for f in &regular { push_unique(&mut bold, &mut seen_bold, f.clone()); }
    if regular.is_empty() {
        for f in &bold { push_unique(&mut regular, &mut seen_regular, f.clone()); }
    }

    if regular.is_empty() {
        log::warn!("Vulturine: no fonts available for software renderer");
    } else {
        log::info!("Vulturine: font stack ready (regular={}, bold={})", regular.len(), bold.len());
    }
    FontStack { regular, bold }
}

fn load_font_bytes(label: &str, bytes: &[u8]) -> Option<Font> {
    Font::from_bytes(bytes, FontSettings::default())
        .map_err(|e| log::warn!("Vulturine: bundled font {label} failed: {e}"))
        .ok()
}

fn load_font_path(path: &Path) -> Option<Font> {
    let bytes = fs::read(path).ok()?;
    Font::from_bytes(bytes, FontSettings::default())
        .map_err(|e| log::debug!("Vulturine: skipped {}: {e}", path.display()))
        .ok()
}

fn push_unique(out: &mut Vec<Font>, seen: &mut HashSet<usize>, font: Font) {
    if seen.insert(font.file_hash()) { out.push(font); }
}

#[cfg(target_os = "windows")]
fn load_os_discovered(regular: &mut Vec<Font>, seen: &mut HashSet<usize>) {
    let keywords = ["noto","unicode","symbol","emoji","arabic","hebrew",
                    "devanagari","thai","cjk","han","jp","kr","simsun",
                    "simhei","msyh","msjh","malgun","meiryo","gothic","nirmala"];
    let mut candidates: Vec<PathBuf> = fs::read_dir(r"C:\Windows\Fonts")
        .into_iter().flatten()
        .filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.extension().and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf"|"otf"|"ttc")).unwrap_or(false))
        .filter(|p| p.file_name().and_then(|n| n.to_str())
            .map(|n| { let n = n.to_ascii_lowercase(); keywords.iter().any(|kw| n.contains(kw)) })
            .unwrap_or(false))
        .collect();
    candidates.sort();
    for p in candidates.into_iter().take(32) {
        if let Some(f) = load_font_path(&p) { push_unique(regular, seen, f); }
    }
}

#[cfg(not(target_os = "windows"))]
fn load_os_discovered(_regular: &mut Vec<Font>, _seen: &mut HashSet<usize>) {}

fn list_asset_fonts() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("fonts");
    fs::read_dir(&dir).into_iter().flatten()
        .filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.extension().and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf"|"otf"|"ttc")).unwrap_or(false))
        .filter(|p| {
            let f = p.file_name().and_then(|n| n.to_str()).map(|n| n.to_ascii_lowercase()).unwrap_or_default();
            f != "notosans-regular.ttf" && f != "notosans-bold.ttf"
        })
        .collect()
}
