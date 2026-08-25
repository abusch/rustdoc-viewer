//! Application state and key handling.

use crate::docs::{ItemRef, Universe};
use crate::index::SearchIndex;
use crate::page::{self, ImplGroup, Page, Target};
use crate::render::Highlighter;

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
    expanded: Vec<ImplGroup>,
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
    /// Width of the last frame, used to decide when to re-wrap.
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
        self.results = self
            .index
            .search(&self.query, 200)
            .into_iter()
            .map(|h| h.entry_idx)
            .collect();
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
    pub fn navigate_to(&mut self, item: ItemRef) {
        self.save_scroll();
        // Following a new link discards any forward history, like a browser.
        self.history.truncate(self.cursor);
        self.history.push(HistoryEntry {
            item,
            scroll: 0,
            expanded: page::default_expanded(),
        });
        self.cursor = self.history.len();
        self.scroll = 0;
        self.focus = None;
        self.section_cursor = 0;
        self.screen = Screen::Item;
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
        let (item, expanded) = (entry.item, entry.expanded.clone());
        let width = self.page_width();
        let page = page::build(&self.universe, item, width, &self.highlighter, &expanded);
        self.page = Some(page);
        self.clamp_scroll();
    }

    /// Re-render if the terminal width changed since the page was built.
    pub fn ensure_width(&mut self, width: u16) {
        let want = width.saturating_sub(2);
        if self.page.as_ref().is_some_and(|p| p.width != want) {
            self.rebuild_page();
        }
    }

    fn page_width(&self) -> u16 {
        self.last_width.saturating_sub(2).max(20)
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
        let Some(group) = p.sections.get(self.section_cursor).copied() else {
            self.status = Some("no section selected — use n/p".into());
            return;
        };

        if let Some(entry) = self.history.get_mut(self.cursor.saturating_sub(1)) {
            if let Some(i) = entry.expanded.iter().position(|g| *g == group) {
                entry.expanded.remove(i);
            } else {
                entry.expanded.push(group);
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
        let Some(t) = p.targets.get(f) else { return false };
        let item = t.item;
        self.navigate_to(item);
        true
    }

    /// Open the item one level up: the module holding a type, the type holding
    /// a method.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load;
    use crate::page::ImplGroup;

    fn app() -> Option<App> {
        let crates = load::load_std_crates().ok()?;
        let names = load::STD_CRATES.iter().map(|s| s.to_string()).collect();
        let mut u = Universe::new(crates, names);
        let idx = SearchIndex::build(&u);
        u.set_display_paths(idx.display_paths());
        Some(App::new(u, idx))
    }

    #[test]
    fn starts_on_the_std_root_page() {
        let Some(a) = app() else {
            eprintln!("skipping: rust-docs-json not available");
            return;
        };
        assert_eq!(a.screen, Screen::Item);
        assert_eq!(a.page.as_ref().expect("a page").title, "std");
        // Back from the landing page has nowhere to go but search.
        let mut a = a;
        a.go_back();
        assert_eq!(a.screen, Screen::Search);
    }

    #[test]
    fn u_goes_up_to_the_parent_and_records_history() {
        let Some(mut a) = app() else {
            eprintln!("skipping: rust-docs-json not available");
            return;
        };
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
        let Some(mut a) = app() else { return };
        let root = a.universe.root().expect("std root");
        a.navigate_to(root);

        a.go_to_parent();
        assert_eq!(a.page.as_ref().unwrap().title, "std");
        assert!(a.status.is_some(), "should explain why nothing happened");
    }

    #[test]
    fn toggling_a_section_expands_it() {
        let Some(mut a) = app() else {
            eprintln!("skipping: rust-docs-json not available");
            return;
        };
        let vec_ref = a.universe.by_path("alloc::vec::Vec").expect("Vec");
        a.navigate_to(vec_ref);

        let sections = a.page.as_ref().unwrap().sections.clone();
        let blanket = sections
            .iter()
            .position(|g| *g == ImplGroup::Blanket)
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
        let Some(mut a) = app() else {
            eprintln!("skipping: rust-docs-json not available");
            return;
        };
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
        let Some(mut a) = app() else { return };
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
        let Some(mut a) = app() else { return };
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
        let Some(mut a) = app() else { return };
        let vec_ref = a.universe.by_path("alloc::vec::Vec").expect("Vec");
        a.navigate_to(vec_ref);

        a.section_cursor = 2;
        a.scroll_to_bottom();
        assert_eq!(a.section_cursor, 2, "G must not change which section folds");
    }

    #[test]
    fn history_records_and_restores_scroll() {
        let Some(mut a) = app() else { return };
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
}
