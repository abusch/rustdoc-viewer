//! Shared setup for the tests that read the real rustdoc JSON.
//!
//! These tests assert against `std` itself rather than a fixture, which is the
//! point: the invariants they cover are about real documentation. That makes
//! the `rust-docs-json` component a hard requirement — when it is missing we
//! panic with the install hint rather than skipping, so a test that cannot run
//! is reported as a failure instead of a pass.

use crate::docs::Universe;
use crate::index::SearchIndex;
use crate::load;

/// Load `std`, `core`, and `alloc` into one universe.
///
/// Panics with the install hint when the rustdoc JSON is unavailable.
pub fn universe() -> Universe {
    let crates = match load::load_std_crates() {
        Ok(crates) => crates,
        Err(e) => {
            panic!("these tests need the rustdoc JSON for std, which could not be loaded:\n{e:#}")
        }
    };
    let names = load::STD_CRATES.iter().map(|s| s.to_string()).collect();
    Universe::new(crates, names)
}

/// A universe and its search index, without display paths applied.
pub fn indexed_raw() -> (Universe, SearchIndex) {
    let u = universe();
    let idx = SearchIndex::build(&u);
    (u, idx)
}

/// A universe with its search index, and display paths already applied.
pub fn indexed() -> (Universe, SearchIndex) {
    let mut u = universe();
    let idx = SearchIndex::build(&u);
    u.set_display_paths(idx.display_paths());
    (u, idx)
}
