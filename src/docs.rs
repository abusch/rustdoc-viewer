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

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use rustdoc_types::{
    Attribute, Crate, Id, Item, ItemEnum, ItemKind, ItemSummary, Type, Visibility,
};

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
    /// Canonical path (e.g. `alloc::vec::Vec`) to every item claiming it.
    ///
    /// Rust's namespaces are separate, so a path can name more than one item —
    /// `alloc::vec` is both the `vec!` macro and the `vec` module. Resolving a
    /// link takes whichever was seen last, since the two are interchangeable
    /// as a destination, but picking a parent cannot; see
    /// [`Universe::container_at`].
    by_path: HashMap<String, Vec<ItemRef>>,
    /// The `std::` spelling of each item, where it differs from the canonical
    /// one: `alloc::vec::Vec` is shown as `std::vec::Vec`.
    display: HashMap<ItemRef, String>,
    /// The type each associated item is declared on, via the impl listing it.
    ///
    /// Trait-impl members such as `String`'s `clone` are absent from `paths`
    /// entirely, so they have neither a path nor a kind to reason about; this
    /// is the only way to get from one back to the type that carries it.
    owner: HashMap<ItemRef, ItemRef>,
}

impl Universe {
    /// Build a universe from parsed crates, each paired with its name.
    ///
    /// The first crate is the one the app opens on; see [`Universe::root`].
    pub fn new(crates: Vec<(String, Crate)>) -> Self {
        let (names, crates): (Vec<String>, Vec<Crate>) = crates.into_iter().unzip();

        // Map each canonical path to the crate that owns it. `crate_id == 0`
        // means "defined here", so this never records a foreign re-export.
        let mut by_path: HashMap<String, Vec<ItemRef>> = HashMap::new();
        for (i, krate) in crates.iter().enumerate() {
            let cid = CrateId(i);
            for (id, summary) in &krate.paths {
                if summary.crate_id == 0 {
                    let path = summary.path.join("::");
                    by_path
                        .entry(path)
                        .or_default()
                        .push(ItemRef::new(cid, *id));
                }
            }
        }

        // Walk every impl so its members can be traced back to the type they
        // are shown under, which `paths` does not record.
        let mut owner = HashMap::new();
        for (i, krate) in crates.iter().enumerate() {
            let cid = CrateId(i);
            for item in krate.index.values() {
                let ItemEnum::Impl(im) = &item.inner else {
                    continue;
                };
                let Some(for_) = impl_self_id(&im.for_) else {
                    continue;
                };
                let owner_ref = ItemRef::new(cid, for_);
                for member in &im.items {
                    owner.insert(ItemRef::new(cid, *member), owner_ref);
                }
            }
        }

        let mut universe = Self {
            crates,
            names,
            by_path,
            display: HashMap::new(),
            owner,
        };
        universe.display = universe.display_paths();
        universe
    }

    /// The public spelling of every item that has one other than its
    /// canonical path.
    ///
    /// Readers know `Vec` as `std::vec::Vec`, not `alloc::vec::Vec`, so this
    /// is the name search shows and pages are titled with. Items take the path
    /// the first crate to expose them gives them — `std` where it does, their
    /// own crate's public path where `std` only points at them; anything else
    /// — a method, an internal — is spelled through its nearest ancestor that
    /// has one.
    fn display_paths(&self) -> HashMap<ItemRef, String> {
        let mut reachable = HashMap::new();
        for cid in self.crate_ids() {
            for (r, shown) in self.namespace(cid) {
                reachable.entry(r).or_insert(shown);
            }
        }
        let mut ancestors = HashMap::new();
        for (r, shown) in &reachable {
            if let Some(s) = self.summary(*r) {
                prefer(&mut ancestors, s.path.join("::"), shown.clone());
            }
        }

        let mut out = HashMap::new();
        for cid in self.crate_ids() {
            for (id, summary) in &self.krate(cid).paths {
                if summary.crate_id != 0 {
                    continue;
                }
                let r = ItemRef::new(cid, *id);
                let canonical = summary.path.join("::");
                let shown = match reachable.get(&r) {
                    Some(shown) => Some(shown.clone()),
                    None => beneath(&canonical, &ancestors),
                };
                if let Some(shown) = shown
                    && shown != canonical
                {
                    out.insert(r, shown);
                }
            }
        }
        out
    }

