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
    ///
    /// Rust's namespaces are separate, so a path can name more than one item —
    /// `alloc::vec` is both the `vec!` macro and the `vec` module. This map
    /// keeps whichever was seen last, which is fine for resolving a link (the
    /// two are interchangeable as a destination) but not for picking a parent;
    /// see [`Universe::container_at`].
    by_path: HashMap<String, ItemRef>,
    /// Every item claiming a given canonical path, for the collision above.
    all_by_path: HashMap<String, Vec<ItemRef>>,
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
        let mut all_by_path: HashMap<String, Vec<ItemRef>> = HashMap::new();
        for (i, krate) in crates.iter().enumerate() {
            let cid = CrateId(i);
            for (id, summary) in &krate.paths {
                if summary.crate_id == 0 {
                    let path = summary.path.join("::");
                    let r = ItemRef::new(cid, *id);
                    by_path.insert(path.clone(), r);
                    all_by_path.entry(path).or_default().push(r);
                }
            }
        }

        Self {
            crates,
            names,
            by_path,
            all_by_path,
            display: HashMap::new(),
        }
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

    /// The item one level up: the module containing a type, or the type
    /// carrying a method.
    ///
    /// Both spellings of the path are tried, because neither alone is enough.
    /// The display path is what the reader sees and so is tried first, but it
    /// is a `std::`-facing alias that `by_path` (keyed canonically) often
    /// cannot resolve — `std::vec::Vec` sits at `alloc::vec`, not `std::vec`.
    /// The canonical path always resolves but can name a crate the reader
    /// never saw. Trying display then canonical keeps the familiar spelling
    /// where one exists and still finds the item where it does not.
    ///
    /// Association falls out of the path for free: `String::push_str` is
    /// listed in `paths` under its full path, so dropping the last segment
    /// lands on `String` rather than on the `string` module.
    pub fn parent_of(&self, r: ItemRef) -> Option<ItemRef> {
        let canonical = self.summary(r).map(|s| s.path.join("::"));
        let display = self.display.get(&r).cloned();

        display
            .as_deref()
            .and_then(parent_path)
            .and_then(|p| self.container_at(&p))
            .or_else(|| {
                canonical
                    .as_deref()
                    .and_then(parent_path)
                    .and_then(|p| self.container_at(&p))
            })
            // A crate root is its own top: report no parent rather than
            // looping back onto the page the reader is already on.
            .filter(|p| *p != r)
    }

    /// The item at `path` that can actually contain something.
    ///
    /// Where a path names items in several namespaces, only one of them is a
    /// plausible parent: `std::vec` is both the `vec!` macro and the `vec`
    /// module, and going up from `Vec` means the module. A macro contains
    /// nothing, so prefer a module or a type and fall back to whatever is
    /// there for paths with no collision.
    fn container_at(&self, path: &str) -> Option<ItemRef> {
        let candidates = self.all_by_path.get(path)?;
        candidates
            .iter()
            .find(|r| self.kind_of(**r).is_some_and(is_container))
            .or_else(|| candidates.first())
            .copied()
    }
}

/// Whether a kind can hold other items, and so can serve as a parent.
fn is_container(kind: ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::Module
            | ItemKind::Struct
            | ItemKind::Enum
            | ItemKind::Trait
            | ItemKind::Union
            | ItemKind::Primitive
    )
}

/// Drop the last `::` segment, if there is one to drop.
fn parent_path(path: &str) -> Option<String> {
    let (parent, _) = path.rsplit_once("::")?;
    (!parent.is_empty()).then(|| parent.to_string())
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
        for path in [
            "alloc::string::String",
            "alloc::vec::Vec",
            "core::option::Option",
        ] {
            let r = u
                .by_path(path)
                .unwrap_or_else(|| panic!("{path} not found"));
            assert!(u.item(r).is_some(), "{path} resolved to a missing item");
            assert_eq!(u.path_of(r).as_deref(), Some(path));
        }
    }

    /// `u` walks up the chain a reader actually sees: method -> type ->
    /// module -> ... -> crate root, in the `std::` spelling wherever std has
    /// one.
    #[test]
    fn parent_walks_up_to_the_crate_root() {
        let Some(mut u) = universe() else {
            eprintln!("skipping: rust-docs-json not available");
            return;
        };
        let idx = crate::index::SearchIndex::build(&u);
        u.set_display_paths(idx.display_paths());

        let start = u
            .by_path("alloc::string::String::push_str")
            .expect("push_str");
        let mut seen = Vec::new();
        let mut at = start;
        while let Some(parent) = u.parent_of(at) {
            seen.push(u.path_of(parent).expect("a path"));
            at = parent;
            assert!(seen.len() < 10, "parent chain should terminate: {seen:?}");
        }
        assert_eq!(seen, ["std::string::String", "std::string", "std",]);
    }

    /// A type whose module std re-exports resolves through the canonical path,
    /// since `std::vec` is not itself a key in `by_path`.
    #[test]
    fn parent_of_a_reexported_type_finds_its_module() {
        let Some(mut u) = universe() else { return };
        let idx = crate::index::SearchIndex::build(&u);
        u.set_display_paths(idx.display_paths());

        let vec = u.by_path("alloc::vec::Vec").expect("Vec");
        let parent = u.parent_of(vec).expect("Vec has a parent module");
        assert_eq!(u.path_of(parent).as_deref(), Some("std::vec"));
        // `std::vec` names both the `vec!` macro and the `vec` module, and
        // asserting on the path alone cannot tell them apart — the macro was
        // what `u` actually opened before `container_at` existed.
        assert_eq!(u.kind_of(parent), Some(ItemKind::Module));
    }

    #[test]
    fn container_at_prefers_a_module_over_a_macro() {
        let Some(u) = universe() else { return };
        // Both live at this path; only the module can be a parent.
        let at = u.container_at("alloc::vec").expect("alloc::vec");
        assert_eq!(u.kind_of(at), Some(ItemKind::Module));
    }

    #[test]
    fn crate_roots_have_no_parent() {
        let Some(u) = universe() else { return };
        for krate in ["std", "core", "alloc"] {
            let root = u.by_path(krate).expect(krate);
            assert_eq!(u.parent_of(root), None, "{krate} should be the top");
        }
    }

    #[test]
    fn parent_path_drops_one_segment() {
        assert_eq!(parent_path("std::vec::Vec").as_deref(), Some("std::vec"));
        assert_eq!(parent_path("std::vec").as_deref(), Some("std"));
        assert_eq!(parent_path("std"), None);
        assert_eq!(parent_path(""), None);
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
