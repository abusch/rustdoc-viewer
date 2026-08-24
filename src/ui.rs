//! Drawing the screens.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Screen};
use crate::index::Entry;

const ACCENT: Color = Color::LightMagenta;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    app.last_width = area.width;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    match app.screen {
        Screen::Search => draw_search(f, app, chunks[0]),
        Screen::Item | Screen::Help => draw_item(f, app, chunks[0]),
    }
    draw_status(f, app, chunks[1]);

    if app.screen == Screen::Help {
        draw_help(f, area);
    }
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    let input = Paragraph::new(Line::from(vec![
        Span::styled("  ", Style::new().fg(ACCENT)),
        Span::raw(app.query.clone()),
        Span::styled("▏", Style::new().fg(ACCENT)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Search ")
            .border_style(Style::new().fg(ACCENT)),
    );
    f.render_widget(input, chunks[0]);

    let list_area = chunks[1];
    let height = list_area.height as usize;
    app.clamp_search_scroll(height);

    let mut lines: Vec<Line> = Vec::new();
    for (row, entry_idx) in app
        .results
        .iter()
        .enumerate()
        .skip(app.search_offset)
        .take(height)
    {
        let e = app.index.entry(*entry_idx);
        lines.push(result_line(e, row == app.selected, list_area.width));
    }

    if app.results.is_empty() {
        lines.push(Line::styled(
            "  no matches",
            Style::new().fg(Color::DarkGray),
        ));
    }

    f.render_widget(Paragraph::new(lines), list_area);
}

fn result_line(e: &Entry, selected: bool, width: u16) -> Line<'static> {
    let (marker, base) = if selected {
        ("▸ ", Style::new().fg(Color::White).add_modifier(Modifier::BOLD))
    } else {
        ("  ", Style::new())
    };

    let kind = kind_badge(&e.kind);
    // Show the module path dimmed, with the name itself highlighted.
    let (parent, name) = match e.path.rsplit_once("::") {
        Some((p, n)) => (format!("{p}::"), n.to_string()),
        None => (String::new(), e.path.clone()),
    };

    let mut spans = vec![
        Span::styled(marker, Style::new().fg(ACCENT)),
        Span::styled(format!("{kind:<9}"), Style::new().fg(Color::Blue)),
        Span::styled(parent, Style::new().fg(Color::DarkGray)),
        Span::styled(name, base.fg(if selected { Color::White } else { Color::Gray })),
    ];
    if selected {
        // Pad so the highlight spans the row.
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if (used as u16) < width {
            spans.push(Span::raw(" ".repeat(width as usize - used)));
        }
        return Line::from(spans).style(Style::new().bg(Color::Rgb(40, 40, 55)));
    }
    Line::from(spans)
}

fn kind_badge(k: &rustdoc_types::ItemKind) -> &'static str {
    use rustdoc_types::ItemKind as K;
    match k {
        K::Struct => "struct",
        K::Enum => "enum",
        K::Trait => "trait",
        K::Function => "fn",
        K::Module => "mod",
        K::Macro => "macro",
        K::TypeAlias => "type",
        K::Constant => "const",
        K::Static => "static",
        K::Primitive => "prim",
        K::Union => "union",
        K::Variant => "variant",
        K::AssocConst => "const",
        K::AssocType => "type",
        K::StructField => "field",
        _ => "item",
    }
}

fn draw_item(f: &mut Frame, app: &mut App, area: Rect) {
    app.viewport = area.height.saturating_sub(2);
    app.ensure_width(area.width);

    let Some(page) = &app.page else {
        f.render_widget(
            Paragraph::new("nothing open — press / to search")
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };

    let focused = app.focus.and_then(|i| page.targets.get(i));
    let focused_line = focused.map(|t| t.line);
    // Mark which section `space` will act on.
    let cursor_line = page.section_lines.get(app.section_cursor).copied();

    let start = app.scroll as usize;
    let visible: Vec<Line> = page
        .lines
        .iter()
        .enumerate()
        .skip(start)
        .take(area.height.saturating_sub(2) as usize)
        .map(|(i, l)| {
            if Some(i) == focused_line {
                let hl = Style::new().bg(Color::Rgb(50, 50, 70));
                let mut l = l.clone();
                match focused.and_then(|t| t.spans.clone()) {
                    // A link highlights its own text only, not the line it sits on.
                    Some(range) => {
                        let n = l.spans.len();
                        for span in &mut l.spans[range.start.min(n)..range.end.min(n)] {
                            span.style = span.style.patch(hl);
                        }
                        l
                    }
                    None => l.style(hl),
                }
            } else if Some(i) == cursor_line {
                l.clone().style(Style::new().bg(Color::Rgb(35, 35, 50)))
            } else {
                l.clone()
            }
        })
        .collect();

    let pct = if page.lines.is_empty() {
        100
    } else {
        let end = (start + area.height as usize).min(page.lines.len());
        end * 100 / page.lines.len()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Line::styled(
            format!(" {} ", page.title),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(
            format!(" {pct}% "),
            Style::new().fg(Color::DarkGray),
        ));

    f.render_widget(Paragraph::new(visible).block(block), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(s) = &app.status {
        s.clone()
    } else {
        match app.screen {
            Screen::Search => {
                format!("{} results · ↑↓ select · ⏎ open · ? help · esc quit", app.results.len())
            }
            _ => "/ search · ⏎ follow · tab/⇧tab link · ^o back · ^f fwd · n/p section · space fold · ? help".into(),
        }
    };
    f.render_widget(
        Paragraph::new(Line::styled(format!(" {text}"), Style::new().fg(Color::DarkGray))),
        area,
    );
}

fn draw_help(f: &mut Frame, area: Rect) {
    let entries = [
        ("/", "open search"),
        ("⏎", "open selected · follow focused link"),
        ("tab", "focus next link (item view)"),
        ("⇧tab", "focus previous link"),
        ("^o / backspace", "go back"),
        ("^f", "go forward"),
        ("j / k / ↑ / ↓", "scroll"),
        ("^d / ^u", "half page"),
        ("g / G", "top / bottom"),
        ("n / p", "next / previous section"),
        ("space", "fold or unfold a section"),
        ("?", "toggle this help"),
        ("q / esc", "quit"),
    ];

    let width = 56u16.min(area.width.saturating_sub(4));
    let height = (entries.len() as u16 + 4).min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    let mut lines = vec![Line::default()];
    for (key, desc) in entries {
        lines.push(Line::from(vec![
            Span::styled(format!("  {key:>15}  "), Style::new().fg(ACCENT)),
            Span::styled(desc.to_string(), Style::new().fg(Color::Gray)),
        ]));
    }

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Keys ")
                .border_style(Style::new().fg(ACCENT)),
        ),
        popup,
    );
}
