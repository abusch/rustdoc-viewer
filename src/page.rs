//! Assembling a full item page: declaration, docs, fields, and impl sections.

use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use rustdoc_types::{Id, Impl, Item, ItemEnum, ItemKind, StructKind, VariantKind, Visibility};

use crate::docs::{ItemRef, Universe};
use crate::format;
use crate::kind;
use crate::render::{self, Highlighter};
use crate::theme;

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

/// A foldable section on a page.
///
/// Module listings (`Structs`, `Functions`, ...) fold and step under `n`/`p`
/// just like impl sections do, but they are not impls and there is a
/// module-dependent set of them, so they are named by their heading. The
/// headings come from [`MODULE_KINDS`], so the `&'static str` is a closed set
/// rather than arbitrary text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionId {
    Impls(ImplGroup),
    Module(&'static str),
}

impl SectionId {
    pub fn title(self) -> &'static str {
        match self {
            SectionId::Impls(g) => g.title(),
            SectionId::Module(title) => title,
        }
    }

    /// Module listings are the whole point of a module page, so they start
    /// open; impl sections keep their own rule.
    fn starts_expanded(self) -> bool {
        match self {
            SectionId::Impls(g) => g.starts_expanded(),
            SectionId::Module(_) => true,
        }
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
    /// Span range to highlight when focused. `None` highlights the whole line,
    /// which is what a method or field row wants; an intra-doc link sets it so
    /// only the link text lights up.
    pub spans: Option<std::ops::Range<usize>>,
    pub item: ItemRef,
}

impl Target {
    /// A target occupying a whole line, such as a method or field row.
    fn line(line: usize, item: ItemRef) -> Self {
        Self {
            line,
            spans: None,
            item,
        }
    }
}

/// The impls of one group, in the order the page lists them.
pub struct ImplSection {
    pub group: ImplGroup,
    pub impls: Vec<ItemRef>,
}

/// A foldable section as laid out on a page: which one, and the line its
/// heading landed on, which is where `n`/`p` scroll to.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub id: SectionId,
    pub line: usize,
}

/// A fully rendered page, ready to be scrolled through.
pub struct Page {
    pub title: String,
    /// The item's kind ("Enum", "Function"), shown beside the title in the
    /// header line the item view draws above the scrolling body.
    pub kind: String,
    pub lines: Vec<Line<'static>>,
    pub sections: Vec<Section>,
    pub targets: Vec<Target>,
    pub width: u16,
}

/// The indent ladder every page follows.
///
/// Each rung is strictly deeper than the one above it, so an item's
/// documentation always reads as hanging off the item rather than as prose
/// that happens to follow it. Impl headers sit a rung above the members they
/// introduce, which puts a trait's methods at the same depth as an inherent
/// one's.
mod indent {
    /// A section heading (`Variants`, `impl Display for T`).
    pub const HEADING: usize = 2;
    /// A member's own line: a signature, a field, a variant.
    pub const MEMBER: usize = 4;
    /// The documentation hanging beneath a member.
    pub const DOCS: usize = 6;
}

/// Module children are listed in this order, matching the docs.rs layout.
///
/// A kind missing from here is silently left out of every module page, which
/// is how `MaybeUninit` (a union) and `core`'s built-in derives once went
/// unlisted.
const MODULE_KINDS: [(ItemKind, &str); 13] = [
    (ItemKind::Module, "Modules"),
    (ItemKind::Macro, "Macros"),
    (ItemKind::Struct, "Structs"),
    (ItemKind::Enum, "Enums"),
    (ItemKind::Union, "Unions"),
    (ItemKind::Trait, "Traits"),
    (ItemKind::Function, "Functions"),
    (ItemKind::TypeAlias, "Type Aliases"),
    (ItemKind::Constant, "Constants"),
    (ItemKind::Static, "Statics"),
    (ItemKind::Primitive, "Primitives"),
    (ItemKind::ProcAttribute, "Attribute Macros"),
    (ItemKind::ProcDerive, "Derive Macros"),
];

