//! Application state and key handling.

use crate::docs::{ItemRef, Universe};
use crate::index::SearchIndex;
use crate::page::{self, Page, SectionId, Target};
use crate::render::Highlighter;

/// Narrowest width a page is ever built at, so wrapping stays sane in a
/// terminal too small to read comfortably anyway.
const MIN_PAGE_WIDTH: u16 = 20;

/// Which screen is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Search,
    Item,
    Help,
}

/// One entry in the browsing history.
struct HistoryEntry {
    item: ItemRef,
    scroll: u16,
    /// Sections the reader has toggled away from their default state; see
    /// [`page::is_expanded`].
    toggled: Vec<SectionId>,
}

pub struct App {
    pub universe: Universe,
    pub index: SearchIndex,
    pub highlighter: Highlighter,

    pub screen: Screen,
    /// The screen to return to when leaving help.
    prev_screen: Screen,
    pub should_quit: bool,

    // Search state
    pub query: String,
    pub results: Vec<usize>,
    pub selected: usize,
    pub search_offset: usize,

    // Item state
    pub page: Option<Page>,
    pub scroll: u16,
    /// Index into the current page's targets, when one is focused.
    pub focus: Option<usize>,
    /// Which impl section `space` will fold, and `n`/`p` step through.
    pub section_cursor: usize,

    history: Vec<HistoryEntry>,
    cursor: usize,

    pub viewport: u16,
    pub status: Option<String>,
    /// Width the item view last had room for, used to decide when to re-wrap.
    /// This is the padded content width, not the terminal's: it must match what
    /// [`Self::ensure_width`] is given or every frame rebuilds the page.
    pub last_width: u16,
}

impl App {
    pub fn new(universe: Universe, index: SearchIndex) -> Self {
        let mut app = Self {
            universe,
            index,
            highlighter: Highlighter::new(),
            screen: Screen::Search,
            prev_screen: Screen::Search,
            should_quit: false,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            search_offset: 0,
            page: None,
            scroll: 0,
            focus: None,
            section_cursor: 0,
            history: Vec::new(),
            cursor: 0,
            viewport: 20,
            status: None,
            last_width: 80,
        };
        app.refresh_search();
        // Open on the crate root so there is something to read and navigate
        // straight away; fall back to search if it cannot be resolved.
        if let Some(root) = app.universe.root() {
            app.navigate_to(root);
        }
        app
    }

    // --- Search -----------------------------------------------------------

    pub fn refresh_search(&mut self) {
        self.results = self.index.search(&self.query, 200);
        self.selected = 0;
        self.search_offset = 0;
    }

