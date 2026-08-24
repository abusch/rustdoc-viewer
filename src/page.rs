//! Assembling a full item page: declaration, docs, fields, and impl sections.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use rustdoc_types::{Id, Impl, Item, ItemEnum, ItemKind, StructKind, VariantKind};

use crate::docs::{ItemRef, Universe};
use crate::format;
use crate::render::{self, Highlighter};

/// Which group an impl belongs to, mirroring the docs.rs layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplGroup {
    Inherent,
    Trait,
    Auto,
    Blanket,
}

impl ImplGroup {
    pub fn title(self) -> &'static str {
        match self {
            ImplGroup::Inherent => "Implementations",
            ImplGroup::Trait => "Trait Implementations",
            ImplGroup::Auto => "Auto Trait Implementations",
            ImplGroup::Blanket => "Blanket Implementations",
        }
    }

    /// Inherent methods are what people came for; the rest start folded.
    pub fn starts_expanded(self) -> bool {
        self == ImplGroup::Inherent
    }

    pub fn all() -> [ImplGroup; 4] {
        [Self::Inherent, Self::Trait, Self::Auto, Self::Blanket]
    }
}

fn classify(im: &Impl) -> ImplGroup {
    if im.blanket_impl.is_some() {
        ImplGroup::Blanket
    } else if im.is_synthetic {
        ImplGroup::Auto
    } else if im.trait_.is_some() {
        ImplGroup::Trait
    } else {
        ImplGroup::Inherent
    }
}

/// A navigable target on the page: an item you can press Enter on.
#[derive(Debug, Clone)]
pub struct Target {
    pub line: usize,
    pub item: ItemRef,
}

/// A collapsible section of impls.
pub struct Section {
    pub group: ImplGroup,
    pub impls: Vec<ItemRef>,
}

/// A fully rendered page, ready to be scrolled through.
pub struct Page {
    pub title: String,
    pub lines: Vec<Line<'static>>,
    /// Line index of each section header, parallel to `sections`.
    pub section_lines: Vec<usize>,
    pub sections: Vec<ImplGroup>,
    pub targets: Vec<Target>,
    pub width: u16,
}

const TITLE: Style = Style::new().fg(Color::LightMagenta).add_modifier(Modifier::BOLD);
const SECTION: Style = Style::new().fg(Color::LightBlue).add_modifier(Modifier::BOLD);
const SIG: Style = Style::new().fg(Color::White);
const DIM: Style = Style::new().fg(Color::DarkGray);
const WARN: Style = Style::new().fg(Color::LightRed);

/// Collect and group the impls attached to an item.
pub fn impls_of(u: &Universe, r: ItemRef) -> Vec<Section> {
    let Some(item) = u.item(r) else { return Vec::new() };
    let ids: &[Id] = match &item.inner {
        ItemEnum::Struct(s) => &s.impls,
        ItemEnum::Enum(e) => &e.impls,
        ItemEnum::Union(un) => &un.impls,
        ItemEnum::Primitive(p) => &p.impls,
        ItemEnum::Trait(t) => &t.implementations,
        _ => return Vec::new(),
    };

    let mut sections: Vec<Section> = ImplGroup::all()
        .into_iter()
        .map(|group| Section { group, impls: Vec::new() })
        .collect();

    for id in ids {
        // Impls always live in the same crate as the type they belong to.
        let iref = ItemRef::new(r.krate, *id);
        let Some(ii) = u.item(iref) else { continue };
        let ItemEnum::Impl(im) = &ii.inner else { continue };
        let g = classify(im);
        if let Some(s) = sections.iter_mut().find(|s| s.group == g) {
            s.impls.push(iref);
        }
    }

    // Sort each group by its rendered header so the lists read alphabetically.
    for s in &mut sections {
        s.impls.sort_by_cached_key(|r| {
            u.item(*r)
                .and_then(|i| match &i.inner {
                    ItemEnum::Impl(im) => Some(format::impl_header(im)),
                    _ => None,
                })
                .unwrap_or_default()
        });
    }
    sections.retain(|s| !s.impls.is_empty());
    sections
}

