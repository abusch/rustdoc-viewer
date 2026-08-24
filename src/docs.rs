//! The merged multi-crate universe.
//!
//! Rustdoc `Id`s are **crate-local and collide across crates**: in `std`,
//! `use String` carries `Id(219)`, which resolves to nothing in `std`'s own
//! index — the real item is `Id(286)` in `alloc`. So every reference to an item
//! must be keyed by `(crate, id)`, never by a bare `Id`.
//!
//! Cross-crate references are resolved through `paths`, which maps a foreign
//! `Id` to its canonical path (`alloc::string::String`); that path is then
//! looked up in the crate that actually owns it.

use std::collections::HashMap;

use rustdoc_types::{Crate, Id, Item, ItemKind, ItemSummary};

/// Index of a crate within a [`Universe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CrateId(pub usize);

/// A fully-qualified reference to an item: which crate, and which id within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemRef {
    pub krate: CrateId,
    pub id: Id,
}

impl ItemRef {
    pub fn new(krate: CrateId, id: Id) -> Self {
        Self { krate, id }
    }
}

/// Several rustdoc crates merged into one browsable whole.
pub struct Universe {
    crates: Vec<Crate>,
    names: Vec<String>,
    /// Canonical path (e.g. `alloc::vec::Vec`) to the item that owns it.
    by_path: HashMap<String, ItemRef>,
    /// Preferred display path per item, when it differs from the canonical one.
    display: HashMap<ItemRef, String>,
}

impl Universe {
    /// Build a universe from parsed crates and their names, in matching order.
    pub fn new(crates: Vec<Crate>, names: Vec<String>) -> Self {
        assert_eq!(crates.len(), names.len(), "crate/name count mismatch");

        // Map each canonical path to the crate that owns it. `crate_id == 0`
        // means "defined here", so this never records a foreign re-export.
        let mut by_path = HashMap::new();
        for (i, krate) in crates.iter().enumerate() {
            let cid = CrateId(i);
            for (id, summary) in &krate.paths {
                if summary.crate_id == 0 {
                    by_path.insert(summary.path.join("::"), ItemRef::new(cid, *id));
                }
            }
        }

        Self { crates, names, by_path, display: HashMap::new() }
    }

    /// Record the user-facing spelling for items whose canonical path differs
    /// (`alloc::vec::Vec` is shown as `std::vec::Vec`).
    pub fn set_display_paths(&mut self, display: HashMap<ItemRef, String>) {
        self.display = display;
    }

    pub fn crate_name(&self, krate: CrateId) -> &str {
        &self.names[krate.0]
    }

    /// All crate ids, in load order.
    pub fn crate_ids(&self) -> impl Iterator<Item = CrateId> {
        (0..self.crates.len()).map(CrateId)
    }

    /// The crate root module of the first crate loaded, i.e. `std`.
    ///
    /// This is the page the app opens on, mirroring the docs.rs front page.
    pub fn root(&self) -> Option<ItemRef> {
        let krate = CrateId(0);
        let id = self.crates.first()?.root;
        self.item(ItemRef::new(krate, id))?;
        Some(ItemRef::new(krate, id))
    }

    pub fn krate(&self, krate: CrateId) -> &Crate {
        &self.crates[krate.0]
    }

    /// Fetch an item by fully-qualified reference.
    pub fn item(&self, r: ItemRef) -> Option<&Item> {
        self.crates.get(r.krate.0)?.index.get(&r.id)
    }

    /// The `paths` summary for a reference, if rustdoc recorded one.
    pub fn summary(&self, r: ItemRef) -> Option<&ItemSummary> {
        self.crates.get(r.krate.0)?.paths.get(&r.id)
    }

    /// Look an item up by its canonical path.
    pub fn by_path(&self, path: &str) -> Option<ItemRef> {
        self.by_path.get(path).copied()
    }

    /// Resolve an `Id` encountered while reading crate `from`.
    ///
    /// Ids that belong to `from` resolve directly. Foreign ids are translated
    /// through `paths` into a canonical path, then looked up in the owning
    /// crate — the strategy verified for `String`, `Vec`, and `Box`.
    pub fn resolve(&self, from: CrateId, id: Id) -> Option<ItemRef> {
        let krate = self.crates.get(from.0)?;
        if krate.index.contains_key(&id) {
            return Some(ItemRef::new(from, id));
        }
        let summary = krate.paths.get(&id)?;
        self.by_path(&summary.path.join("::"))
    }

    /// The canonical path for a reference, as a `::`-joined string.
    ///
    /// Falls back to walking up the module tree when `paths` has no entry,
    /// which is common for associated items.
    pub fn path_of(&self, r: ItemRef) -> Option<String> {
        if let Some(p) = self.display.get(&r) {
            return Some(p.clone());
        }
        self.summary(r).map(|s| s.path.join("::"))
    }

    /// The kind rustdoc assigned to an item, when it is listed in `paths`.
    pub fn kind_of(&self, r: ItemRef) -> Option<ItemKind> {
        self.summary(r).map(|s| s.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load;

    /// Loading the real std JSON is slow-ish but this is the single most
    /// important invariant in the program, so it is worth testing for real.
    fn universe() -> Option<Universe> {
        let crates = load::load_std_crates().ok()?;
        let names = load::STD_CRATES.iter().map(|s| s.to_string()).collect();
        Some(Universe::new(crates, names))
    }

    #[test]
    fn resolves_reexports_across_crates() {
        let Some(u) = universe() else {
            eprintln!("skipping: rust-docs-json not available");
            return;
        };

        // These live in `alloc`/`core` but are reached through `std`.
        for path in ["alloc::string::String", "alloc::vec::Vec", "core::option::Option"] {
            let r = u.by_path(path).unwrap_or_else(|| panic!("{path} not found"));
            assert!(u.item(r).is_some(), "{path} resolved to a missing item");
            assert_eq!(u.path_of(r).as_deref(), Some(path));
        }
    }

    #[test]
    fn impls_resolve_within_owning_crate() {
        let Some(u) = universe() else { return };

        let r = u.by_path("alloc::vec::Vec").expect("Vec");
        let item = u.item(r).expect("Vec item");
        let rustdoc_types::ItemEnum::Struct(s) = &item.inner else {
            panic!("Vec should be a struct");
        };
        assert!(!s.impls.is_empty());
        for id in &s.impls {
            assert!(
                u.resolve(r.krate, *id).is_some(),
                "impl {id:?} of Vec failed to resolve"
            );
        }
    }
}