/// Collect and group the impls attached to an item.
pub fn impls_of(u: &Universe, r: ItemRef) -> Vec<ImplSection> {
    let Some(item) = u.item(r) else {
        return Vec::new();
    };
    let ids: &[Id] = match &item.inner {
        ItemEnum::Struct(s) => &s.impls,
        ItemEnum::Enum(e) => &e.impls,
        ItemEnum::Union(un) => &un.impls,
        ItemEnum::Primitive(p) => &p.impls,
        ItemEnum::Trait(t) => &t.implementations,
        _ => return Vec::new(),
    };

    let mut sections: Vec<ImplSection> = ImplGroup::all()
        .into_iter()
        .map(|group| ImplSection {
            group,
            impls: Vec::new(),
        })
        .collect();

    for id in ids {
        // Impls always live in the same crate as the type they belong to.
        let iref = ItemRef::new(r.krate, *id);
        let Some(ii) = u.item(iref) else { continue };
        let ItemEnum::Impl(im) = &ii.inner else {
            continue;
        };
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

/// The context and accumulating output shared by the page-building helpers.
///
/// `u`, `width`, `hl`, and `toggled` are read-only context; `lines`,
/// `targets`, and `sections` are the page being accumulated. Bundling them
/// keeps the helpers from threading the same parameters through every call.
struct Builder<'a> {
    u: &'a Universe,
    width: u16,
    hl: &'a Highlighter,
    toggled: &'a [SectionId],
    lines: Vec<Line<'static>>,
    targets: Vec<Target>,
    sections: Vec<Section>,
}

impl<'a> Builder<'a> {
    fn new(u: &'a Universe, width: u16, hl: &'a Highlighter, toggled: &'a [SectionId]) -> Self {
        Self {
            u,
            width,
            hl,
            toggled,
            lines: Vec::new(),
            targets: Vec::new(),
            sections: Vec::new(),
        }
    }

    fn push(&mut self, line: Line<'static>) {
        self.lines.push(line);
    }

    fn blank(&mut self) {
        self.lines.push(Line::default());
    }
}

/// Build the page for an item at a given width.
///
/// `toggled` lists the sections flipped away from their default; see
/// [`is_expanded`].
pub fn build(
    u: &Universe,
    r: ItemRef,
    width: u16,
    hl: &Highlighter,
    toggled: &[SectionId],
) -> Page {
    let Some(item) = u.item(r) else {
        return Page {
            title: "<missing>".into(),
            kind: String::new(),
            lines: vec![Line::styled("item not found", theme::deprecated())],
            sections: Vec::new(),
            targets: Vec::new(),
            width,
        };
    };
    let mut b = Builder::new(u, width, hl, toggled);

    let path = u
        .path_of(r)
        .unwrap_or_else(|| item.name.clone().unwrap_or_else(|| "?".into()));

    // --- Header -----------------------------------------------------------
    // The kind and path identify the page rather than belonging to its body,
    // so they are handed to the item view as a fixed header line instead of
    // scrolling away with the first screenful.
    let kind_label = kind::label(item.inner.item_kind());

    if let Some(dep) = &item.deprecation {
        let mut s = String::from("Deprecated");
        if let Some(since) = &dep.since {
            s.push_str(&format!(" since {since}"));
        }
        if let Some(note) = &dep.note {
            s.push_str(&format!(": {note}"));
        }
        b.push(Line::styled(s, theme::deprecated()));
    }
    if let Some(stab) = &item.stability
        && matches!(stab.level, rustdoc_types::StabilityLevel::Unstable)
    {
        b.push(Line::styled(
            format!("Unstable — feature = \"{}\"", stab.feature),
            theme::unstable(),
        ));
    }
    // Only separate the notices from the declaration when there were any: with
    // the title line gone the body would otherwise open on a blank row.
    if !b.lines.is_empty() {
        b.blank();
    }

    // --- Declaration ------------------------------------------------------
    if let Some(sig) = format::signature(item) {
        for l in hl_code(&sig, hl) {
            b.push(indent_line(indent::HEADING, l));
        }
        b.blank();
    }

    // --- Docs -------------------------------------------------------------
    // Indented to the same rung as a member's docs, so the page's description
    // hangs off its declaration the way a method's hangs off its signature.
    b.push_docs_styled(r, item, indent::HEADING, false);
    b.blank();

    // --- Fields / Variants ------------------------------------------------
    match &item.inner {
        ItemEnum::Struct(s) => {
            if let StructKind::Plain { fields, .. } = &s.kind {
                b.push_members(r, "Fields", fields, member_decl);
            }
        }
        ItemEnum::Enum(e) => {
            b.push_members(r, "Variants", &e.variants, |v| variant_decl(u, r, v));
        }
        ItemEnum::Trait(t) => {
            b.push_members(r, "Required Methods", &t.items, member_decl);
        }
        ItemEnum::Module(m) => {
            b.push_module_contents(r, &m.items);
        }
        _ => {}
    }

    // --- Impl sections ----------------------------------------------------
    for section in impls_of(u, r) {
        if !b.open_section(SectionId::Impls(section.group), section.impls.len()) {
            continue;
        }

        for iref in &section.impls {
            let Some(ii) = u.item(*iref) else { continue };
            let ItemEnum::Impl(im) = &ii.inner else {
                continue;
            };

            // The header is highlighted like the signatures it introduces, and
            // stays bold so it still reads as their heading.
            for l in hl_code(&format::impl_header(im), hl) {
                b.push(indent_line(indent::HEADING, bold(l)));
            }

            // Auto and blanket impls have no methods worth listing.
            if matches!(section.group, ImplGroup::Auto | ImplGroup::Blanket) {
                continue;
            }
            for mid in &im.items {
                let mref = ItemRef::new(iref.krate, *mid);
                let Some(m) = u.item(mref) else { continue };
                b.push_member(mref, m, &member_decl(m));
                b.blank();
            }
        }
    }

    Page {
        title: path,
        kind: kind_label.to_string(),
        lines: b.lines,
        sections: b.sections,
        targets: b.targets,
        width,
    }
}

impl Builder<'_> {
    /// Render an item's markdown docs indented beneath its signature.
    ///
    /// Method, field, and variant docs go through the same pipeline as the
    /// page's own docs, so prose wraps, examples keep their syntax
    /// highlighting, and intra-doc links stay navigable.
    fn push_docs(&mut self, r: ItemRef, item: &Item, indent: usize) {
        self.push_docs_styled(r, item, indent, true);
    }

    /// As [`Self::push_docs`], but `tint` decides whether the prose carries the
    /// member-documentation background.
    ///
    /// The page's own description passes `false`: the tint exists to separate
    /// one member's docs from the next, and the item's own docs have nothing
    /// above or below them to be separated from.
    fn push_docs_styled(&mut self, r: ItemRef, item: &Item, indent: usize, tint: bool) {
        let rendered = render::item_docs(item, self.width.saturating_sub(indent as u16), self.hl);
        let start = self.lines.len();
        for link in &rendered.links {
            if let Some(target) = self.u.resolve(r.krate, link.id) {
                self.targets.push(Target {
                    line: start + link.line,
                    // The indent prepends one span, shifting the link's extent.
                    spans: Some(link.spans.start + 1..link.spans.end + 1),
                    item: target,
                });
            }
        }
        let width = self.width;
        for l in rendered.lines {
            let l = indent_line(indent, l);
            self.lines.push(if tint { tinted(l, width) } else { l });
        }
    }

    /// A fixed heading followed by one row per member, each with its docs.
    ///
    /// Fields, variants, and required methods are all listed this way; only
    /// how a member is declared differs, which `decl` supplies.
    fn push_members(
        &mut self,
        parent: ItemRef,
        title: &str,
        ids: &[Id],
        decl: impl Fn(&Item) -> String,
    ) {
        let u = self.u;
        let members: Vec<(ItemRef, &Item)> = ids
            .iter()
            .map(|id| ItemRef::new(parent.krate, *id))
            .filter_map(|m| Some((m, u.item(m)?)))
            .collect();
        if members.is_empty() {
            return;
        }

        self.push(Line::styled(
            format!("{:indent$}{title}", "", indent = indent::HEADING),
            theme::section(),
        ));
        for (m, item) in members {
            self.push_member(m, item, &decl(item));
        }
        self.blank();
    }

    /// One member row: its declaration, highlighted like the page's own, then
    /// its docs hanging beneath. The row is the member's target, so opening a
    /// method focuses this line.
    fn push_member(&mut self, m: ItemRef, item: &Item, decl: &str) {
        self.targets.push(Target::line(self.lines.len(), m));
        for l in hl_code(decl, self.hl) {
            self.push(indent_line(indent::MEMBER, l));
        }
        self.push_docs(m, item, indent::DOCS);
    }

    /// Register a foldable section and draw its heading: a marker, the title,
    /// and how many members it holds. Returns whether the section is open,
    /// i.e. whether its body should follow.
    fn open_section(&mut self, id: SectionId, count: usize) -> bool {
        let is_open = is_expanded(id, self.toggled);
        self.sections.push(Section {
            id,
            line: self.lines.len(),
        });
        let marker = if is_open { "▾" } else { "▸" };
        self.push(Line::from(vec![
            Span::styled(format!("{marker} {} ", id.title()), theme::section()),
            Span::styled(format!("({count})"), theme::dim()),
        ]));
        if !is_open {
            self.blank();
        }
        is_open
    }

    /// List a module's children, grouped by kind, one line each.
    ///
    /// A module has far too many children to show full docs for, so each gets
    /// its name and a one-line summary, the way the docs.rs module index does.
    /// Re-exports are resolved to what they point at, since a `use` item has
    /// no docs of its own and is not worth navigating to.
    fn push_module_contents(&mut self, parent: ItemRef, ids: &[Id]) {
        // Group by kind, keeping each group in the order rustdoc listed it.
        let mut groups: Vec<(&'static str, Vec<ItemRef>)> = MODULE_KINDS
            .iter()
            .map(|(_, title)| (*title, Vec::new()))
            .collect();

        for id in ids {
            let child = ItemRef::new(parent.krate, *id);
            let Some(item) = self.u.item(child) else {
                continue;
            };
            // A `use` is a pointer; show the item it names instead.
            let target = match &item.inner {
                ItemEnum::Use(u) => match u.id.and_then(|id| self.u.resolve(parent.krate, id)) {
                    Some(t) => t,
                    None => continue,
                },
                _ => child,
            };
            let Some(resolved) = self.u.item(target) else {
                continue;
            };
            if resolved.name.is_none() || !is_public(resolved) {
                continue;
            }
            let slot = MODULE_KINDS
                .iter()
                .position(|(k, _)| *k == resolved.inner.item_kind());
            if let Some(i) = slot
                && !groups[i].1.iter().any(|e| {
                    self.u.item(*e).and_then(|i| i.name.as_ref()) == resolved.name.as_ref()
                })
            {
                // Re-exports can bring the same name in twice — `std::task`
                // would otherwise list its `task` module twice; keep the first.
                groups[i].1.push(target);
            }
        }

        for (title, mut members) in groups {
            if members.is_empty() {
                continue;
            }
            // rustdoc lists children in source order; alphabetical scans better.
            members.sort_by_cached_key(|m| {
                self.u
                    .item(*m)
                    .and_then(|i| i.name.clone())
                    .unwrap_or_default()
            });

            if !self.open_section(SectionId::Module(title), members.len()) {
                continue;
            }

            for m in members {
                let Some(item) = self.u.item(m) else { continue };
                let name = item.name.clone().unwrap_or_default();
                self.targets.push(Target::line(self.lines.len(), m));
                let mut spans = vec![
                    Span::raw(" ".repeat(indent::MEMBER)),
                    Span::styled(name, theme::signature()),
                ];
                if let Some(summary) = summary_line(item) {
                    spans.push(Span::styled(format!("  {summary}"), theme::dim()));
                }
                self.push(Line::from(spans));
            }
            self.blank();
        }
    }
}

/// How a member is declared on its row: its signature, or failing that its name.
fn member_decl(item: &Item) -> String {
    format::signature(item).unwrap_or_else(|| item.name.clone().unwrap_or_default())
}

/// How an enum variant is declared, with its fields' types spelled out.
///
/// The variant only holds field ids, so this needs the universe to look the
/// fields up, which is why it is not one of [`format::signature`]'s arms.
fn variant_decl(u: &Universe, parent: ItemRef, item: &Item) -> String {
    let name = item.name.clone().unwrap_or_default();
    let ItemEnum::Variant(v) = &item.inner else {
        return name;
    };
    let field = |id: Id| u.item(ItemRef::new(parent.krate, id));
    match &v.kind {
        VariantKind::Plain => name,
        VariantKind::Tuple(fields) => {
            let tys: Vec<String> = fields
                .iter()
                .map(|f| {
                    f.and_then(field)
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
                .filter_map(|fid| field(*fid))
                .filter_map(|fi| match &fi.inner {
                    ItemEnum::StructField(t) => {
                        Some(format!("{}: {}", fi.name.clone()?, format::ty(t)))
                    }
                    _ => None,
                })
                .collect();
            format!("{name} {{ {} }}", fs.join(", "))
        }
    }
}

/// Embolden every span of a line, in place of a line-level style.
///
/// The spans carry their own colors from the highlighter, and styling each one
/// keeps the emphasis with them rather than depending on a line style showing
/// through underneath.
fn bold(line: Line<'static>) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(|s| {
                let style = s.style.add_modifier(Modifier::BOLD);
                s.style(style)
            })
            .collect::<Vec<_>>(),
    )
}

