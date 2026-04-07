// ── Lynx-style frame renderer ─────────────────────────────────────────────────

use crate::tui::app::{App, AppMode};
use crate::tui::COLORS;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Background
    frame.render_widget(Block::default().style(Style::default().bg(COLORS.bg)), area);

    //  1 line  — title bar
    //  fill    — page content
    //  1 line  — hints
    //  1 line  — URL / status bar
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    render_titlebar(frame, app, chunks[0]);

    match app.mode {
        AppMode::NewTab => render_newtab(frame, app, chunks[1]),
        AppMode::Loading => render_loading(frame, app, chunks[1]),
        AppMode::Page | AppMode::Input => render_page(frame, app, chunks[1]),
    }

    render_hints(frame, chunks[2]);
    render_urlbar(frame, app, chunks[3]);
}

// ── Title bar ─────────────────────────────────────────────────────────────────

fn render_titlebar(frame: &mut Frame, app: &App, area: Rect) {
    let title = app
        .current_page
        .as_ref()
        .map(|p| p.title.to_uppercase())
        .unwrap_or_else(|| "A-Z BROWSER".to_string());

    let page_info = app
        .current_page
        .as_ref()
        .map(|_| format!(" (p{})", (app.scroll_offset / area.height.max(1)) + 1))
        .unwrap_or_default();

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(title, Style::default().fg(COLORS.bg).bg(COLORS.text)),
            Span::styled(
                page_info,
                Style::default().fg(COLORS.text_dim).bg(COLORS.bg_dark),
            ),
        ]))
        .style(Style::default().bg(COLORS.bg_dark)),
        area,
    );
}

// ── Page content (lynx-style) ─────────────────────────────────────────────────

fn render_page(frame: &mut Frame, app: &App, area: Rect) {
    let Some(page) = &app.current_page else {
        render_newtab(frame, app, area);
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    for rich_line in &page.rendered_lines {
        if rich_line.is_empty() {
            lines.push(Line::raw(""));
            continue;
        }

        let mut spans: Vec<Span> = Vec::new();
        for span in rich_line {
            let style = match span.link_idx {
                Some(i) if app.selected_link == Some(i) => {
                    // Selected link: reverse video like lynx
                    Style::default().fg(COLORS.bg_dark).bg(COLORS.text)
                }
                Some(_) => {
                    // Normal link: underlined
                    Style::default()
                        .fg(COLORS.blue)
                        .add_modifier(Modifier::UNDERLINED)
                }
                None if span.heading => Style::default().fg(COLORS.green),
                None => Style::default().fg(COLORS.text),
            };
            spans.push(Span::styled(span.text.clone(), style));
        }
        lines.push(Line::from(spans));
    }

    // ── Videos ────────────────────────────────────────────────────────────────
    if !app
        .current_page
        .as_ref()
        .map(|p| p.videos.is_empty())
        .unwrap_or(true)
    {
        let page = app.current_page.as_ref().unwrap();
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "── Videos ───────────────────────────────────────────────",
            Style::default().fg(COLORS.text_dim),
        )));
        lines.push(Line::raw(""));
        for video in &page.videos {
            use a_z::browser::video::VideoType;
            let tag = match video.video_type {
                VideoType::YouTube => "[YT]  ",
                VideoType::Hls => "[HLS] ",
                VideoType::Dash => "[DASH]",
                _ => "[vid] ",
            };
            lines.push(Line::from(vec![
                Span::styled(tag, Style::default().fg(COLORS.orange)),
                Span::styled(&video.title, Style::default().fg(COLORS.text)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("       "),
                Span::styled(
                    "[P] Play in external player",
                    Style::default().fg(COLORS.green),
                ),
            ]));
            lines.push(Line::raw(""));
        }
    }

    // ── Forms ─────────────────────────────────────────────────────────────────
    if !app
        .current_page
        .as_ref()
        .map(|p| p.forms.is_empty())
        .unwrap_or(true)
    {
        let page = app.current_page.as_ref().unwrap();
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "── Forms ────────────────────────────────────────────────",
            Style::default().fg(COLORS.text_dim),
        )));
        lines.push(Line::raw(""));
        for form in &page.forms {
            let label = if form.action.is_empty() {
                "form"
            } else {
                &form.action
            };
            lines.push(Line::from(Span::styled(
                format!("  [Form: {}]", label),
                Style::default().fg(COLORS.orange),
            )));
            for field in &form.fields {
                if field.field_type == "hidden" {
                    continue;
                }
                let display = if field.field_type == "password" {
                    "••••••••".to_string()
                } else if !field.value.is_empty() {
                    field.value.clone()
                } else if !field.placeholder.is_empty() {
                    field.placeholder.clone()
                } else {
                    "________________".to_string()
                };
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        format!("{}: ", field.name),
                        Style::default().fg(COLORS.text_dim),
                    ),
                    Span::styled(format!("[{}]", display), Style::default().fg(COLORS.yellow)),
                ]));
            }
            lines.push(Line::raw(""));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (page has no readable content)",
            Style::default().fg(COLORS.text_dim),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(COLORS.bg))
            .scroll((app.scroll_offset, 0)),
        area,
    );
}

