//! A text-mode browser for Rust documentation, driven by rustdoc's JSON output.

mod app;
mod docs;
mod format;
mod index;
mod load;
mod page;
mod render;
#[cfg(test)]
mod testdocs;
mod ui;

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use app::{App, Screen};

fn main() -> Result<()> {
    // Load before touching the terminal, so errors print normally.
    eprint!("loading rust documentation... ");
    let started = std::time::Instant::now();
    let crates = load::load_std_crates()?;
    let names = load::STD_CRATES.iter().map(|s| s.to_string()).collect();
    let mut universe = docs::Universe::new(crates, names);
    let index = index::SearchIndex::build(&universe);
    // Show items under the name people searched for, not their defining crate.
    universe.set_display_paths(index.display_paths());
    eprintln!(
        "{} items in {:.2}s",
        index.len(),
        started.elapsed().as_secs_f32()
    );

    let mut app = App::new(universe, index);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Poll so a resize redraws promptly even without key input.
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                app.status = None;
                handle_key(app, key);
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Help swallows every key except the ones that dismiss it.
    if app.screen == Screen::Help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => app.close_help(),
            _ => {}
        }
        return;
    }

    // Global bindings.
    match (key.code, ctrl) {
        (KeyCode::Char('c'), true) => {
            app.should_quit = true;
            return;
        }
        (KeyCode::Char('?'), _) if app.screen != Screen::Search => {
            app.show_help();
            return;
        }
        _ => {}
    }

    match app.screen {
        Screen::Search => search_key(app, key, ctrl),
        Screen::Item => item_key(app, key, ctrl),
        Screen::Help => {}
    }
}

fn search_key(app: &mut App, key: KeyEvent, ctrl: bool) {
    match (key.code, ctrl) {
        (KeyCode::Esc, _) => {
            // Esc dismisses the search and returns to the page behind it.
            // Only `q` quits, so with no page to go back to it does nothing.
            if app.has_page() {
                app.screen = Screen::Item;
            }
        }
        (KeyCode::Enter, _) => app.open_selected(),
        (KeyCode::Down, _) | (KeyCode::Char('n'), true) => app.select_next(),
        (KeyCode::Up, _) | (KeyCode::Char('p'), true) => app.select_prev(),
        (KeyCode::Backspace, _) => {
            app.query.pop();
            app.refresh_search();
        }
        (KeyCode::Char('u'), true) => {
            app.query.clear();
            app.refresh_search();
        }
        (KeyCode::Char(c), false) => {
            app.query.push(c);
            app.refresh_search();
        }
        _ => {}
    }
}