/// Build the page for an item at a given width.
pub fn build(
    u: &Universe,
    r: ItemRef,
    width: u16,
    hl: &Highlighter,
    expanded: &[ImplGroup],
) -> Page {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut targets: Vec<Target> = Vec::new();
    let mut section_lines = Vec::new();
    let mut sections = Vec::new();

    let Some(item) = u.item(r) else {
        return Page {
            title: "<missing>".into(),
            lines: vec![Line::styled("item not found", WARN)],
            section_lines,
            sections,
            targets,
            width,
        };
    };

    let kind = u.kind_of(r);
    let path = u
        .path_of(r)
        .unwrap_or_else(|| item.name.clone().unwrap_or_else(|| "?".into()));

    // --- Header -----------------------------------------------------------
    let kind_label = kind.as_ref().map(kind_name).unwrap_or_else(|| inner_name(&item.inner));
    lines.push(Line::from(vec![
        Span::styled(format!("{kind_label} "), DIM),
        Span::styled(path.clone(), TITLE),
    ]));

    if let Some(dep) = &item.deprecation {
        let mut s = String::from("Deprecated");
        if let Some(since) = &dep.since {
            s.push_str(&format!(" since {since}"));
        }
        if let Some(note) = &dep.note {
            s.push_str(&format!(": {note}"));
        }
        lines.push(Line::styled(s, WARN));
    }
    if let Some(stab) = &item.stability
        && matches!(stab.level, rustdoc_types::StabilityLevel::Unstable)
    {
        lines.push(Line::styled(
            format!("Unstable — feature = \"{}\"", stab.feature),
            Style::new().fg(Color::Yellow),
        ));
    }
    lines.push(Line::default());

    // --- Declaration ------------------------------------------------------
    if let Some(sig) = format::signature(item) {
        for l in hl_code(&sig, hl) {
            lines.push(l);
        }
        lines.push(Line::default());
    }

    // --- Docs -------------------------------------------------------------
    let rendered = render::item_docs(item, width, hl);
    let doc_start = lines.len();
    for link in &rendered.links {
        if let Some(target) = u.resolve(r.krate, link.id) {
            targets.push(Target { line: doc_start + link.line, item: target });
        }
    }
    lines.extend(rendered.lines);
    lines.push(Line::default());

    // --- Fields / Variants ------------------------------------------------
    match &item.inner {
        ItemEnum::Struct(s) => {
            if let StructKind::Plain { fields, .. } = &s.kind {
                push_members(u, r, "Fields", fields, &mut lines, &mut targets, width, hl);
            }
        }
        ItemEnum::Enum(e) => {
            push_variants(u, r, &e.variants, &mut lines, &mut targets, width, hl);
        }
        ItemEnum::Trait(t) => {
            push_members(
                u, r, "Required Methods", &t.items, &mut lines, &mut targets, width, hl,
            );
        }
        _ => {}
    }

    // --- Impl sections ----------------------------------------------------
    for section in impls_of(u, r) {
        let is_open = expanded.contains(&section.group);
        section_lines.push(lines.len());
        sections.push(section.group);

        let marker = if is_open { "▾" } else { "▸" };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} {} ", section.group.title()), SECTION),
            Span::styled(format!("({})", section.impls.len()), DIM),
        ]));

        if !is_open {
            lines.push(Line::default());
            continue;
        }

        for iref in &section.impls {
            let Some(ii) = u.item(*iref) else { continue };
            let ItemEnum::Impl(im) = &ii.inner else { continue };

            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format::impl_header(im), SIG.add_modifier(Modifier::BOLD)),
            ]));

            // Auto and blanket impls have no methods worth listing.
            if matches!(section.group, ImplGroup::Auto | ImplGroup::Blanket) {
                continue;
            }
            for mid in &im.items {
                let mref = ItemRef::new(iref.krate, *mid);
                let Some(m) = u.item(mref) else { continue };
                let sig = format::signature(m)
                    .unwrap_or_else(|| m.name.clone().unwrap_or_default());
                targets.push(Target { line: lines.len(), item: mref });
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(sig, SIG),
                ]));
                if let Some(first) = summary_line(m) {
                    lines.push(Line::from(vec![
                        Span::raw("      "),
                        Span::styled(first, DIM),
                    ]));
                }
            }
            lines.push(Line::default());
        }
    }

    Page {
        title: path,
        lines,
        section_lines,
        sections,
        targets,
        width,
    }
}

/// Render a declaration through the syntax highlighter.
fn hl_code(code: &str, hl: &Highlighter) -> Vec<Line<'static>> {
    let empty = std::collections::HashMap::new();
    // Route through the markdown renderer so highlighting stays in one place.
    let md = format!("```rust\n{code}\n```");
    let mut r = render::markdown(&md, &empty, u16::MAX, hl);
    r.lines.retain(|l| l.width() > 0);
    r.lines
}

