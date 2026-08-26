//! Drawing the screens.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Screen};
use crate::index::Entry;
use crate::theme;

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
        Span::styled("  ", theme::accent()),
        Span::raw(app.query.clone()),
        Span::styled("▏", theme::accent()),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Search ")
            .border_style(theme::focused_border()),
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
        lines.push(Line::styled("  no matches", theme::dim()));
    }

    f.render_widget(Paragraph::new(lines), list_area);
}

fn result_line(e: &Entry, selected: bool, width: u16) -> Line<'static> {
    let (marker, base) = if selected {
        ("▸ ", theme::selected_name())
    } else {
        ("  ", theme::unselected_name())
    };

    let kind = kind_badge(&e.kind);
    // Show the module path dimmed, with the name itself highlighted.
    let (parent, name) = match e.path.rsplit_once("::") {
        Some((p, n)) => (format!("{p}::"), n.to_string()),
        None => (String::new(), e.path.clone()),
    };

    let mut spans = vec![
        Span::styled(marker, theme::accent()),
        Span::styled(format!("{kind:<9}"), theme::kind_badge()),
        Span::styled(parent, theme::dim()),
        Span::styled(name, base),
    ];
    if selected {
        // Pad so the highlight spans the row.
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if (used as u16) < width {
            spans.push(Span::raw(" ".repeat(width as usize - used)));
        }
        return Line::from(spans).style(theme::selected_row());
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
                let hl = theme::focused_link();
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
                l.clone().style(theme::cursor_line())
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
        .border_style(theme::unfocused_border())
        .title(Line::styled(format!(" {} ", page.title), theme::title()))
        .title_bottom(Line::styled(format!(" {pct}% "), theme::dim()));

    f.render_widget(Paragraph::new(visible).block(block), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(s) = &app.status {
        s.clone()
    } else {
        match app.screen {
            Screen::Search => {
                format!(
                    "{} results · ↑↓ select · ⏎ open · esc back · ? help",
                    app.results.len()
                )
            }
            // The full list of item-view bindings does not fit an 80-column
            // terminal, and this line is truncated rather than wrapped. Drop
            // hints from the end until it fits, keeping `? help` pinned so the
            // way to the rest is never the part that gets cut.
            _ => fit_hints(
                &[
                    "/ search",
                    "⏎ follow",
                    "tab/⇧tab link",
                    "u up",
                    "^o back",
                    "^f fwd",
                    "n/p section",
                    "space fold",
                ],
                "? help",
                area.width.saturating_sub(1),
            ),
        }
    };
    f.render_widget(
        Paragraph::new(Line::styled(format!(" {text}"), theme::status_bar())),
        area,
    );
}

/// Join as many `hints` as fit in `width`, always keeping `pinned` last.
fn fit_hints(hints: &[&str], pinned: &str, width: u16) -> String {
    const SEP: &str = " · ";
    let width = width as usize;

    let mut out = String::new();
    for hint in hints {
        let extra = if out.is_empty() {
            hint.chars().count()
        } else {
            SEP.chars().count() + hint.chars().count()
        };
        // Leave room for the separator and the pinned hint that follow.
        let reserved = SEP.chars().count() + pinned.chars().count();
        if out.chars().count() + extra + reserved > width {
            break;
        }
        if !out.is_empty() {
            out.push_str(SEP);
        }
        out.push_str(hint);
    }

    if out.is_empty() {
        return pinned.to_string();
    }
    out.push_str(SEP);
    out.push_str(pinned);
    out
}

fn draw_help(f: &mut Frame, area: Rect) {
    let entries = [
        ("/", "open search"),
        ("⏎", "open selected · follow focused link"),
        ("tab", "focus next link (item view)"),
        ("⇧tab", "focus previous link"),
        ("u", "go up to the parent item"),
        ("^o / backspace", "go back"),
        ("^f", "go forward"),
        ("j / k / ↑ / ↓", "scroll"),
        ("^d / ^u", "half page"),
        ("g / G", "top / bottom"),
        ("n / p", "next / previous section"),
        ("space", "fold or unfold a section"),
        ("?", "toggle this help"),
        ("esc", "close search or help"),
        ("q", "quit"),
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
            Span::styled(format!("  {key:>15}  "), theme::accent()),
            Span::styled(desc.to_string(), theme::muted()),
        ]));
    }

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Keys ")
                .border_style(theme::focused_border()),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_fit_the_width_and_always_keep_the_pinned_one() {
        let hints = ["/ search", "⏎ follow", "tab/⇧tab link", "u up"];

        // Wide enough for everything.
        let all = fit_hints(&hints, "? help", 80);
        assert_eq!(all, "/ search · ⏎ follow · tab/⇧tab link · u up · ? help");
        assert!(all.chars().count() <= 80);

        // Too narrow: hints drop off the end, `? help` survives.
        for width in [10, 20, 30, 40, 50] {
            let line = fit_hints(&hints, "? help", width);
            assert!(
                line.chars().count() <= width as usize || line == "? help",
                "width {width} overflowed: {line:?}"
            );
            assert!(line.ends_with("? help"), "width {width} lost the help hint");
        }

        // Narrower than the pinned hint itself: still report it.
        assert_eq!(fit_hints(&hints, "? help", 2), "? help");
    }

    /// The item-view footer is measured in characters, not bytes: the
    /// separators and arrows are multi-byte.
    #[test]
    fn hints_are_measured_in_characters() {
        let line = fit_hints(&["⏎ follow", "⇧tab link"], "? help", 30);
        assert!(line.chars().count() <= 30, "{line:?}");
    }

    /// A method opened from search is not just focused in the model: the row
    /// is actually painted as focused in the rendered frame.
    #[test]
    fn an_opened_method_row_is_highlighted_on_screen() {
        use crate::app::App;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (u, idx) = crate::testdocs::indexed();
        let mut app = App::new(u, idx);
        let m = app
            .universe
            .by_path("alloc::string::String::push_str")
            .expect("String::push_str");
        app.navigate_to(m);

        let mut term = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        term.draw(|f| draw(f, &mut app)).expect("draw");
        let buf = term.backend().buffer();

        // Find the row carrying the declaration, and confirm it is painted
        // with the focus background rather than the page background.
        let want = theme::focused_link().bg.expect("focus bg");
        let mut found = false;
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            if row.contains("fn push_str") {
                found = true;
                let bg = buf[(6, y)].bg;
                assert_eq!(bg, want, "method row {y} is not highlighted: {row:?}");
            }
        }
        assert!(found, "the focused method row was never drawn");
    }
}
