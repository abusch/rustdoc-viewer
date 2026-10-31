//! Turning rustdoc markdown and item structures into styled terminal text.

use std::cell::RefCell;
use std::collections::HashMap;

use arborium_theme::{HIGHLIGHTS, Theme};
use pulldown_cmark::{BrokenLink, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use rustdoc_types::{Id, Item};

use crate::theme;

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
///
/// Grammars are compiled lazily on first use and then cached, so constructing
/// this is cheap; the first Rust code block pays the compilation cost.
pub struct Highlighter {
    /// Arborium needs `&mut` to parse, but rendering only ever holds a shared
    /// reference to the highlighter, so the parser state lives behind a cell.
    inner: RefCell<arborium::Highlighter>,
    theme: Theme,
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(arborium::Highlighter::new()),
            theme: theme::syntax(),
        }
    }

    /// Highlight Rust code, returning one styled line per source line.
    ///
    /// Rust is the only grammar built in. Code it cannot parse falls back to
    /// [`plain`].
    fn highlight(&self, code: &str) -> Vec<Line<'static>> {
        // Arborium trims trailing newlines itself, so span offsets are relative
        // to the trimmed source. Trim up front to keep our slicing in step.
        let code = code.trim_end_matches('\n');
        let Ok(spans) = self.inner.borrow_mut().highlight_spans("rust", code) else {
            return plain(code);
        };
        // Raw spans overlap and are unordered; flattening resolves them into a
        // sorted, disjoint token stream, innermost match winning.
        let tokens = arborium_highlight::spans_to_flat_tokens(code, spans);
        if tokens.is_empty() {
            return plain(code);
        }

        let mut lines: Vec<Line<'static>> = vec![Line::default()];
        // A token may straddle newlines, so text is emitted piecewise and split
        // wherever it crosses into the next line.
        let push = |lines: &mut Vec<Line<'static>>, text: &str, style: Style| {
            let mut parts = text.split('\n').peekable();
            while let Some(part) = parts.next() {
                if !part.is_empty() {
                    let line: &mut Line<'static> = lines.last_mut().expect("never empty");
                    line.push_span(Span::styled(part.to_string(), style));
                }
                if parts.peek().is_some() {
                    lines.push(Line::default());
                }
            }
        };

        // Tokens cover only what the grammar matched; the gaps between them are
        // ordinary text and still have to be emitted.
        let mut pos = 0;
        for token in &tokens {
            let start = (token.start as usize).min(code.len());
            let end = (token.end as usize).min(code.len());
            if start > pos {
                push(&mut lines, &code[pos..start], Style::default());
            }
            if end > start {
                push(&mut lines, &code[start..end], self.style_for(token.tag));
            }
            pos = pos.max(end);
        }
        if pos < code.len() {
            push(&mut lines, &code[pos..], Style::default());
        }
        lines
    }

    /// Look up the theme style for a resolved highlight tag.
    fn style_for(&self, tag: &str) -> Style {
        // Tags are theme-slot abbreviations ("k", "cr"); the theme indexes its
        // styles by position in HIGHLIGHTS, and exposes no direct tag lookup.
        let Some(index) = HIGHLIGHTS.iter().position(|h| h.tag == tag) else {
            return Style::default();
        };
        let Some(style) = self.theme.style(index) else {
            return Style::default();
        };

        let mut out = Style::default();
        if let Some(c) = style.fg {
            out = out.fg(Color::Rgb(c.r, c.g, c.b));
        }
        if style.modifiers.bold {
            out = out.add_modifier(Modifier::BOLD);
        }
        if style.modifiers.italic {
            out = out.add_modifier(Modifier::ITALIC);
        }
        out
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Code as unstyled lines, for anything that is not Rust.
fn plain(code: &str) -> Vec<Line<'static>> {
    code.trim_end_matches('\n')
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect()
}