/// The first sentence of an item's docs, for use as a one-line summary.
fn summary_line(item: &Item) -> Option<String> {
    let docs = item.docs.as_ref()?;
    let first = docs.lines().find(|l| !l.trim().is_empty())?.trim();
    if first.is_empty() {
        return None;
    }
    // Summaries bypass the markdown renderer, so undo the link syntax by hand
    // rather than showing rustdoc's `[`Foo`]` brackets verbatim.
    let cleaned = strip_link_brackets(first);
    let truncated: String = cleaned.chars().take(100).collect();
    Some(truncated)
}


/// Remove markdown link brackets from a plain-text summary.
///
/// Rustdoc writes intra-doc links as `[`Foo`]` or `[text](dest)`; a one-line
/// summary has no link handling, so the brackets are just noise.
fn strip_link_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' => {}
            ']' => {
                // Drop a trailing `(...)` or `[...]` reference, keeping the text.
                match chars.peek() {
                    Some('(') => {
                        for c in chars.by_ref() {
                            if c == ')' {
                                break;
                            }
                        }
                    }
                    Some('[') => {
                        for c in chars.by_ref() {
                            if c == ']' {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn push_members(
    u: &Universe,
    parent: ItemRef,
    title: &str,
    ids: &[Id],
    lines: &mut Vec<Line<'static>>,
    targets: &mut Vec<Target>,
    width: u16,
    hl: &Highlighter,
) {
    let members: Vec<ItemRef> = ids
        .iter()
        .map(|id| ItemRef::new(parent.krate, *id))
        .filter(|m| u.item(*m).is_some())
        .collect();
    if members.is_empty() {
        return;
    }

    lines.push(Line::styled(format!("  {title}"), SECTION));
    for m in members {
        let Some(item) = u.item(m) else { continue };
        let sig = format::signature(item).unwrap_or_else(|| item.name.clone().unwrap_or_default());
        targets.push(Target { line: lines.len(), item: m });
        lines.push(Line::from(vec![Span::raw("    "), Span::styled(sig, SIG)]));
        push_indented_docs(item, lines, width, hl, 6);
    }
    lines.push(Line::default());
}

fn push_variants(
    u: &Universe,
    parent: ItemRef,
    ids: &[Id],
    lines: &mut Vec<Line<'static>>,
    targets: &mut Vec<Target>,
    width: u16,
    hl: &Highlighter,
) {
    if ids.is_empty() {
        return;
    }
    lines.push(Line::styled("  Variants".to_string(), SECTION));
    for id in ids {
        let vref = ItemRef::new(parent.krate, *id);
        let Some(item) = u.item(vref) else { continue };
        let ItemEnum::Variant(v) = &item.inner else { continue };
        let name = item.name.clone().unwrap_or_default();
        let rendered = match &v.kind {
            VariantKind::Plain => name.clone(),
            VariantKind::Tuple(fields) => {
                let tys: Vec<String> = fields
                    .iter()
                    .map(|f| {
                        f.and_then(|fid| u.item(ItemRef::new(parent.krate, fid)))
                            .and_then(|fi| match &fi.inner {
                                ItemEnum::StructField(t) => Some(format::ty(t)),
                                _ => None,
                            })
                            .unwrap_or_else(|| "_".into())
                    })
                    .collect();
                format!("{name}({})", tys.join(", "))
            }
            VariantKind::Struct { fields, .. } => {
                let fs: Vec<String> = fields
                    .iter()
                    .filter_map(|fid| u.item(ItemRef::new(parent.krate, *fid)))
                    .filter_map(|fi| match &fi.inner {
                        ItemEnum::StructField(t) => {
                            Some(format!("{}: {}", fi.name.clone()?, format::ty(t)))
                        }
                        _ => None,
                    })
                    .collect();
                format!("{name} {{ {} }}", fs.join(", "))
            }
        };
        targets.push(Target { line: lines.len(), item: vref });
        lines.push(Line::from(vec![Span::raw("    "), Span::styled(rendered, SIG)]));
        push_indented_docs(item, lines, width, hl, 6);
    }
    lines.push(Line::default());
}

fn push_indented_docs(
    item: &Item,
    lines: &mut Vec<Line<'static>>,
    width: u16,
    hl: &Highlighter,
    indent: usize,
) {
    let r = render::item_docs(item, width.saturating_sub(indent as u16), hl);
    let pad = " ".repeat(indent);
    for line in r.lines {
        if line.width() == 0 {
            continue;
        }
        let mut spans = vec![Span::styled(pad.clone(), DIM)];
        spans.extend(line.spans.into_iter().map(|s| s.patch_style(DIM)));
        lines.push(Line::from(spans));
    }
}

fn kind_name(k: &ItemKind) -> &'static str {
    match k {
        ItemKind::Module => "Module",
        ItemKind::Struct => "Struct",
        ItemKind::StructField => "Field",
        ItemKind::Union => "Union",
        ItemKind::Enum => "Enum",
        ItemKind::Variant => "Variant",
        ItemKind::Function => "Function",
        ItemKind::Trait => "Trait",
        ItemKind::TraitAlias => "Trait Alias",
        ItemKind::Impl => "Impl",
        ItemKind::TypeAlias => "Type Alias",
        ItemKind::Constant => "Constant",
        ItemKind::Static => "Static",
        ItemKind::Macro => "Macro",
        ItemKind::ProcAttribute => "Attribute Macro",
        ItemKind::ProcDerive => "Derive Macro",
        ItemKind::AssocConst => "Assoc Const",
        ItemKind::AssocType => "Assoc Type",
        ItemKind::Primitive => "Primitive",
        ItemKind::Keyword => "Keyword",
        ItemKind::ExternCrate => "Extern Crate",
        ItemKind::Use => "Use",
        ItemKind::ExternType => "Extern Type",
        ItemKind::Attribute => "Attribute",
    }
}

fn inner_name(inner: &ItemEnum) -> &'static str {
    match inner {
        ItemEnum::Module(_) => "Module",
        ItemEnum::Struct(_) => "Struct",
        ItemEnum::Enum(_) => "Enum",
        ItemEnum::Function(_) => "Function",
        ItemEnum::Trait(_) => "Trait",
        ItemEnum::Impl(_) => "Impl",
        ItemEnum::TypeAlias(_) => "Type Alias",
        ItemEnum::Constant { .. } => "Constant",
        ItemEnum::Static(_) => "Static",
        ItemEnum::Macro(_) => "Macro",
        ItemEnum::Primitive(_) => "Primitive",
        ItemEnum::Variant(_) => "Variant",
        ItemEnum::StructField(_) => "Field",
        ItemEnum::AssocConst { .. } => "Assoc Const",
        ItemEnum::AssocType { .. } => "Assoc Type",
        _ => "Item",
    }
}

/// The default set of expanded sections for a fresh page.
pub fn default_expanded() -> Vec<ImplGroup> {
    ImplGroup::all()
        .into_iter()
        .filter(|g| g.starts_expanded())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load;

    fn universe() -> Option<Universe> {
        let crates = load::load_std_crates().ok()?;
        let names = load::STD_CRATES.iter().map(|s| s.to_string()).collect();
        Some(Universe::new(crates, names))
    }

    #[test]
    fn summaries_drop_link_brackets() {
        assert_eq!(strip_link_brackets("Returns [`None`] if empty"), "Returns `None` if empty");
        assert_eq!(strip_link_brackets("See [text](std::foo) here"), "See text here");
        assert_eq!(strip_link_brackets("See [text][key] here"), "See text here");
        assert_eq!(strip_link_brackets("no links here"), "no links here");
    }

    #[test]
    fn groups_vec_impls_into_four_sections() {
        let Some(u) = universe() else {
            eprintln!("skipping: rust-docs-json not available");
            return;
        };
        let r = u.by_path("alloc::vec::Vec").expect("Vec");
        let sections = impls_of(&u, r);

        // Vec has inherent, trait, auto and blanket impls; all four must appear
        // rather than one flat list.
        let groups: Vec<ImplGroup> = sections.iter().map(|s| s.group).collect();
        for g in ImplGroup::all() {
            assert!(groups.contains(&g), "missing group {g:?}");
        }
        let total: usize = sections.iter().map(|s| s.impls.len()).sum();
        assert!(total > 100, "expected Vec's ~110 impls, got {total}");
    }

    #[test]
    fn builds_a_page_for_string() {
        let Some(u) = universe() else { return };
        let hl = Highlighter::new();
        let r = u.by_path("alloc::string::String").expect("String");
        let page = build(&u, r, 80, &hl, &default_expanded());

        assert!(page.title.ends_with("string::String"));
        assert!(page.lines.len() > 20, "page looks empty");
        assert!(!page.sections.is_empty(), "no impl sections");
        // Inherent methods are targets you can navigate to.
        assert!(!page.targets.is_empty(), "no navigable targets");
    }

    #[test]
    fn option_variants_render() {
        let Some(u) = universe() else { return };
        let hl = Highlighter::new();
        let r = u.by_path("core::option::Option").expect("Option");
        let page = build(&u, r, 80, &hl, &default_expanded());
        let text: String = page
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Variants"), "no Variants section");
        assert!(text.contains("None"), "missing None variant");
        assert!(text.contains("Some"), "missing Some variant");
    }
}
