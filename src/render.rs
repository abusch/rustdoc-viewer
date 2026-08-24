//! Turning rustdoc markdown and item structures into styled terminal text.

use std::collections::HashMap;

use pulldown_cmark::{BrokenLink, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use rustdoc_types::{Id, Item};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// A link target discovered while rendering docs, so the UI can follow it.
///
/// A link is addressed by the exact spans it occupies, not merely by its line,
/// so the UI can highlight the link text alone. Wrapping can split one link
/// across lines, in which case it yields one `LinkTarget` per line.
#[derive(Debug, Clone)]
pub struct LinkTarget {
    /// Line within the rendered block that carries the link.
    pub line: usize,
    /// Half-open range of span indices within that line covered by the link.
    pub spans: std::ops::Range<usize>,
    pub id: Id,
}

/// Rendered documentation: styled lines plus the links found in them.
#[derive(Debug, Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<LinkTarget>,
}

/// Syntax highlighting resources, built once and reused.
pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    pub fn new() -> Self {
        let syntaxes = two_face::syntax::extra_newlines();
        let theme = ThemeSet::load_defaults().themes["base16-eighties.dark"].clone();
        Self { syntaxes, theme }
    }

    /// Highlight a block of code, returning one styled line per source line.
    fn highlight(&self, code: &str, lang: &str) -> Vec<Line<'static>> {
        let syntax = self
            .syntaxes
            .find_syntax_by_token(lang)
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        let mut h = HighlightLines::new(syntax, &self.theme);

        let mut out = Vec::new();
        for raw in LinesWithEndings::from(code) {
            let Ok(ranges) = h.highlight_line(raw, &self.syntaxes) else {
                out.push(Line::from(raw.trim_end_matches('\n').to_string()));
                continue;
            };
            let spans: Vec<Span<'static>> = ranges
                .into_iter()
                .map(|(style, text)| {
                    Span::styled(text.trim_end_matches('\n').to_string(), convert(style))
                })
                .filter(|s| !s.content.is_empty())
                .collect();
            out.push(Line::from(spans));
        }
        out
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a syntect style to a ratatui one.
fn convert(s: syntect::highlighting::Style) -> Style {
    let fg = Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b);
    let mut style = Style::default().fg(fg);
    if s.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}

/// Strip rustdoc's fence annotations and pick a highlighting language.
///
/// Rustdoc fences carry attributes rather than a language — ```` ```ignore
/// (needs windows) ````, ```` ```no_run ````, ```` ```should_panic ````. Any
/// token that is not a known language means "this is still Rust", which is
/// rustdoc's own default for an unannotated block.
pub fn code_language(info: &str) -> &'static str {
    const ATTRS: [&str; 10] = [
        "ignore",
        "no_run",
        "should_panic",
        "compile_fail",
        "edition2015",
        "edition2018",
        "edition2021",
        "edition2024",
        "test_harness",
        "standalone_crate",
    ];

    let first = info.trim().split([',', ' ']).next().unwrap_or("").trim();
    if first.is_empty() || ATTRS.contains(&first) || first.starts_with("edition") {
        return "rs";
    }
    match first {
        "rust" | "rs" => "rs",
        "text" | "txt" => "txt",
        "toml" => "toml",
        "json" => "json",
        "sh" | "bash" | "shell" | "console" => "sh",
        "c" => "c",
        "cpp" | "c++" => "cpp",
        "python" | "py" => "py",
        // Unknown annotation: rustdoc still treats the block as Rust.
        _ => "rs",
    }
}