/// Whether a fenced block is Rust, judging by its info string the way
/// rustdoc's `LangString::parse` does.
///
/// A block is Rust unless it carries a word rustdoc does not know (`text`,
/// `toml`, a typo) — any such word is taken to name another language — or the
/// `custom` tag. A doctest attribute (`no_run`, `ignore`, ...) or `rust` itself
/// keeps it Rust despite an unknown word, though the first few only count when
/// they come before it. `edition20xx` is neutral, as are `(comments)` and
/// `{attribute blocks}`.
fn is_rust(info: &str) -> bool {
    let mut seen_rust = false;
    let mut seen_other = false;
    for token in fence_tokens(info) {
        match token {
            "rust" => seen_rust = true,
            "custom" => return false,
            "should_panic" | "no_run" | "ignore" => seen_rust = !seen_other,
            t if t.starts_with("ignore-") => seen_rust = !seen_other,
            "test_harness" | "compile_fail" | "standalone_crate" => {
                seen_rust = !seen_other || seen_rust;
            }
            t if t.starts_with("edition") => {}
            _ => seen_other = true,
        }
    }
    !seen_other || seen_rust
}

/// The words of a fence's info string, with `(comments)` and `{attribute
/// blocks}` dropped, as rustdoc's tag iterator reads them.
fn fence_tokens(info: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut skip_until = None;
    for (i, c) in info.char_indices() {
        if let Some(close) = skip_until {
            if c == close {
                skip_until = None;
            }
            continue;
        }
        match c {
            ' ' | ',' | '\t' | '(' | '{' => {
                if let Some(s) = start.take() {
                    tokens.push(&info[s..i]);
                }
                skip_until = match c {
                    '(' => Some(')'),
                    '{' => Some('}'),
                    _ => None,
                };
            }
            _ if start.is_none() => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        tokens.push(&info[s..]);
    }
    tokens
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

/// Inline text waiting to be wrapped: each span with the link it belongs to.
///
/// A link's extent cannot be recorded when the link is emitted, because only
/// wrapping decides which line each piece lands on. Carrying the link beside
/// the span lets [`flush`] record the extent as it lays the lines out.
type Run = Vec<(Span<'static>, Option<Id>)>;

/// Render an item's markdown docs to styled, width-wrapped lines.
pub fn markdown(docs: &str, links: &HashMap<String, Id>, width: u16, hl: &Highlighter) -> Rendered {
    let width = width.max(20) as usize;
    let mut out = Rendered::default();

    // State accumulated across events for the current paragraph.
    let mut run: Run = Vec::new();
    let mut style = Style::default();
    let mut list_depth: usize = 0;
    let mut in_code: Option<String> = None;
    let mut code_is_rust = true;
    let mut quote_depth = 0usize;
    let mut pending_link: Option<String> = None;

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

    let parser = Parser::new_with_broken_link_callback(docs, opts, Some(&mut callback));
    for event in parser {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                flush(&mut out, &mut run, width, "");
                if !out.lines.is_empty() {
                    out.lines.push(Line::default());
                }
                style = theme::heading();
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut out, &mut run, width, "");
                style = Style::default();
                out.lines.push(Line::default());
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                let indent = "  ".repeat(list_depth) + &"> ".repeat(quote_depth);
                flush(&mut out, &mut run, width, &indent);
                out.lines.push(Line::default());
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut out, &mut run, width, "");
                code_is_rust = match &kind {
                    CodeBlockKind::Fenced(info) => is_rust(info),
                    CodeBlockKind::Indented => true,
                };
                in_code = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(code) = in_code.take() {
                    // Hidden lines are a doctest feature; in a shell or TOML
                    // example a leading `#` is a comment the reader should see.
                    let lines = if code_is_rust {
                        hl.highlight(&strip_hidden(&code))
                    } else {
                        plain(&code)
                    };
                    for line in lines {
                        out.lines.push(code_line(line, width));
                    }
                    out.lines.push(Line::default());
                }
            }
            Event::Start(Tag::List(_)) => {
                flush(&mut out, &mut run, width, "");
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    out.lines.push(Line::default());
                }
            }
            Event::Start(Tag::Item) => {
                run.push((Span::raw("• "), None));
            }
            Event::End(TagEnd::Item) => {
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                flush(&mut out, &mut run, width, &indent);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut out, &mut run, width, "");
                quote_depth += 1;
                style = theme::quote();
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
                style = theme::link();
            }
            Event::End(TagEnd::Link) => {
                pending_link = None;
                style = Style::default();
            }
            Event::Code(text) => {
                // Inline code may itself be an intra-doc link target.
                let link = lookup(links, &text, pending_link.as_deref());
                let st = if link.is_some() {
                    theme::link()
                } else {
                    theme::inline_code()
                };
                run.push((Span::styled(text.to_string(), st), link));
            }
            Event::Text(text) => {
                if let Some(code) = in_code.as_mut() {
                    code.push_str(&text);
                } else {
                    let link = lookup(links, &text, pending_link.as_deref());
                    run.push((Span::styled(text.to_string(), style), link));
                }
            }
            Event::SoftBreak => run.push((Span::raw(" "), None)),
            Event::HardBreak => {
                let indent = "  ".repeat(list_depth);
                flush(&mut out, &mut run, width, &indent);
            }
            Event::Rule => {
                flush(&mut out, &mut run, width, "");
                out.lines
                    .push(Line::styled("─".repeat(width.min(60)), theme::rule()));
            }
            _ => {}
        }
    }
    flush(&mut out, &mut run, width, "");

    // Trailing blank lines add nothing but scrolling.
    while out.lines.last().is_some_and(|l| l.width() == 0) {
        out.lines.pop();
    }

    // A paragraph closes with a blank line and a heading opens with one, so
    // the two stack up wherever prose meets a heading. One blank separates
    // them just as well, and link targets are recorded against line indices,
    // so the collapse has to carry them along.
    collapse_blank_runs(&mut out);
    out
}