    /// Every item reachable through a crate's public modules, under the
    /// shortest path that reaches it.
    ///
    /// Re-exports are followed through every crate, because that is how the
    /// names are built: `Formatter` is defined at `core::fmt::Formatter`,
    /// re-exported by `alloc::fmt`, which `std` re-exports as `std::fmt`.
    /// Following them rather than mapping module names is also what keeps
    /// this right as `core` and `alloc` reorganise their internals.
    fn namespace(&self, krate: CrateId) -> HashMap<ItemRef, String> {
        let mut shown = HashMap::new();
        let root = ItemRef::new(krate, self.krate(krate).root);
        let mut stack = vec![(root, self.crate_name(krate).to_string())];
        let mut visited = HashSet::new();

        while let Some((module, path)) = stack.pop() {
            if !visited.insert((module, path.clone())) {
                continue;
            }
            let Some(ItemEnum::Module(m)) = self.item(module).map(|i| &i.inner) else {
                continue;
            };
            for id in &m.items {
                let Some(child) = self.item(ItemRef::new(module.krate, *id)) else {
                    continue;
                };
                if matches!(
                    child.visibility,
                    Visibility::Crate | Visibility::Restricted { .. }
                ) {
                    continue;
                }
                let (target, at) = match &child.inner {
                    // `#[doc(no_inline)]` is rustdoc's own word that a
                    // re-export is a pointer, not a home: the preludes are
                    // written this way, and the website gives such an item
                    // no page there.
                    ItemEnum::Use(_) if is_no_inline(child) => continue,
                    ItemEnum::Use(u) => {
                        let Some(target) = u.id.and_then(|id| self.resolve(module.krate, id))
                        else {
                            continue;
                        };
                        // A glob pours the target's contents into this module:
                        // `pub use alloc_crate::alloc::*` is what fills
                        // `std::alloc`, so that is also the name the globbed
                        // module goes by. (An enum's variants can be globbed
                        // too, and the enum is not renamed by it.)
                        if u.is_glob {
                            if self.is_module(target) {
                                prefer(&mut shown, target, path.clone());
                            }
                            stack.push((target, path.clone()));
                            continue;
                        }
                        (target, format!("{path}::{}", u.name))
                    }
                    _ => {
                        let Some(name) = &child.name else { continue };
                        (ItemRef::new(module.krate, *id), format!("{path}::{name}"))
                    }
                };
                // Only a module reached by a better path is worth walking
                // again, which is also what stops re-export cycles.
                if prefer(&mut shown, target, at.clone()) && self.is_module(target) {
                    stack.push((target, at));
                }
            }
        }
        shown
    }

    fn is_module(&self, r: ItemRef) -> bool {
        matches!(self.item(r).map(|i| &i.inner), Some(ItemEnum::Module(_)))
    }

    /// The type an associated item is declared on, if it is one.
    pub fn owner_of(&self, r: ItemRef) -> Option<ItemRef> {
        self.owner.get(&r).copied()
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
    #[cfg(test)]
    pub fn by_path(&self, path: &str) -> Option<ItemRef> {
        self.by_path.get(path)?.last().copied()
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
        // The foreign path can name items in several namespaces (`alloc::vec`
        // is a macro and a module); the summary's kind says which was meant.
        let summary = krate.paths.get(&id)?;
        let candidates = self.by_path.get(&summary.path.join("::"))?;
        candidates
            .iter()
            .find(|r| self.kind_of(**r) == Some(summary.kind))
            .or(candidates.last())
            .copied()
    }

    /// The path to show for a reference: `std`'s spelling where it has one,
    /// the canonical path otherwise.
    ///
    /// `None` for items absent from `paths`, such as trait-impl members.
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
        let candidates = self.by_path.get(path)?;
        candidates
            .iter()
            .find(|r| self.kind_of(**r).is_some_and(is_container))
            .or_else(|| candidates.first())
            .copied()
    }
}