/// Drop rustdoc's hidden lines (`# ...`), matching what the website shows.
fn strip_hidden(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    for line in code.lines() {
        let t = line.trim_start();
        // `##` is an escaped `#`, not a hidden line.
        if t == "#" || (t.starts_with("# ") && !t.starts_with("##")) {
            continue;
        }
        let line = if let Some(rest) = t.strip_prefix("##") {
            format!("{}#{}", &line[..line.len() - t.len()], rest)
        } else {
            line.to_string()
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

const HEADING: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
const CODE: Style = Style::new().fg(Color::LightGreen);
const LINK: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
const QUOTE: Style = Style::new().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);

/// Marks a span as belonging to link number `n`.
///
/// Link extents cannot be recorded when the link is emitted: at that point the
/// text is still an unwrapped run of spans, and `wrap_into` decides only later
/// which line each piece lands on. So links are tagged here and their extents
/// recovered from the wrapped lines afterwards. `underline_color` is unused by
/// this renderer, carries through wrapping untouched, and is cleared once the
/// tag has been read back.
fn tag(style: Style, n: usize) -> Style {
    style.underline_color(Color::Indexed(u8::try_from(n % 255).unwrap_or(0) + 1))
}

/// Read back the link number a span was tagged with, if any.
fn tag_of(style: Style) -> Option<usize> {
    match style.underline_color {
        Some(Color::Indexed(n)) if n > 0 => Some(usize::from(n - 1)),
        _ => None,
    }
}

/// Render an item's markdown docs to styled, width-wrapped lines.
pub fn markdown(
    docs: &str,
    links: &HashMap<String, Id>,
    width: u16,
    hl: &Highlighter,
) -> Rendered {
    let width = width.max(20) as usize;
    let mut out = Rendered::default();

    // State accumulated across events for the current paragraph.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let mut list_depth: usize = 0;
    let mut in_code: Option<String> = None;
    let mut code_lang = "rs";
    let mut quote_depth = 0usize;
    let mut pending_link: Option<String> = None;
    // Links tagged in the current unflushed run, indexed by tag number.
    let mut pending_ids: Vec<Id> = Vec::new();

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);

    // Rustdoc writes `[`Foo`]` shortcut links whose definitions live in
    // `Item.links`, not in the markdown. Without a broken-link callback
    // pulldown-cmark emits the brackets as literal text, so every intra-doc
    // link would render as `[`Foo`]`. Claiming them here turns them into real
    // link events whose destination we then resolve through `links`.
    let mut callback = |broken: BrokenLink<'_>| {
        let raw = broken.reference.to_string();
        Some((raw.into(), pulldown_cmark::CowStr::Borrowed("")))
    };

    // Flush the current inline run as wrapped lines with the given indent,
    // then recover the extent of every link tagged within it.
    macro_rules! flush {
        ($indent:expr) => {{
            if !spans.is_empty() {
                let indent: String = $indent;
                let from = out.lines.len();
                wrap_into(&mut out.lines, std::mem::take(&mut spans), width, &indent);
                collect_links(&mut out, from, &pending_ids);
            }
            pending_ids.clear();
        }};
    }

    let parser = Parser::new_with_broken_link_callback(docs, opts, Some(&mut callback));
    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush!(String::new());
                if !out.lines.is_empty() {
                    out.lines.push(Line::default());
                }
                let hashes = match level {
                    HeadingLevel::H1 => "",
                    HeadingLevel::H2 => "",
                    _ => "",
                };
                let _ = hashes;
                style = HEADING;
            }
            Event::End(TagEnd::Heading(_)) => {
                flush!(String::new());
                style = Style::default();
                out.lines.push(Line::default());
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                let indent = "  ".repeat(list_depth) + &"> ".repeat(quote_depth);
                flush!(indent);
                out.lines.push(Line::default());
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush!(String::new());
                code_lang = match &kind {
                    CodeBlockKind::Fenced(info) => code_language(info),
                    CodeBlockKind::Indented => "rs",
                };
                in_code = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(code) = in_code.take() {
                    let code = strip_hidden(&code);
                    for line in hl.highlight(&code, code_lang) {
                        let mut spans = vec![Span::raw("    ")];
                        spans.extend(line.spans);
                        out.lines.push(Line::from(spans));
                    }
                    out.lines.push(Line::default());
                }
            }
            Event::Start(Tag::List(_)) => {
                flush!(String::new());
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    out.lines.push(Line::default());
                }
            }
            Event::Start(Tag::Item) => {
                spans.push(Span::raw("• "));
            }
            Event::End(TagEnd::Item) => {
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                flush!(indent);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush!(String::new());
                quote_depth += 1;
                style = QUOTE;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                quote_depth = quote_depth.saturating_sub(1);
                if quote_depth == 0 {
                    style = Style::default();
                }
            }
            Event::Start(Tag::Emphasis) => style = style.add_modifier(Modifier::ITALIC),
            Event::End(TagEnd::Emphasis) => style = style.remove_modifier(Modifier::ITALIC),
            Event::Start(Tag::Strong) => style = style.add_modifier(Modifier::BOLD),
            Event::End(TagEnd::Strong) => style = style.remove_modifier(Modifier::BOLD),
            Event::Start(Tag::Link { dest_url, .. }) => {
                pending_link = Some(dest_url.to_string());
                style = LINK;
            }
            Event::End(TagEnd::Link) => {
                pending_link = None;
                style = Style::default();
            }
            Event::Code(text) => {
                // Inline code may itself be an intra-doc link target.
                let key = text.to_string();
                let mut st = CODE;
                if let Some(id) = lookup(links, &key, pending_link.as_deref()) {
                    st = tag(LINK, pending_ids.len());
                    pending_ids.push(id);
                }
                spans.push(Span::styled(format!("`{key}`"), st));
            }
            Event::Text(text) => {
                if let Some(code) = in_code.as_mut() {
                    code.push_str(&text);
                } else {
                    let mut st = style;
                    if let Some(id) = lookup(links, &text, pending_link.as_deref()) {
                        st = tag(style, pending_ids.len());
                        pending_ids.push(id);
                    }
                    spans.push(Span::styled(text.to_string(), st));
                }
            }
            Event::SoftBreak => spans.push(Span::raw(" ")),
            Event::HardBreak => {
                let indent = "  ".repeat(list_depth);
                flush!(indent);
            }
            Event::Rule => {
                flush!(String::new());
                out.lines
                    .push(Line::styled("─".repeat(width.min(60)), Style::new().fg(Color::DarkGray)));
            }
            _ => {}
        }
    }
    flush!(String::new());

    // Trailing blank lines add nothing but scrolling.
    while out.lines.last().is_some_and(|l| l.width() == 0) {
        out.lines.pop();
    }
    out
}