/// Lay a member's documentation background across the full width of a line.
///
/// Padding to `width` is what separates one method's description from the
/// next: without it the tint would only follow the text and stop at whatever
/// ragged column the wrap landed on. Spans that already carry a background of
/// their own — a code block's slab, an inline code span — keep it, so the
/// tint fills the gaps around them rather than flattening them out.
fn tinted(line: Line<'static>, width: u16) -> Line<'static> {
    let bg = theme::member_docs();
    let mut used = 0usize;
    let mut spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| {
            used += s.content.chars().count();
            let style = if s.style.bg.is_some() {
                s.style
            } else {
                s.style.patch(bg)
            };
            s.style(style)
        })
        .collect();
    if used < width as usize {
        spans.push(Span::styled(" ".repeat(width as usize - used), bg));
    }
    Line::from(spans)
}

/// Shift a rendered line right by `n` columns.
fn indent_line(n: usize, line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(n))];
    spans.extend(line.spans);
    Line::from(spans)
}

/// Render a declaration through the syntax highlighter.
fn hl_code(code: &str, hl: &Highlighter) -> Vec<Line<'static>> {
    let mut lines = render::declaration(code, hl);
    lines.retain(|l| l.width() > 0);
    lines
}

/// Whether an item belongs in a module listing.
///
/// The JSON carries crate-internal items (`std::sys`, `std::panicking`)
/// alongside the public API; only the latter is worth listing. A stripped
/// module is likewise not public API — it exists only to carry re-exports.
/// `#[doc(hidden)]` items never reach the JSON at all.
fn is_public(item: &Item) -> bool {
    if matches!(&item.inner, ItemEnum::Module(m) if m.is_stripped) {
        return false;
    }
    matches!(item.visibility, Visibility::Public | Visibility::Default)
}

