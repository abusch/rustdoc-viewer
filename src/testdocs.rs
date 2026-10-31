//! Shared setup for the tests that read the real rustdoc JSON.
//!
//! These tests assert against `std` itself rather than a fixture, which is the
//! point: the invariants they cover are about real documentation. That makes
//! the `rust-docs-json` component a hard requirement — when it is missing we
//! panic with the install hint rather than skipping, so a test that cannot run
//! is reported as a failure instead of a pass.

use crate::app::App;
use crate::docs::Universe;
use crate::index::SearchIndex;
use crate::load;

/// Load `std`, `core`, and `alloc` into one universe.
///
/// Panics with the install hint when the rustdoc JSON is unavailable.
pub fn universe() -> Universe {
    match load::load_std_crates() {
        Ok(crates) => Universe::new(crates),
        Err(e) => {
            panic!("these tests need the rustdoc JSON for std, which could not be loaded:\n{e:#}")
        }
    }
}

/// A universe and its search index.
pub fn indexed() -> (Universe, SearchIndex) {
    let u = universe();
    let idx = SearchIndex::build(&u);
    (u, idx)
}

/// An app on its landing page.
pub fn app() -> App {
    let (u, idx) = indexed();
    App::new(u, idx)
}

/// An app showing the item at canonical `path`.
pub fn app_on(path: &str) -> App {
    let mut a = app();
    let r = a.universe.by_path(path).expect(path);
    a.navigate_to(r);
    a
}