/// Recover link extents from lines `from..` and clear the tags.
///
/// Wrapping may split a tagged run across lines; each line the tag appears on
/// becomes its own `LinkTarget`, so the highlight follows the text exactly.
fn collect_links(out: &mut Rendered, from: usize, ids: &[Id]) {
    for line in from..out.lines.len() {
        // Run of consecutive spans sharing one tag: (tag, start index).
        let mut run: Option<(usize, usize)> = None;
        let len = out.lines[line].spans.len();
        for i in 0..=len {
            let here = (i < len).then(|| tag_of(out.lines[line].spans[i].style)).flatten();
            match (run, here) {
                (Some((n, _)), Some(m)) if n == m => continue,
                (Some((n, start)), _) => {
                    if let Some(id) = ids.get(n) {
                        out.links.push(LinkTarget { line, spans: start..i, id: *id });
                    }
                    run = here.map(|m| (m, i));
                }
                (None, Some(m)) => run = Some((m, i)),
                (None, None) => {}
            }
        }
        for span in &mut out.lines[line].spans {
            span.style.underline_color = None;
        }
    }
}

/// Resolve a link by its display text or its destination.
///
/// Rustdoc keys `Item.links` by the link text as written, which may or may not
/// include the surrounding backticks.
fn lookup(links: &HashMap<String, Id>, text: &str, dest: Option<&str>) -> Option<Id> {
    if let Some(d) = dest
        && let Some(id) = links.get(d)
    {
        return Some(*id);
    }
    links
        .get(text)
        .or_else(|| links.get(&format!("`{text}`")))
        .copied()
}

/// Word-wrap a run of spans to `width`, preserving each span's style.
fn wrap_into(out: &mut Vec<Line<'static>>, spans: Vec<Span<'static>>, width: usize, indent: &str) {
    let avail = width.saturating_sub(indent.len()).max(8);
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;

    // Continuation lines line up under the first, past any bullet.
    let cont: String = " ".repeat(indent.len());
    let mut first = true;

    for span in spans {
        let style = span.style;
        for word in split_keeping_spaces(&span.content) {
            let w = word.chars().count();
            // Never start a line with the space that followed a wrap.
            if used == 0 && word.trim().is_empty() {
                continue;
            }
            if used + w > avail && used > 0 {
                out.push(prefixed(if first { indent } else { &cont }, std::mem::take(&mut current)));
                first = false;
                used = 0;
                if word.trim().is_empty() {
                    continue;
                }
            }
            used += w;
            current.push(Span::styled(word, style));
        }
    }
    if !current.is_empty() {
        out.push(prefixed(if first { indent } else { &cont }, current));
    }
}