/// The first sentence of an item's docs, for a one-line listing.
///
/// This is plain text, not rendered markdown, so rustdoc's `[`Foo`]` link
/// brackets are stripped rather than shown verbatim.
fn summary_line(item: &Item) -> Option<String> {
    let docs = item.docs.as_ref()?;
    let first = docs.lines().find(|l| !l.trim().is_empty())?.trim();
    let cleaned = strip_link_brackets(first);
    Some(cleaned.chars().take(80).collect())
}

/// Remove markdown link brackets from a plain-text summary.
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

/// Whether a section is open, given the sections the reader has toggled.
///
/// A toggled section is the opposite of its default; everything else sits at
/// its default.
pub fn is_expanded(id: SectionId, toggled: &[SectionId]) -> bool {
    id.starts_expanded() != toggled.contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdocs::universe;

    /// What a module page lists: each row's section and name, read from the
    /// page's targets and sections rather than parsed back out of its text.
    fn listings(u: &Universe, r: ItemRef) -> Vec<(&'static str, String)> {
        let page = build(u, r, 100, &Highlighter::new(), &[]);
        page.targets
            .iter()
            .filter(|t| t.spans.is_none())
            .map(|t| {
                let section = page
                    .sections
                    .iter()
                    .rev()
                    .find(|s| s.line < t.line)
                    .expect("a listed row outside any section");
                let name = u.item(t.item).and_then(|i| i.name.clone());
                (section.id.title(), name.unwrap_or_default())
            })
            .collect()
    }

    /// Every kind a module can hold gets a listing, not just the common ones.
    #[test]
    fn module_pages_list_their_contents() {
        let u = universe();
        let at = |path: &str| listings(&u, u.by_path(path).expect(path));

        let root = at("std");
        assert!(
            root.contains(&("Modules", "collections".into())),
            "{root:?}"
        );
        let mem = at("core::mem");
        assert!(mem.contains(&("Unions", "MaybeUninit".into())), "{mem:?}");
        let clone = at("core::clone");
        assert!(
            clone.contains(&("Derive Macros", "Clone".into())),
            "{clone:?}"
        );
    }

    /// A module lists its public API, each item once. The JSON also carries
    /// `pub(crate)` internals the website omits, and re-exports can bring the
    /// same item in more than once: `std::task` would otherwise list its
    /// `task` module twice.
    #[test]
    fn module_listings_show_each_public_child_once() {
        let u = universe();
        let root = listings(&u, u.root().expect("std root"));
        for internal in ["panicking", "backtrace_rs", "__restricted_std_workaround"] {
            assert!(
                root.iter().all(|(_, name)| name != internal),
                "{internal} should not be listed"
            );
        }

        let task = listings(&u, u.by_path("std::task").expect("std::task"));
        let mut seen = std::collections::HashSet::new();
        for row in &task {
            assert!(seen.insert(row), "{row:?} listed twice");
        }
    }

    #[test]
    fn module_listings_are_foldable_sections() {
        let u = universe();
        let hl = Highlighter::new();
        let root = u.root().expect("std root");
        let page = build(&u, root, 100, &hl, &[]);

        // A module page's listings register as sections, so n/p steps them.
        let titles: Vec<&str> = page.sections.iter().map(|s| s.id.title()).collect();
        assert!(titles.contains(&"Modules"), "got {titles:?}");
        assert!(titles.contains(&"Macros"), "got {titles:?}");

        // Every registered section heading really is at the line claimed.
        for Section { id, line } in &page.sections {
            let t = page.lines[*line].to_string();
            assert!(
                t.contains(id.title()),
                "line {line} is not {:?}: {t:?}",
                id.title()
            );
        }

        // Folding one drops its members but keeps the heading. std's root is
        // almost entirely modules, so that is the section worth folding here.
        let modules = SectionId::Module("Modules");
        let folded = build(&u, root, 100, &hl, &[modules]);
        assert!(
            folded.lines.len() < page.lines.len(),
            "folding Modules should shorten the page ({} -> {})",
            page.lines.len(),
            folded.lines.len()
        );
        let titles: Vec<&str> = folded.sections.iter().map(|s| s.id.title()).collect();
        assert!(
            titles.contains(&"Modules"),
            "heading should survive folding"
        );
    }

    #[test]
    fn module_sections_start_open_and_impl_sections_keep_their_defaults() {
        assert!(SectionId::Module("Structs").starts_expanded());
        assert!(SectionId::Impls(ImplGroup::Inherent).starts_expanded());
        assert!(!SectionId::Impls(ImplGroup::Blanket).starts_expanded());

        // `toggled` flips whatever the default was, in both directions.
        let structs = SectionId::Module("Structs");
        let blanket = SectionId::Impls(ImplGroup::Blanket);
        assert!(is_expanded(structs, &[]));
        assert!(!is_expanded(structs, &[structs]));
        assert!(!is_expanded(blanket, &[]));
        assert!(is_expanded(blanket, &[blanket]));
    }

    #[test]
    fn groups_vec_impls_into_four_sections() {
        let u = universe();
        let r = u.by_path("alloc::vec::Vec").expect("Vec");
        let sections = impls_of(&u, r);

        // Vec has inherent, trait, auto and blanket impls; all four must appear
        // rather than one flat list.
        let groups: Vec<ImplGroup> = sections.iter().map(|s| s.group).collect();
        for g in ImplGroup::all() {
            assert!(groups.contains(&g), "missing group {g:?}");
        }
    }

    #[test]
    fn method_docs_are_rendered_and_highlighted() {
        let u = universe();
        let hl = Highlighter::new();
        let r = u.by_path("alloc::string::String").expect("String");
        let page = build(&u, r, 100, &hl, &[]);

        // `String::new` documents itself with a `let s = String::new();`
        // example; finding it proves method docs reach the markdown renderer
        // rather than being cut to a one-line summary.
        let example = page
            .lines
            .iter()
            .position(|l| l.to_string().contains("String::new()"))
            .expect("no rendered example from a method's docs");

        // Syntax highlighting splits a code line into several coloured spans;
        // an unhighlighted line would arrive as one span plus its indent.
        assert!(
            page.lines[example].spans.len() > 3,
            "example line is not syntax highlighted: {:?}",
            page.lines[example]
        );
    }

    /// Impl headers are declarations too, and are highlighted like the method
    /// signatures they introduce rather than rendered as one flat span.
    #[test]
    fn impl_headers_are_highlighted_and_bold() {
        let u = universe();
        let hl = Highlighter::new();
        let r = u.by_path("alloc::string::String").expect("String");
        // Trait impls start folded, so ask for the section that carries the
        // `impl Trait for Type` headers this is about.
        let expanded = [SectionId::Impls(ImplGroup::Trait)];
        let page = build(&u, r, 100, &hl, &expanded);

        let header = page
            .lines
            .iter()
            .find(|l| {
                let text = l.to_string();
                text.trim_start().starts_with("impl ") && text.contains(" for ")
            })
            .expect("no trait impl header on String");

        // `impl` and `for` are keywords, so a highlighted header carries several
        // differently-coloured spans; a flat one would be a single styled span.
        let coloured: Vec<_> = header
            .spans
            .iter()
            .filter(|s| s.style.fg.is_some())
            .collect();
        assert!(
            coloured.len() > 2,
            "impl header is not syntax highlighted: {header:?}"
        );

        // The emphasis lives on the spans themselves, not on a line style that
        // merely shows through beneath them.
        assert!(
            coloured
                .iter()
                .all(|s| s.style.add_modifier.contains(Modifier::BOLD)),
            "impl header lost its bold: {header:?}"
        );
    }

    #[test]
    fn doc_link_targets_carry_span_extents() {
        let u = universe();
        let hl = Highlighter::new();
        let r = u.by_path("core::option::Option").expect("Option");
        let page = build(&u, r, 100, &hl, &[]);

        let links: Vec<&Target> = page.targets.iter().filter(|t| t.spans.is_some()).collect();
        assert!(!links.is_empty(), "no intra-doc link targets on Option");
        for t in links {
            let range = t.spans.clone().unwrap();
            let n = page.lines[t.line].spans.len();
            assert!(range.start < range.end, "empty link extent");
            assert!(
                range.end <= n,
                "link extent {range:?} past line of {n} spans"
            );
        }
    }

    #[test]
    fn enum_pages_list_their_variants() {
        let u = universe();
        let hl = Highlighter::new();
        let r = u.by_path("core::option::Option").expect("Option");
        let page = build(&u, r, 80, &hl, &[]);

        let variants: Vec<&str> = page
            .targets
            .iter()
            .filter(|t| t.spans.is_none())
            .filter_map(|t| u.item(t.item))
            .filter(|i| matches!(i.inner, ItemEnum::Variant(_)))
            .filter_map(|i| i.name.as_deref())
            .collect();
        assert_eq!(variants, ["None", "Some"]);
    }

    /// Columns of leading whitespace on a line.
    fn indent_of(l: &Line<'_>) -> usize {
        let t = l.to_string();
        t.len() - t.trim_start().len()
    }

    /// An item's documentation must sit deeper than the item it describes,
    /// which in turn sits deeper than the impl header introducing it.
    ///
    /// This used to run backwards for methods: declarations were routed
    /// through the markdown renderer, which silently added a code block's own
    /// indent on top of theirs, leaving the prose shallower than the signature
    /// it belonged to.
    #[test]
    fn documentation_is_indented_under_the_item_it_describes() {
        let u = universe();
        let hl = Highlighter::new();
        let r = u.by_path("alloc::string::String").expect("String");
        // Trait impls exercise the deepest ladder on any page.
        let expanded = [SectionId::Impls(ImplGroup::Trait)];
        let page = build(&u, r, 100, &hl, &expanded);

        // Any impl header ends the block before it, `impl String` as much as
        // `impl Display for String`; a header is the one line on a page that
        // sits *above* the signatures around it, so missing one would let the
        // walk below compare a method against the next impl's heading.
        let is_impl_header = |l: &Line<'_>| {
            let t = l.to_string();
            let t = t.trim_start();
            t.starts_with("impl ") || t.starts_with("impl<")
        };

        // Find a documented method: many trait impls (`AddAssign for String`)
        // carry none, and their signature is followed straight by the next
        // impl header.
        let mut checked = 0;
        for (header, _) in page
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| is_impl_header(l))
        {
            assert_eq!(indent_of(&page.lines[header]), indent::HEADING);

            // Everything up to whatever ends this block: the next impl
            // header, or the next section heading if this was the last impl
            // in its section.
            let end = (header + 1..page.lines.len())
                .find(|&i| {
                    is_impl_header(&page.lines[i]) || page.sections.iter().any(|s| s.line == i)
                })
                .unwrap_or(page.lines.len());

            for sig in header + 1..end {
                if !page.lines[sig].to_string().trim_start().starts_with("fn ") {
                    continue;
                }
                assert_eq!(indent_of(&page.lines[sig]), indent::MEMBER);

                // Prose before the next signature, if this method has any.
                let Some(docs) = (sig + 1..end)
                    .take_while(|&i| !page.lines[i].to_string().trim_start().starts_with("fn "))
                    .find(|&i| !page.lines[i].to_string().trim().is_empty())
                else {
                    continue;
                };
                assert!(
                    indent_of(&page.lines[docs]) > indent_of(&page.lines[sig]),
                    "docs at {} are shallower than the signature at {}: {:?}",
                    indent_of(&page.lines[docs]),
                    indent_of(&page.lines[sig]),
                    page.lines[docs].to_string(),
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "no documented method under any trait impl");
    }

    /// A member's description carries a background across the full page width,
    /// so one method's prose is visibly separate from the next's. Padding to
    /// the width is the whole point: a tint that stopped at the end of the
    /// text would leave a ragged edge rather than a block.
    #[test]
    fn member_documentation_is_painted_across_the_full_width() {
        const WIDTH: u16 = 100;
        let u = universe();
        let hl = Highlighter::new();
        let r = u.by_path("core::option::Option").expect("Option");
        let page = build(&u, r, WIDTH, &hl, &[]);

        let want = theme::member_docs().bg.expect("panel bg");
        let tinted = page
            .lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.style.bg == Some(want)))
            .expect("no member documentation carried the panel background");

        assert_eq!(
            tinted.width(),
            WIDTH as usize,
            "tinted line stops short of the width: {:?}",
            tinted.to_string()
        );
        // The page's own top-level docs are not a member's, so they stay plain
        // — but they are indented to the same rung as one, so the description
        // hangs off the declaration rather than sitting flush left.
        // A link before the first member row sits in the page's own docs.
        let first_member = page
            .targets
            .iter()
            .find(|t| t.spans.is_none())
            .expect("no member rows")
            .line;
        let first_prose = page
            .targets
            .iter()
            .find(|t| t.spans.is_some() && t.line < first_member)
            .map(|t| &page.lines[t.line])
            .expect("no link in Option's own docs");
        assert!(
            first_prose.spans.iter().all(|s| s.style.bg.is_none()),
            "the page's own docs were tinted like a member's"
        );
        assert_eq!(
            indent_of(first_prose),
            indent::HEADING,
            "the page's own docs are not indented: {:?}",
            first_prose.to_string()
        );
    }
}