fn item_key(app: &mut App, key: KeyEvent, ctrl: bool) {
    let half = (app.viewport / 2).max(1) as i32;
    match (key.code, ctrl) {
        (KeyCode::Char('q'), false) => app.should_quit = true,
        (KeyCode::Char('/'), false) => {
            app.screen = Screen::Search;
            app.query.clear();
            app.refresh_search();
        }
        (KeyCode::Enter, _) => {
            if !app.follow_focus() {
                app.status = Some("press tab to focus a link first".into());
            }
        }
        (KeyCode::Tab, _) => app.focus_next_target(),
        // Crossterm reports shift-tab as its own key code, not Tab+SHIFT.
        (KeyCode::BackTab, _) => app.focus_prev_target(),
        (KeyCode::Backspace, _) | (KeyCode::Char('o'), true) => app.go_back(),
        // Terminals send the same byte for Tab and Ctrl-i, and Tab is bound to
        // link focus, so forward navigation lives on Ctrl-f instead.
        (KeyCode::Char('f'), true) => app.go_forward(),
        (KeyCode::Char('j'), false) | (KeyCode::Down, false) => app.scroll_by(1),
        (KeyCode::Char('k'), false) | (KeyCode::Up, false) => app.scroll_by(-1),
        (KeyCode::Char('d'), true) | (KeyCode::PageDown, _) => app.scroll_by(half),
        (KeyCode::Char('u'), true) | (KeyCode::PageUp, _) => app.scroll_by(-half),
        (KeyCode::Char('g'), false) | (KeyCode::Home, _) => app.scroll_to_top(),
        (KeyCode::Char('G'), false) | (KeyCode::End, _) => app.scroll_to_bottom(),
        (KeyCode::Char('u'), false) => app.go_to_parent(),
        (KeyCode::Char('n'), false) => app.next_section(),
        (KeyCode::Char('p'), false) => app.prev_section(),
        (KeyCode::Char(' '), _) => app.toggle_section(),
        (KeyCode::Char('?'), _) => app.show_help(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let (u, idx) = crate::testdocs::indexed();
        App::new(u, idx)
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.status = None;
        handle_key(app, KeyEvent::new(code, modifiers));
    }

    /// `u` and `^u` share a character and must stay told apart: one walks up
    /// to the parent item, the other scrolls half a page.
    #[test]
    fn u_goes_up_while_ctrl_u_scrolls() {
        let mut a = app();
        let vec_ref = a.universe.by_path("alloc::vec::Vec").expect("Vec");
        a.navigate_to(vec_ref);

        // ^u scrolls within the page, leaving the item alone.
        a.scroll_by(40);
        let scrolled = a.scroll;
        press(&mut a, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(a.scroll < scrolled, "^u should scroll up");
        assert_eq!(a.page.as_ref().unwrap().title, "std::vec::Vec");

        // Plain u leaves the page for its parent.
        press(&mut a, KeyCode::Char('u'), KeyModifiers::NONE);
        assert_eq!(a.page.as_ref().unwrap().title, "std::vec");
    }

    /// Only `q` quits. Esc backs out of whatever is in front, and on the
    /// item view there is nothing to back out of.
    #[test]
    fn esc_does_not_quit() {
        let mut a = app();

        // Item view: Esc is inert.
        press(&mut a, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!a.should_quit, "esc should not quit the item view");
        assert_eq!(a.screen, Screen::Item);

        // Search: Esc dismisses it back to the page.
        press(&mut a, KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(a.screen, Screen::Search);
        press(&mut a, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!a.should_quit, "esc should not quit the search");
        assert_eq!(a.screen, Screen::Item);

        // Help: Esc closes the popup.
        press(&mut a, KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(a.screen, Screen::Help);
        press(&mut a, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!a.should_quit, "esc should not quit the help");
        assert_eq!(a.screen, Screen::Item);

        // q still quits.
        press(&mut a, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(a.should_quit, "q should quit");
    }

    /// n/p and space work on a module page's listings, not just on impls.
    #[test]
    fn module_sections_step_and_fold() {
        let mut a = app();
        let root = a.universe.root().expect("std root");
        a.navigate_to(root);
        assert!(
            !a.page.as_ref().unwrap().sections.is_empty(),
            "std's root page should have sections"
        );

        // n steps to the next section and scrolls it into view.
        press(&mut a, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(a.section_cursor, 1);
        press(&mut a, KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(a.section_cursor, 0);

        // space folds the section under the cursor.
        let before = a.page.as_ref().unwrap().lines.len();
        press(&mut a, KeyCode::Char(' '), KeyModifiers::NONE);
        let after = a.page.as_ref().unwrap().lines.len();
        assert!(after < before, "space should fold ({before} -> {after})");

        // ...and again unfolds it.
        press(&mut a, KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(a.page.as_ref().unwrap().lines.len(), before);
    }

    /// On the search screen `u` is literal text, not a command.
    #[test]
    fn u_types_into_the_search_query() {
        let mut a = app();
        a.screen = Screen::Search;
        a.query.clear();
        press(&mut a, KeyCode::Char('u'), KeyModifiers::NONE);
        assert_eq!(a.query, "u");
    }
}