fn prefixed(indent: &str, mut spans: Vec<Span<'static>>) -> Line<'static> {
    if indent.is_empty() {
        return Line::from(spans);
    }
    let mut v = vec![Span::raw(indent.to_string())];
    v.append(&mut spans);
    Line::from(v)
}

/// Split text into words, keeping the whitespace as its own chunks.
fn split_keeping_spaces(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_space = false;
    for c in s.chars() {
        let is_space = c.is_whitespace();
        if !buf.is_empty() && is_space != in_space {
            out.push(std::mem::take(&mut buf));
        }
        in_space = is_space;
        buf.push(c);
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Convenience: render an item's own docs.
pub fn item_docs(item: &Item, width: u16, hl: &Highlighter) -> Rendered {
    match &item.docs {
        Some(d) if !d.is_empty() => markdown(d, &item.links, width, hl),
        _ => Rendered::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_rustdoc_fence_attributes() {
        assert_eq!(code_language(""), "rs");
        assert_eq!(code_language("ignore (needs windows)"), "rs");
        assert_eq!(code_language("no_run"), "rs");
        assert_eq!(code_language("should_panic"), "rs");
        assert_eq!(code_language("compile_fail,edition2021"), "rs");
        assert_eq!(code_language("edition2024"), "rs");
        assert_eq!(code_language("text"), "txt");
        assert_eq!(code_language("toml"), "toml");
        // An unknown annotation still means Rust, per rustdoc's default.
        assert_eq!(code_language("weird_attr"), "rs");
    }

    #[test]
    fn removes_hidden_lines() {
        let code = "# use std::fmt;\nlet x = 1;\n# hidden();\n";
        assert_eq!(strip_hidden(code), "let x = 1;\n");
    }

    #[test]
    fn unescapes_double_hash() {
        // `##` is an escaped `#`, and must survive as a single `#`.
        let code = "## not hidden\n";
        assert_eq!(strip_hidden(code), "# not hidden\n");
    }

    #[test]
    fn wraps_to_width() {
        let hl = Highlighter::new();
        let text = "word ".repeat(40);
        let r = markdown(&text, &HashMap::new(), 30, &hl);
        assert!(r.lines.len() > 1, "expected wrapping");
        for line in &r.lines {
            assert!(line.width() <= 30, "line too wide: {}", line.width());
        }
    }

    #[test]
    fn finds_intra_doc_links() {
        let hl = Highlighter::new();
        let mut links = HashMap::new();
        links.insert("`None`".to_string(), Id(59));
        let r = markdown("See [`None`] for details.", &links, 60, &hl);
        assert_eq!(r.links.len(), 1);
        assert_eq!(r.links[0].id, Id(59));
    }

    /// Text of the spans a link covers, which is what gets highlighted.
    fn link_text(r: &Rendered, i: usize) -> String {
        let t = &r.links[i];
        r.lines[t.line].spans[t.spans.clone()]
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn link_extent_covers_only_the_link_text() {
        let hl = Highlighter::new();
        let mut links = HashMap::new();
        links.insert("`None`".to_string(), Id(59));
        let r = markdown("See [`None`] for details.", &links, 60, &hl);

        assert_eq!(link_text(&r, 0), "`None`");
        // The surrounding prose must stay outside the highlighted range.
        assert!(r.lines[r.links[0].line].width() > 6);
    }

    #[test]
    fn multiple_links_get_separate_extents() {
        let hl = Highlighter::new();
        let mut links = HashMap::new();
        links.insert("`Some`".to_string(), Id(1));
        links.insert("`None`".to_string(), Id(2));
        let r = markdown("Either [`Some`] or [`None`] here.", &links, 60, &hl);

        assert_eq!(r.links.len(), 2);
        assert_eq!(link_text(&r, 0), "`Some`");
        assert_eq!(link_text(&r, 1), "`None`");
        assert_eq!(r.links[0].id, Id(1));
        assert_eq!(r.links[1].id, Id(2));
    }

    #[test]
    fn link_tags_do_not_survive_into_rendered_spans() {
        let hl = Highlighter::new();
        let mut links = HashMap::new();
        links.insert("`None`".to_string(), Id(59));
        let r = markdown("See [`None`] for details.", &links, 60, &hl);

        // The tag is an internal marker; leaving it set would paint a stray
        // underline colour in the terminal.
        for line in &r.lines {
            for span in &line.spans {
                assert!(span.style.underline_color.is_none());
            }
        }
    }

    #[test]
    fn a_link_wrapped_across_lines_yields_one_target_per_line() {
        let hl = Highlighter::new();
        let mut links = HashMap::new();
        links.insert("a very long link label that must wrap".to_string(), Id(7));
        let r = markdown(
            "x [a very long link label that must wrap](y) z",
            &links,
            20,
            &hl,
        );

        assert!(r.links.len() > 1, "expected the link to span several lines");
        assert!(r.links.iter().all(|t| t.id == Id(7)));
        // Each target must point at a distinct line and cover a real range.
        let mut lines: Vec<usize> = r.links.iter().map(|t| t.line).collect();
        lines.dedup();
        assert_eq!(lines.len(), r.links.len());
        assert!(r.links.iter().all(|t| t.spans.start < t.spans.end));
    }
}