/// Spell a canonical path through its deepest ancestor that has a display
/// path: methods of `core::option::Option` sit under `std::option::Option`.
fn beneath(canonical: &str, ancestors: &HashMap<String, String>) -> Option<String> {
    canonical.rmatch_indices("::").find_map(|(i, _)| {
        let shown = ancestors.get(&canonical[..i])?;
        Some(format!("{shown}{}", &canonical[i..]))
    })
}

/// Record `path` for `key` unless a shorter spelling is already there, and
/// report whether it was recorded.
///
/// An item can be exposed in more than one place — `HashMap` has a page at
/// `std::collections` and at `std::collections::hash_map` — and the shorter
/// is the one people write.
fn prefer<K: Hash + Eq>(map: &mut HashMap<K, String>, key: K, path: String) -> bool {
    match map.entry(key) {
        Entry::Occupied(mut e) => {
            let better = path.len() < e.get().len();
            if better {
                e.insert(path);
            }
            better
        }
        Entry::Vacant(e) => {
            e.insert(path);
            true
        }
    }
}

/// Whether a re-export is marked `#[doc(no_inline)]`.
fn is_no_inline(item: &Item) -> bool {
    item.attrs
        .iter()
        .any(|a| matches!(a, Attribute::Other(s) if s == "#[doc(no_inline)]"))
}

/// The id of the type an impl is written for, when it names one.
fn impl_self_id(t: &Type) -> Option<Id> {
    match t {
        Type::ResolvedPath(p) => Some(p.id),
        // `impl Trait for &T` and friends are shown on `T`'s page.
        Type::BorrowedRef { type_, .. } => impl_self_id(type_),
        _ => None,
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
    /// Loading the real std JSON is slow-ish but this is the single most
    /// important invariant in the program, so it is worth testing for real.
    use crate::testdocs::universe;

    #[test]
    fn resolves_reexports_across_crates() {
        let u = universe();

        // These live in `alloc`/`core` but are reached, and shown, through `std`.
        for (path, shown) in [
            ("alloc::string::String", "std::string::String"),
            ("alloc::vec::Vec", "std::vec::Vec"),
            ("core::option::Option", "std::option::Option"),
        ] {
            let r = u
                .by_path(path)
                .unwrap_or_else(|| panic!("{path} not found"));
            assert!(u.item(r).is_some(), "{path} resolved to a missing item");
            assert_eq!(u.path_of(r).as_deref(), Some(shown));
        }
    }

    /// What std does not expose by name is spelled through the nearest
    /// ancestor it does, however deep that ancestor sits in its own crate.
    #[test]
    fn items_take_the_std_name_of_their_nearest_exposed_ancestor() {
        let ancestors = HashMap::from([
            ("alloc::rcs::rc".to_string(), "std::rc".to_string()),
            (
                "core::option::Option".to_string(),
                "std::option::Option".to_string(),
            ),
        ]);
        assert_eq!(
            beneath("alloc::rcs::rc::Rc", &ancestors).as_deref(),
            Some("std::rc::Rc")
        );
        assert_eq!(
            beneath("core::option::Option::map", &ancestors).as_deref(),
            Some("std::option::Option::map")
        );
        // Nothing above it is exposed, so there is nothing to spell it with.
        assert_eq!(beneath("core::core_arch::x86::foo", &ancestors), None);
    }

    /// `u` walks up the chain a reader actually sees: method -> type ->
    /// module -> ... -> crate root, in the `std::` spelling wherever std has
    /// one.
    #[test]
    fn parent_walks_up_to_the_crate_root() {
        let u = universe();

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
        let u = universe();

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
        let u = universe();
        // Both live at this path; only the module can be a parent.
        let at = u.container_at("alloc::vec").expect("alloc::vec");
        assert_eq!(u.kind_of(at), Some(ItemKind::Module));
    }

    #[test]
    fn crate_roots_have_no_parent() {
        let u = universe();
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
        let u = universe();

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