    pub fn select_next(&mut self) {
        if !self.results.is_empty() {
            self.selected = (self.selected + 1).min(self.results.len() - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Keep the selected row inside the visible window.
    pub fn clamp_search_scroll(&mut self, height: usize) {
        if self.selected < self.search_offset {
            self.search_offset = self.selected;
        } else if height > 0 && self.selected >= self.search_offset + height {
            self.search_offset = self.selected + 1 - height;
        }
    }

    /// Open the item currently selected in the search results.
    pub fn open_selected(&mut self) {
        let Some(idx) = self.results.get(self.selected).copied() else {
            return;
        };
        let item = self.index.entry(idx).item;
        self.navigate_to(item);
    }

    // --- Navigation -------------------------------------------------------

    /// Follow a link, pushing onto the history.
    ///
    /// A method has no page of its own: it is shown on the page of the type
    /// that carries it, focused within the impl section that lists it. See
    /// [`Self::member_of`].
    pub fn navigate_to(&mut self, item: ItemRef) {
        let member = self.member_of(item);
        let target = member.map_or(item, |(owner, _)| owner);

        self.save_scroll();
        // Following a new link discards any forward history, like a browser.
        self.history.truncate(self.cursor);
        self.history.push(HistoryEntry {
            item: target,
            scroll: 0,
            toggled: page::default_expanded(),
        });
        self.cursor = self.history.len();
        self.scroll = 0;
        self.focus = None;
        self.section_cursor = 0;
        self.screen = Screen::Item;
        self.rebuild_page();

        if let Some((_, method)) = member {
            self.reveal(method);
        }
    }

    /// Resolve an item to the type that carries it, if it is a member.
    ///
    /// Two routes, because neither covers everything. Inherent methods are in
    /// `paths`, so the parent is read from there: that is what distinguishes
    /// `String::push_str` from `mem::swap`, since both are `Function`s and only
    /// the parent says which is a member. Trait-impl members such as `String`'s
    /// `clone` are missing from `paths` altogether — no path, no kind — and are
    /// found instead through the impl that lists them.
    fn member_of(&self, item: ItemRef) -> Option<(ItemRef, ItemRef)> {
        use rustdoc_types::ItemKind as K;
        let owner = match self.universe.kind_of(item) {
            Some(K::Function | K::AssocConst | K::AssocType | K::StructField) => {
                let owner = self.universe.parent_of(item)?;
                // A module parent means a free function, which keeps its page.
                matches!(
                    self.universe.kind_of(owner)?,
                    K::Struct | K::Enum | K::Union | K::Trait | K::Primitive
                )
                .then_some(owner)?
            }
            // Not in `paths` at all: only an impl can account for it.
            None => self.universe.owner_of(item)?,
            Some(_) => return None,
        };
        (owner != item).then_some((owner, item))
    }

    /// Focus `item` on the page just built, unfolding the section holding it.
    ///
    /// The impl sections that list trait methods start folded, so the target
    /// usually does not exist on the freshly built page; expanding its section
    /// and rebuilding is what brings it into being.
    fn reveal(&mut self, item: ItemRef) {
        if self.focus_on(item) {
            return;
        }
        // Not on the page as built: open each folded section until it appears.
        let sections: Vec<SectionId> = self
            .page
            .as_ref()
            .map(|p| p.sections.clone())
            .unwrap_or_default();
        for id in sections {
            if page::is_expanded(id, self.toggled()) {
                continue;
            }
            self.toggle_section_id(id);
            if self.focus_on(item) {
                return;
            }
            // Leave only the section that actually holds the target open.
            self.toggle_section_id(id);
        }
    }

    /// Focus the target for `item` on the current page, scrolling it into view.
    ///
    /// A method can be named twice on its owner's page: once by the row that
    /// declares it, and again by any intra-doc link in prose that happens to
    /// point at it. The declaration is the one worth landing on, and it is the
    /// target that owns its whole line, so prefer that over a link's span.
    fn focus_on(&mut self, item: ItemRef) -> bool {
        let Some(p) = &self.page else { return false };
        let Some(i) = target_index(p, item, true) else {
            return false;
        };
        let line = p.targets[i].line as u16;
        self.focus = Some(i);
        // Sit the target a third of the way down rather than at the very top,
        // so the impl header above it stays visible.
        self.scroll = line.saturating_sub(self.viewport / 3);
        self.clamp_scroll();
        true
    }

    /// The sections toggled away from their default on the current page.
    fn toggled(&self) -> &[SectionId] {
        self.current().map(|e| e.toggled.as_slice()).unwrap_or(&[])
    }

    /// Flip one section's folded state and rebuild.
    fn toggle_section_id(&mut self, id: SectionId) {
        if let Some(entry) = self.history.get_mut(self.cursor.saturating_sub(1)) {
            match entry.toggled.iter().position(|s| *s == id) {
                Some(i) => {
                    entry.toggled.remove(i);
                }
                None => entry.toggled.push(id),
            }
        }
        self.rebuild_page();
    }

    pub fn go_back(&mut self) {
        if self.cursor <= 1 {
            // Nothing behind the first page: fall back to search.
            if self.screen == Screen::Item {
                self.screen = Screen::Search;
            }
            return;
        }
        self.save_scroll();
        self.cursor -= 1;
        self.restore();
    }

    pub fn go_forward(&mut self) {
        if self.cursor >= self.history.len() {
            return;
        }
        self.save_scroll();
        self.cursor += 1;
        self.restore();
    }

    fn restore(&mut self) {
        if let Some(entry) = self.history.get(self.cursor.saturating_sub(1)) {
            self.scroll = entry.scroll;
            self.focus = None;
            self.section_cursor = 0;
            self.screen = Screen::Item;
            self.rebuild_page();
        }
    }

    fn save_scroll(&mut self) {
        if self.cursor > 0
            && let Some(entry) = self.history.get_mut(self.cursor - 1)
        {
            entry.scroll = self.scroll;
        }
    }

    fn current(&self) -> Option<&HistoryEntry> {
        self.history.get(self.cursor.checked_sub(1)?)
    }

    pub fn rebuild_page(&mut self) {
        let Some(entry) = self.current() else {
            self.page = None;
            return;
        };
        let (item, toggled) = (entry.item, entry.toggled.clone());
        let width = self.page_width();
        // Re-wrapping moves every line, so remember what the focus pointed at
        // and follow it to wherever it lands; the raw index would otherwise
        // address a different target, and the scroll a different line.
        let focused = self
            .focus
            .and_then(|f| self.page.as_ref()?.targets.get(f))
            .map(|t| (t.item, t.line, t.spans.is_none()));
        let page = page::build(&self.universe, item, width, &self.highlighter, &toggled);
        self.page = Some(page);

        if let Some((item, was, decl)) = focused
            && self.refocus(item, was, decl)
        {
            return;
        }
        self.clamp_scroll();
    }

    /// Re-point the focus at `item` after a rebuild, keeping it on screen if it
    /// was before. Returns whether the target was found again.
    ///
    /// `decl` carries whether the focus was on the row declaring `item` rather
    /// than a link to it, since one item can be both and the two sit far apart
    /// on the page.
    fn refocus(&mut self, item: ItemRef, was: usize, decl: bool) -> bool {
        let Some(p) = &self.page else { return false };
        let Some(i) = target_index(p, item, decl) else {
            self.focus = None;
            return false;
        };
        let now = p.targets[i].line;
        self.focus = Some(i);
        // Keep the target the same distance down the viewport as it was, so a
        // resize does not jump the page around under the reader.
        let offset = (was as u16).saturating_sub(self.scroll);
        self.scroll = (now as u16).saturating_sub(offset);
        self.clamp_scroll();
        true
    }

    /// Re-render if the terminal width changed since the page was built.
    pub fn ensure_width(&mut self, width: u16) {
        if self
            .page
            .as_ref()
            .is_some_and(|p| p.width != width.max(MIN_PAGE_WIDTH))
        {
            self.rebuild_page();
        }
    }

    /// The width pages are built at. The item view has no border, so this is
    /// the whole terminal; both callers must agree or every frame rebuilds.
    fn page_width(&self) -> u16 {
        self.last_width.max(MIN_PAGE_WIDTH)
    }

    // --- Scrolling --------------------------------------------------------

    pub fn scroll_by(&mut self, delta: i32) {
        let next = self.scroll as i32 + delta;
        self.scroll = next.max(0) as u16;
        self.clamp_scroll();
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll = 0;
    }

    pub fn scroll_to_bottom(&mut self) {
        if let Some(p) = &self.page {
            let max = p.lines.len().saturating_sub(self.viewport as usize);
            self.scroll = max as u16;
        }
    }

    fn clamp_scroll(&mut self) {
        if let Some(p) = &self.page {
            let max = p.lines.len().saturating_sub(self.viewport as usize) as u16;
            self.scroll = self.scroll.min(max);
        }
    }

    // --- Sections ---------------------------------------------------------

    /// Step the section cursor forward and scroll its header into view.
    pub fn next_section(&mut self) {
        self.move_section(1);
    }

    pub fn prev_section(&mut self) {
        self.move_section(-1);
    }

    fn move_section(&mut self, delta: i32) {
        let Some(p) = &self.page else { return };
        if p.section_lines.is_empty() {
            return;
        }
        let last = p.section_lines.len() - 1;
        let next = (self.section_cursor as i32 + delta).clamp(0, last as i32) as usize;
        self.section_cursor = next;
        self.scroll = p.section_lines[next] as u16;
        self.clamp_scroll();
    }

    /// Fold or unfold the section under the cursor.
    ///
    /// The cursor is tracked explicitly rather than inferred from the scroll
    /// offset, so `G` (jump to bottom) does not change which section `space`
    /// acts on.
    pub fn toggle_section(&mut self) {
        let Some(p) = &self.page else { return };
        let Some(section) = p.sections.get(self.section_cursor).copied() else {
            self.status = Some("no section selected — use n/p".into());
            return;
        };

        if let Some(entry) = self.history.get_mut(self.cursor.saturating_sub(1)) {
            if let Some(i) = entry.toggled.iter().position(|g| *g == section) {
                entry.toggled.remove(i);
            } else {
                entry.toggled.push(section);
            }
        }
        self.rebuild_page();

        // Keep the toggled header where the eye already is.
        if let Some(p) = &self.page
            && let Some(l) = p.section_lines.get(self.section_cursor)
        {
            self.scroll = *l as u16;
            self.clamp_scroll();
        }
    }

    // --- Links ------------------------------------------------------------

    /// Focus the next link/target visible in the viewport.
    pub fn focus_next_target(&mut self) {
        self.focus_target(true);
    }

    /// Focus the previous link/target.
    pub fn focus_prev_target(&mut self) {
        self.focus_target(false);
    }

    /// Step the focus one target forwards or backwards, wrapping at the ends.
    ///
    /// With nothing focused yet, start from what the reader can actually see
    /// rather than from the top of the page: the first target in the viewport
    /// going forwards, the last one going backwards.
    fn focus_target(&mut self, forward: bool) {
        let Some(p) = &self.page else { return };
        if p.targets.is_empty() {
            self.status = Some("no links on this page".into());
            return;
        }
        let last = p.targets.len() - 1;
        let start = self.scroll as usize;
        let end = start + self.viewport as usize;
        let in_view = |t: &Target| t.line >= start && t.line < end;

        self.focus = Some(match (self.focus, forward) {
            (Some(f), true) if f < last => f + 1,
            (Some(_), true) => 0,
            (Some(f), false) if f > 0 => f - 1,
            (Some(_), false) => last,
            (None, true) => p
                .targets
                .iter()
                .position(in_view)
                .or_else(|| p.targets.iter().position(|t| t.line >= start))
                .unwrap_or(0),
            (None, false) => p
                .targets
                .iter()
                .rposition(in_view)
                .or_else(|| p.targets.iter().rposition(|t| t.line < end))
                .unwrap_or(last),
        });

        // Scroll the focused target into view.
        if let Some(f) = self.focus
            && let Some(t) = p.targets.get(f)
        {
            let line = t.line as u16;
            if line < self.scroll || line >= self.scroll + self.viewport {
                self.scroll = line.saturating_sub(self.viewport / 3);
            }
        }
        self.clamp_scroll();
    }

    /// Follow the focused target, if any.
    pub fn follow_focus(&mut self) -> bool {
        let Some(f) = self.focus else { return false };
        let Some(p) = &self.page else { return false };
        let Some(t) = p.targets.get(f) else {
            return false;
        };
        let item = t.item;
        self.navigate_to(item);
        true
    }

    /// Open the item one level up: the module holding a type, or the module
    /// holding that module.
    ///
    /// A method is never the page's own item — it is shown on its type's page —
    /// so going up from one means going up from the type.
    pub fn go_to_parent(&mut self) {
        let Some(item) = self.current().map(|e| e.item) else {
            return;
        };
        match self.universe.parent_of(item) {
            Some(parent) => self.navigate_to(parent),
            None => self.status = Some("already at the top".into()),
        }
    }

    pub fn show_help(&mut self) {
        if self.screen != Screen::Help {
            self.prev_screen = self.screen;
            self.screen = Screen::Help;
        }
    }

    pub fn close_help(&mut self) {
        if self.screen == Screen::Help {
            self.screen = self.prev_screen;
        }
    }

    pub fn has_page(&self) -> bool {
        self.page.is_some()
    }
}

/// The index of the target for `item`, preferring the row that declares it.
///
/// An item can be named twice on a page: by the row declaring it, and by any
/// intra-doc link pointing at it. With `prefer_decl` the declaration wins, which
/// is what landing on a method should do; a link's own target is used otherwise.
fn target_index(page: &Page, item: ItemRef, prefer_decl: bool) -> Option<usize> {
    let matches = |t: &Target| t.item == item;
    if prefer_decl
        && let Some(i) = page
            .targets
            .iter()
            .position(|t| matches(t) && t.spans.is_none())
    {
        return Some(i);
    }
    page.targets.iter().position(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{ImplGroup, SectionId};

    fn app() -> App {
        let (u, idx) = crate::testdocs::indexed();
        App::new(u, idx)
    }

    #[test]
    fn starts_on_the_std_root_page() {
        let a = app();
        assert_eq!(a.screen, Screen::Item);
        assert_eq!(a.page.as_ref().expect("a page").title, "std");
        // Back from the landing page has nowhere to go but search.
        let mut a = a;
        a.go_back();
        assert_eq!(a.screen, Screen::Search);
    }

    #[test]
    fn u_goes_up_to_the_parent_and_records_history() {
        let mut a = app();
        let vec_ref = a.universe.by_path("alloc::vec::Vec").expect("Vec");
        a.navigate_to(vec_ref);
        assert_eq!(a.page.as_ref().unwrap().title, "std::vec::Vec");

        a.go_to_parent();
        assert_eq!(a.page.as_ref().unwrap().title, "std::vec");

        // Going up is a normal navigation, so back returns to where you were.
        a.go_back();
        assert_eq!(a.page.as_ref().unwrap().title, "std::vec::Vec");
    }

    #[test]
    fn u_at_the_crate_root_says_so_and_stays_put() {
        let mut a = app();
        let root = a.universe.root().expect("std root");
        a.navigate_to(root);

        a.go_to_parent();
        assert_eq!(a.page.as_ref().unwrap().title, "std");
        assert!(a.status.is_some(), "should explain why nothing happened");
    }

    #[test]
    fn toggling_a_section_expands_it() {
        let mut a = app();
        let vec_ref = a.universe.by_path("alloc::vec::Vec").expect("Vec");
        a.navigate_to(vec_ref);

        let sections = a.page.as_ref().unwrap().sections.clone();
        let blanket = sections
            .iter()
            .position(|g| *g == SectionId::Impls(ImplGroup::Blanket))
            .expect("blanket section");

        let before = a.page.as_ref().unwrap().lines.len();
        a.section_cursor = blanket;
        a.toggle_section();
        let after = a.page.as_ref().unwrap().lines.len();

        assert!(
            after > before,
            "expanding blanket impls should add lines ({before} -> {after})"
        );
    }

    #[test]
    fn shift_tab_walks_links_backwards() {
        let mut a = app();
        let opt = a.universe.by_path("core::option::Option").expect("Option");
        a.navigate_to(opt);
        a.viewport = 30;

        a.focus_next_target();
        a.focus_next_target();
        let second = a.focus.expect("a target should be focused");
        a.focus_prev_target();
        assert_eq!(a.focus, Some(second - 1), "shift-tab should step back one");
    }

    #[test]
    fn focus_wraps_at_both_ends() {
        let mut a = app();
        let opt = a.universe.by_path("core::option::Option").expect("Option");
        a.navigate_to(opt);
        a.viewport = 30;
        let last = a.page.as_ref().unwrap().targets.len() - 1;

        // Backwards off the front wraps to the final target.
        a.focus = Some(0);
        a.focus_prev_target();
        assert_eq!(a.focus, Some(last));

        // And forwards off the end comes back to the first.
        a.focus_next_target();
        assert_eq!(a.focus, Some(0));
    }

    #[test]
    fn first_shift_tab_starts_from_the_visible_page() {
        let mut a = app();
        let opt = a.universe.by_path("core::option::Option").expect("Option");
        a.navigate_to(opt);
        a.viewport = 30;

        // With nothing focused, stepping back should pick a target the reader
        // can see rather than jumping to the end of a long page.
        a.focus_prev_target();
        let f = a.focus.expect("a target should be focused");
        let line = a.page.as_ref().unwrap().targets[f].line;
        assert!(
            line < a.scroll as usize + a.viewport as usize,
            "focused a target below the viewport"
        );
    }

    #[test]
    fn jumping_to_bottom_does_not_move_the_section_cursor() {
        let mut a = app();
        let vec_ref = a.universe.by_path("alloc::vec::Vec").expect("Vec");
        a.navigate_to(vec_ref);

        a.section_cursor = 2;
        a.scroll_to_bottom();
        assert_eq!(a.section_cursor, 2, "G must not change which section folds");
    }

    #[test]
    fn history_records_and_restores_scroll() {
        let mut a = app();
        let s = a.universe.by_path("alloc::string::String").expect("String");
        let o = a.universe.by_path("core::option::Option").expect("Option");

        a.navigate_to(s);
        a.scroll = 42;
        a.navigate_to(o);
        assert_eq!(a.scroll, 0, "a fresh page starts at the top");

        a.go_back();
        assert_eq!(a.scroll, 42, "going back restores the previous position");

        a.go_forward();
        assert_eq!(a.page.as_ref().unwrap().title, "std::option::Option");
    }

    /// A method is shown on its type's page, focused on its declaration,
    /// rather than getting a bare page of its own.
    #[test]
    fn opening_an_inherent_method_lands_on_its_type() {
        let mut a = app();
        let m = a
            .universe
            .by_path("alloc::string::String::push_str")
            .expect("String::push_str");
        a.navigate_to(m);

        let page = a.page.as_ref().expect("a page");
        assert_eq!(page.title, "std::string::String");

        let line = a
            .focus
            .and_then(|f| page.targets.get(f))
            .map(|t| text_of(&page.lines[t.line]))
            .expect("no method focused");
        assert!(
            line.trim().starts_with("fn push_str"),
            "focused the wrong line: {line:?}"
        );
    }

    /// Trait-impl members are absent from rustdoc's `paths`, so they resolve
    /// through the impl that lists them, and their section is unfolded to
    /// bring them into view.
    #[test]
    fn opening_a_trait_method_unfolds_its_section() {
        let mut a = app();
        let sr = a.universe.by_path("alloc::string::String").expect("String");

        // Find `clone` the way the reader would: on String's page, with the
        // trait impls open.
        a.navigate_to(sr);
        let ids: Vec<SectionId> = a.page.as_ref().unwrap().sections.clone();
        for id in ids {
            if !page::is_expanded(id, a.toggled()) {
                a.toggle_section_id(id);
            }
        }
        let page = a.page.as_ref().unwrap();
        let clone = page
            .targets
            .iter()
            .find(|t| {
                t.spans.is_none()
                    && text_of(&page.lines[t.line]).trim() == "fn clone(&self) -> Self"
            })
            .map(|t| t.item)
            .expect("no clone method on String");

        // Opening it from a fresh app must still land on String, which means
        // unfolding the section that was closed by default.
        let mut b = app();
        b.navigate_to(clone);
        let page = b.page.as_ref().expect("a page");
        assert_eq!(page.title, "std::string::String");
        let line = b
            .focus
            .and_then(|f| page.targets.get(f))
            .map(|t| text_of(&page.lines[t.line]))
            .expect("no method focused");
        assert_eq!(line.trim(), "fn clone(&self) -> Self");
    }

    /// A free function is not a member and keeps its own page, even though
    /// rustdoc gives it the same `Function` kind as a method.
    #[test]
    fn a_free_function_keeps_its_own_page() {
        let mut a = app();
        let f = a.universe.by_path("core::mem::swap").expect("mem::swap");
        a.navigate_to(f);

        let page = a.page.as_ref().expect("a page");
        assert_eq!(page.title, "std::mem::swap");
        assert_eq!(a.focus, None, "nothing should be focused on its own page");
    }

    /// Going back from a method lands where the reader came from, not on the
    /// method page that no longer exists.
    #[test]
    fn history_holds_the_type_a_method_redirected_to() {
        let mut a = app();
        let opt = a.universe.by_path("core::option::Option").expect("Option");
        let m = a
            .universe
            .by_path("alloc::string::String::push_str")
            .expect("String::push_str");

        a.navigate_to(opt);
        a.navigate_to(m);
        assert_eq!(a.page.as_ref().unwrap().title, "std::string::String");

        a.go_back();
        assert_eq!(a.page.as_ref().unwrap().title, "std::option::Option");
        a.go_forward();
        assert_eq!(a.page.as_ref().unwrap().title, "std::string::String");
    }

    /// Flatten a line to its text, for asserting on what was focused.
    fn text_of(l: &ratatui::text::Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Re-wrapping at a new width moves every line, so a focused target must be
    /// followed to its new position instead of leaving the scroll behind.
    #[test]
    fn a_focused_method_survives_a_width_change() {
        let mut a = app();
        a.last_width = 80;
        a.viewport = 25;
        let m = a
            .universe
            .by_path("alloc::string::String::push_str")
            .expect("String::push_str");
        a.navigate_to(m);

        let on_screen = |a: &App| {
            let p = a.page.as_ref().unwrap();
            let t = &p.targets[a.focus.expect("focused")];
            let line = t.line as u16;
            (line >= a.scroll && line < a.scroll + a.viewport)
                .then(|| text_of(&p.lines[t.line]).trim().to_string())
        };
        assert!(
            on_screen(&a).is_some_and(|l| l.starts_with("fn push_str")),
            "not visible before the resize"
        );

        // Same page, rebuilt for a wider terminal.
        a.last_width = 140;
        a.ensure_width(140);
        assert!(
            on_screen(&a).is_some_and(|l| l.starts_with("fn push_str")),
            "the focused method scrolled off after the resize"
        );
    }
}