// ── New tab / home ────────────────────────────────────────────────────────────

fn render_newtab(frame: &mut Frame, app: &App, area: Rect) {
    let vert = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(9),
        Constraint::Fill(1),
    ])
    .split(area);

    let input_len = app.search_input.len();
    let blanks = "_".repeat(44_usize.saturating_sub(input_len));

    let content = vec![
        Line::from(Span::styled("A-Z", Style::default().fg(COLORS.green))),
        Line::raw(""),
        Line::from(vec![
            Span::styled("[", Style::default().fg(COLORS.text_dim)),
            Span::styled(&app.search_input, Style::default().fg(COLORS.text)),
            Span::styled(blanks, Style::default().fg(COLORS.text_dim)),
            Span::styled("]", Style::default().fg(COLORS.text_dim)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "Enter: search   /: URL bar   q: quit",
            Style::default().fg(COLORS.text_dim),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "────────────────────────────────────────────────",
            Style::default().fg(COLORS.text_dim),
        )),
        Line::from(vec![
            Span::styled(
                "[Google]  ",
                Style::default()
                    .fg(COLORS.blue)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Span::styled(
                "[GitHub]  ",
                Style::default()
                    .fg(COLORS.blue)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Span::styled(
                "[Wikipedia]  ",
                Style::default()
                    .fg(COLORS.blue)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Span::styled(
                "[YouTube]",
                Style::default()
                    .fg(COLORS.blue)
                    .add_modifier(Modifier::UNDERLINED),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(content)
            .alignment(Alignment::Center)
            .style(Style::default().bg(COLORS.bg)),
        vert[1],
    );
}

// ── Loading ───────────────────────────────────────────────────────────────────

fn render_loading(frame: &mut Frame, app: &App, area: Rect) {
    let url = app.loading_url.as_deref().unwrap_or("...");
    let vert = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  Loading {}...", url),
            Style::default().fg(COLORS.yellow),
        )))
        .style(Style::default().bg(COLORS.bg)),
        vert[1],
    );
}

// ── Hint bar ──────────────────────────────────────────────────────────────────

fn render_hints(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Arrow keys: Up and Down to move.  Right/Enter: follow link.  Left/b: back.  /: URL  q: quit",
            Style::default().fg(COLORS.text_dim),
        )))
        .style(Style::default().bg(COLORS.bg_dark)),
        area,
    );
}

// ── URL / status bar ──────────────────────────────────────────────────────────

fn render_urlbar(frame: &mut Frame, app: &App, area: Rect) {
    let line = match app.mode {
        AppMode::Input => Line::from(vec![
            Span::styled("URL: ", Style::default().fg(COLORS.text_dim)),
            Span::styled(&app.url_input, Style::default().fg(COLORS.text)),
            Span::styled("_", Style::default().fg(COLORS.text)),
        ]),
        AppMode::Loading => Line::from(vec![Span::styled(
            format!("  Loading {}...", app.loading_url.as_deref().unwrap_or("")),
            Style::default().fg(COLORS.yellow),
        )]),
        _ => {
            let url = app
                .current_page
                .as_ref()
                .map(|p| p.url.as_str())
                .unwrap_or("about:blank");
            Line::from(vec![
                Span::styled(url, Style::default().fg(COLORS.green)),
                Span::raw("  "),
                Span::styled(&app.status_text, Style::default().fg(COLORS.text_dim)),
            ])
        }
    };

    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(COLORS.bg_dark)),
        area,
    );
}