/// Squeeze runs of blank lines down to one, moving link targets to match.
fn collapse_blank_runs(out: &mut Rendered) {
    // New index of each old line, so link targets can be rewritten.
    let mut moved = Vec::with_capacity(out.lines.len());
    let mut kept: Vec<Line<'static>> = Vec::with_capacity(out.lines.len());
    let mut prev_blank = false;

    for line in std::mem::take(&mut out.lines) {
        let blank = line.width() == 0;
        if blank && prev_blank {
            // Point anything anchored here at the blank line that survived.
            moved.push(kept.len() - 1);
            continue;
        }
        prev_blank = blank;
        moved.push(kept.len());
        kept.push(line);
    }

    for link in &mut out.links {
        link.line = moved[link.line];
    }
    out.lines = kept;
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

/// Word-wrap the pending run to `width` beneath `indent`, emptying it.
///
/// Each span keeps its style, and every link records the spans it landed on.
/// Wrapping can split one link across lines, which yields one target per line
/// so the highlight follows the text exactly.
fn flush(out: &mut Rendered, run: &mut Run, width: usize, indent: &str) {
    let avail = width.saturating_sub(indent.len()).max(8);
    let mut current: Run = Vec::new();
    let mut used = 0usize;

    // Continuation lines line up under the first, past any bullet.
    let cont: String = " ".repeat(indent.len());
    let mut first = true;

    for (span, link) in run.drain(..) {
        let style = span.style;
        for word in split_keeping_spaces(&span.content) {
            let w = word.chars().count();
            // Never start a line with the space that followed a wrap.
            if used == 0 && word.trim().is_empty() {
                continue;
            }
            if used + w > avail && used > 0 {
                push_wrapped(out, if first { indent } else { &cont }, &mut current);
                first = false;
                used = 0;
                if word.trim().is_empty() {
                    continue;
                }
            }
            used += w;
            current.push((Span::styled(word, style), link));
        }
    }
    if !current.is_empty() {
        push_wrapped(out, if first { indent } else { &cont }, &mut current);
    }
}

/// Emit one wrapped line, recording a target for each link on it.
///
/// Consecutive spans naming the same item are one link: a link's text is
/// split into words for wrapping, and they must light up together.
fn push_wrapped(out: &mut Rendered, indent: &str, current: &mut Run) {
    let line = out.lines.len();
    let mut spans = Vec::with_capacity(current.len() + 1);
    if !indent.is_empty() {
        spans.push(Span::raw(indent.to_string()));
    }
    for (span, link) in current.drain(..) {
        let i = spans.len();
        if let Some(id) = link {
            match out.links.last_mut() {
                Some(t) if t.line == line && t.id == id && t.spans.end == i => t.spans.end += 1,
                _ => out.links.push(LinkTarget {
                    line,
                    spans: i..i + 1,
                    id,
                }),
            }
        }
        spans.push(span);
    }
    out.lines.push(Line::from(spans));
}

/// Dress one line of a fenced code block: a gutter rule, then the code, then
/// padding out to `width`.
///
/// The background alone would mark the block, but it is a single step off the
/// page colour and disappears entirely on terminals that override the
/// background, so the rule carries the same information in glyphs. Padding to
/// the full width is what makes the run of lines read as one slab instead of a
/// ragged stripe following the text.
fn code_line(line: Line<'static>, width: usize) -> Line<'static> {
    let bg = theme::code_block();
    let mut spans = vec![
        Span::styled("┃", theme::dim().patch(bg)),
        Span::styled(" ", bg),
    ];
    spans.extend(line.spans.into_iter().map(|s| {
        let style = s.style.patch(bg);
        s.style(style)
    }));
    // Stop the slab at a comfortable reading measure rather than running it to
    // the window edge: rustdoc examples are written to roughly this width, so
    // past it the background is empty anyway and only makes the block louder.
    // A line longer than the cap keeps its own length — cutting the background
    // mid-code would look like damage.
    pad_to(Line::from(spans), width.min(CODE_BLOCK_WIDTH), bg)
}

/// Pad a line with spaces in `style` out to `width` columns, so a background
/// reads as one block rather than stopping at the text's ragged end. A line
/// already that wide is left alone.
pub fn pad_to(mut line: Line<'static>, width: usize, style: Style) -> Line<'static> {
    let used = line.width();
    if used < width {
        line.push_span(Span::styled(" ".repeat(width - used), style));
    }
    line
}

/// Widest a code block's background is painted, gutter included.
const CODE_BLOCK_WIDTH: usize = 100;

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

/// Syntax-highlight a declaration, with none of the dressing a fenced block
/// inside prose gets.
///
/// A signature is a heading, not an example: it carries no gutter, no
/// background, no indent of its own, and no blank lines, so it sits as one
/// compact block wherever its caller puts it.
pub fn declaration(code: &str, hl: &Highlighter) -> Vec<Line<'static>> {
    let mut lines = hl.highlight(code);
    lines.retain(|l| l.width() > 0);
    lines
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

    /// The cases rustdoc's `LangString::parse` distinguishes: doctest
    /// attributes keep a block Rust, any unknown word makes it another
    /// language unless a Rust tag outweighs it, and order matters for the
    /// attributes that only count before an unknown word.
    #[test]
    fn fences_are_rust_the_way_rustdoc_decides() {
        for info in [
            "",
            "rust",
            "ignore (needs windows)",
            "no_run",
            "compile_fail,edition2021",
            "ignore-wasm32",
            "ignore,text",
            "text,rust",
            "rust {.example}",
        ] {
            assert!(is_rust(info), "{info:?} should be Rust");
        }
        for info in [
            "text",
            "toml",
            "console",
            "weird_attr",
            "text,ignore",
            "rust2021",
            "custom",
        ] {
            assert!(!is_rust(info), "{info:?} should not be Rust");
        }
    }

    #[test]
    fn fence_tokens_drop_comments_and_attribute_blocks() {
        assert_eq!(fence_tokens("ignore (needs windows)"), ["ignore"]);
        assert_eq!(
            fence_tokens("rust,no_run {.class} edition2021"),
            ["rust", "no_run", "edition2021"]
        );
        assert_eq!(fence_tokens(" , "), Vec::<&str>::new());
    }

    /// A `#` line is hidden only in Rust, where it is a doctest feature; in
    /// another language it is a comment and stays.
    #[test]
    fn hidden_lines_are_only_stripped_from_rust() {
        let hl = Highlighter::new();
        let render = |md: &str| -> Vec<String> {
            markdown(md, &HashMap::new(), 60, &hl)
                .lines
                .iter()
                .map(ToString::to_string)
                .collect()
        };
        let rust = render("```\n# use std::fmt;\nlet x = 1;\n```\n");
        assert!(!rust.iter().any(|l| l.contains("use std::fmt")), "{rust:?}");
        let sh = render("```sh\n# install it\ncargo add x\n```\n");
        assert!(sh.iter().any(|l| l.contains("# install it")), "{sh:?}");
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

    /// Text of the spans a link covers, which is what gets highlighted.
    fn link_text(r: &Rendered, i: usize) -> String {
        let t = &r.links[i];
        r.lines[t.line].spans[t.spans.clone()]
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    /// An intra-doc link resolves to its item and covers only its own text,
    /// not the prose around it.
    #[test]
    fn finds_intra_doc_links_and_their_extent() {
        let hl = Highlighter::new();
        let mut links = HashMap::new();
        links.insert("`None`".to_string(), Id(59));
        let r = markdown("See [`None`] for details.", &links, 60, &hl);

        assert_eq!(r.links.len(), 1);
        assert_eq!(r.links[0].id, Id(59));
        assert_eq!(link_text(&r, 0), "None");
    }

    #[test]
    fn multiple_links_get_separate_extents() {
        let hl = Highlighter::new();
        let mut links = HashMap::new();
        links.insert("`Some`".to_string(), Id(1));
        links.insert("`None`".to_string(), Id(2));
        let r = markdown("Either [`Some`] or [`None`] here.", &links, 60, &hl);

        assert_eq!(r.links.len(), 2);
        assert_eq!(link_text(&r, 0), "Some");
        assert_eq!(link_text(&r, 1), "None");
        assert_eq!(r.links[0].id, Id(1));
        assert_eq!(r.links[1].id, Id(2));
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

    /// Highlighting must be purely additive: styling a block may not add,
    /// drop, or reorder a single byte of the source.
    #[test]
    fn highlighting_preserves_the_source_text() {
        let hl = Highlighter::new();
        // A blank interior line is the case most easily lost when splitting
        // multi-line tokens.
        let code = "pub fn demo(x: &str) -> Option<u32> {\n    // a comment\n    let n = x.parse().ok()?;\n\n    Some(n + 1)\n}";

        let lines = hl.highlight(code);
        assert_eq!(lines.len(), 6, "expected one line per source line");

        let flat: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(flat.join("\n"), code);
    }

    #[test]
    fn rust_code_is_highlighted() {
        let hl = Highlighter::new();
        let lines = hl.highlight("let n = 1;");
        // `let` is a keyword and must be coloured differently from the rest.
        assert!(
            lines[0].spans.iter().any(|s| s.style.fg.is_some()),
            "no span carried a colour: {:?}",
            lines[0]
        );
    }

    /// A fence naming another language renders as plain text rather than
    /// being run through the Rust grammar.
    #[test]
    fn other_languages_render_unhighlighted() {
        let hl = Highlighter::new();
        let r = markdown(
            "```toml\n[package]\nname = \"x\"\n```\n",
            &HashMap::new(),
            60,
            &hl,
        );
        let code = r
            .lines
            .iter()
            .find(|l| l.to_string().contains("[package]"))
            .expect("no code line");
        // Gutter, its space, the text, and the padding: no per-token spans.
        assert_eq!(code.spans.len(), 4, "{code:?}");
    }

    #[test]
    fn empty_code_block_does_not_panic() {
        let hl = Highlighter::new();
        assert!(hl.highlight("").len() <= 1);
    }

    /// A fenced block is dressed as a slab: every line carries the code
    /// background, with a gutter rule marking its extent for terminals that
    /// drop the background.
    #[test]
    fn fenced_code_is_dressed_as_a_slab() {
        let hl = Highlighter::new();
        let r = markdown("text\n\n```\nlet n = 1;\n```\n", &HashMap::new(), 60, &hl);

        let bg = theme::code_block().bg.expect("code bg");
        let code = r
            .lines
            .iter()
            .find(|l| l.to_string().contains("let n = 1;"))
            .expect("no code line");
        assert!(code.to_string().starts_with("┃ "), "no gutter rule: {code}");
        assert!(
            code.spans.iter().all(|s| s.style.bg == Some(bg)),
            "a span in the code line missed the background"
        );

        // The prose above it is not code and keeps the page background.
        let prose = r
            .lines
            .iter()
            .find(|l| l.to_string() == "text")
            .expect("no prose line");
        assert!(prose.spans.iter().all(|s| s.style.bg.is_none()));
    }

    /// The slab is padded so a block reads as one piece rather than a ragged
    /// stripe, but stops at a reading measure, since past it there is only
    /// empty background. A line longer than that keeps its own length:
    /// truncating the background partway through the code would look like
    /// damage.
    #[test]
    fn code_slabs_are_padded_up_to_the_reading_measure() {
        let bg = theme::code_block().bg.expect("code bg");
        let width = |code: &str, page: usize| {
            let line = code_line(Line::raw(code.to_string()), page);
            assert!(line.spans.iter().all(|s| s.style.bg == Some(bg)));
            line.width()
        };
        let long = "y".repeat(CODE_BLOCK_WIDTH);

        assert_eq!(width("let n = 1;", 60), 60, "not padded to the page");
        assert_eq!(width("let n = 1;", CODE_BLOCK_WIDTH * 2), CODE_BLOCK_WIDTH);
        assert!(
            width(&long, CODE_BLOCK_WIDTH * 2) > CODE_BLOCK_WIDTH,
            "long code was cut"
        );
    }

    /// A paragraph ends with a blank line and a heading starts with one, so
    /// the two used to stack into a double gap wherever prose met a heading.
    #[test]
    fn blank_lines_do_not_stack_up() {
        let hl = Highlighter::new();
        let r = markdown(
            "Some prose.\n\n# Examples\n\nMore prose.\n",
            &HashMap::new(),
            60,
            &hl,
        );

        let blanks: Vec<usize> = r
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.width() == 0)
            .map(|(i, _)| i)
            .collect();
        for pair in blanks.windows(2) {
            assert!(
                pair[1] - pair[0] > 1,
                "consecutive blank lines at {pair:?} in {:?}",
                r.lines.iter().map(ToString::to_string).collect::<Vec<_>>()
            );
        }
    }

    /// Collapsing blank lines moves every line after them, so link targets
    /// have to be carried along or they point at the wrong row.
    #[test]
    fn collapsing_blanks_keeps_link_targets_on_their_text() {
        let hl = Highlighter::new();
        let mut links = HashMap::new();
        links.insert("`None`".to_string(), Id(59));
        let r = markdown(
            "Intro.\n\n# Heading\n\nSee [`None`] here.\n",
            &links,
            60,
            &hl,
        );

        assert_eq!(r.links.len(), 1);
        assert_eq!(link_text(&r, 0), "None");
    }
}
