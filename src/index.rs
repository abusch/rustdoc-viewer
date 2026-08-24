//! The flat, searchable index over the whole universe.

use std::collections::HashMap;

use frizbee::{Config, Matcher};
use rustdoc_types::{ItemEnum, ItemKind};

use crate::docs::{CrateId, ItemRef, Universe};

/// One searchable entry: an item, and the path we show for it.
pub struct Entry {
    pub item: ItemRef,
    /// Display path, preferring the `std::`-facing name for re-exports.
    pub path: String,
    /// Final path segment — what most searches are actually aiming at.
    pub name: String,
    pub kind: ItemKind,
}

pub struct SearchIndex {
    entries: Vec<Entry>,
    /// Parallel to `entries`; kept separate so frizbee can match a `&[String]`.
    names: Vec<String>,
    paths: Vec<String>,
}

/// A single search hit, identified by its index into [`SearchIndex::entries`].
pub struct Hit {
    pub entry_idx: usize,
}

impl SearchIndex {
    pub fn build(universe: &Universe) -> Self {
        let aliases = std_aliases(universe);

        let mut entries: Vec<Entry> = Vec::new();
        for cid in universe.crate_ids() {
            let krate = universe.krate(cid);
            for (id, summary) in &krate.paths {
                // `crate_id == 0` means the crate owns this item, so each item
                // is indexed exactly once across the whole universe.
                if summary.crate_id != 0 {
                    continue;
                }
                let canonical = summary.path.join("::");
                let Some(name) = summary.path.last().cloned() else {
                    continue;
                };
                // Prefer the familiar `std::` spelling when std re-exports it.
                // Both sources can name the same item; pick the better spelling.
                let path = [aliases.get(&canonical).cloned(), std_facing_path(&canonical)]
                    .into_iter()
                    .flatten()
                    .min_by_key(|p| alias_preference(p))
                    .unwrap_or(canonical);
                entries.push(Entry {
                    item: ItemRef::new(cid, *id),
                    path,
                    name,
                    kind: summary.kind.clone(),
                });
            }
        }

        // A stable, sensible base order: better-ranked items win score ties.
        entries.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.path.len().cmp(&b.path.len()))
                .then_with(|| a.path.cmp(&b.path))
        });

        let names = entries.iter().map(|e| e.name.clone()).collect();
        let paths = entries.iter().map(|e| e.path.clone()).collect();
        Self { entries, names, paths }
    }

    pub fn entry(&self, idx: usize) -> &Entry {
        &self.entries[idx]
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The display path chosen for each item, for the universe to reuse.
    pub fn display_paths(&self) -> HashMap<ItemRef, String> {
        self.entries
            .iter()
            .map(|e| (e.item, e.path.clone()))
            .collect()
    }

    /// Fuzzy-search the index, best matches first.
    ///
    /// A query containing `::` is matched against full paths; otherwise it is
    /// matched against bare names, which is what makes searching `push_str`
    /// behave the way you would expect.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let query = query.trim();
        if query.is_empty() {
            return (0..self.entries.len().min(limit))
                .map(|entry_idx| Hit { entry_idx })
                .collect();
        }

        let by_path = query.contains("::");
        let haystack: &[String] = if by_path { &self.paths } else { &self.names };

        let config = Config::default();
        let mut matcher = Matcher::new(query, &config);
        let mut matches = matcher.match_list(haystack);

        // frizbee sorts by score then original index; because `entries` is
        // pre-sorted by rank, that tiebreak already favours the better item.
        matches.truncate(limit);
        matches
            .into_iter()
            .map(|m| Hit { entry_idx: m.index as usize })
            .collect()
    }
}

/// Ranking bucket: lower sorts first among equally-scoring matches.
fn rank(e: &Entry) -> u8 {
    let depth_penalty = match e.path.split("::").next() {
        // `std::`-facing names are what most people mean.
        Some("std") => 0,
        Some("alloc") => 1,
        _ => 2,
    };
    // Prefer types and traits over the sea of free functions in `core`.
    let kind_penalty = match e.kind {
        ItemKind::Struct | ItemKind::Enum | ItemKind::Trait | ItemKind::Union => 0,
        ItemKind::Function | ItemKind::Macro | ItemKind::TypeAlias | ItemKind::Primitive => 1,
        ItemKind::Module => 2,
        _ => 3,
    };
    depth_penalty * 4 + kind_penalty
}

/// Map canonical paths to the `std::` path that re-exports them.
///
/// Walks `std`'s `Use` items; each names a target whose id is crate-local to
/// `std` but whose `paths` entry gives the canonical foreign path.
fn std_aliases(universe: &Universe) -> HashMap<String, String> {
    let mut aliases: HashMap<String, String> = HashMap::new();

    let Some(std_id) = universe
        .crate_ids()
        .find(|c| universe.crate_name(*c) == "std")
    else {
        return aliases;
    };
    let krate = universe.krate(std_id);

    // Module paths within std, so a re-export can be given its full std path.
    let module_path = std_module_paths(universe, std_id);

    for (id, item) in &krate.index {
        let ItemEnum::Use(u) = &item.inner else { continue };
        if u.is_glob {
            continue;
        }
        let Some(target) = u.id else { continue };
        // The target id is meaningful only inside std; `paths` translates it.
        let Some(summary) = krate.paths.get(&target) else { continue };
        let canonical = summary.path.join("::");

        let Some(parent) = module_path.get(id) else { continue };
        let alias = if parent.is_empty() {
            format!("std::{}", u.name)
        } else {
            format!("std::{}::{}", parent, u.name)
        };

        // Prefer the canonical home (`std::vec::Vec`) over re-export shims
        // like `std::prelude::v1::Vec`, then prefer the shorter path.
        aliases
            .entry(canonical)
            .and_modify(|existing| {
                if alias_preference(&alias) < alias_preference(existing) {
                    *existing = alias.clone();
                }
            })
            .or_insert(alias);
    }

    aliases
}

/// Rewrite a `core::`/`alloc::` path to the `std::` path that mirrors it.
///
/// `std` re-exports whole modules of `core` and `alloc` with `pub use`, but
/// rustdoc's JSON records those as glob re-exports whose module trees are not
/// reachable from `std`'s root — so the mapping cannot be derived structurally
/// and is listed here instead. These module names are long-standing stable
/// public API, so the list is stable too.
fn std_facing_path(canonical: &str) -> Option<String> {
    const MIRRORED: [&str; 34] = [
        "alloc", "any", "array", "ascii", "borrow", "boxed", "cell", "char", "clone", "cmp",
        "convert", "default", "error", "f32", "f64", "ffi", "fmt", "future", "hash", "hint",
        "isize", "iter", "marker", "mem", "num", "ops", "option", "panic", "pin", "primitive",
        "ptr", "result", "slice", "str",
    ];
    // A second list keeps the first within a fixed-size array literal.
    const MIRRORED_EXTRA: [&str; 6] = ["string", "sync", "task", "time", "usize", "vec"];

    let (krate, rest) = canonical.split_once("::")?;
    if krate != "core" && krate != "alloc" {
        return None;
    }
    let module = rest.split("::").next()?;
    if MIRRORED.contains(&module) || MIRRORED_EXTRA.contains(&module) {
        Some(format!("std::{rest}"))
    } else {
        None
    }
}

/// Sort key deciding which of several std spellings to display.
///
/// `std::prelude::*` and `std::simd::prelude::*` re-export half the library
/// under a shorter path, so path length alone picks the wrong name.
fn alias_preference(path: &str) -> (u8, usize) {
    let bucket = if path.contains("::prelude::") { 1 } else { 0 };
    (bucket, path.len())
}

/// For each item in std, the `::`-joined module path containing it (minus `std::`).
fn std_module_paths(universe: &Universe, std_id: CrateId) -> HashMap<rustdoc_types::Id, String> {
    let krate = universe.krate(std_id);
    let mut out = HashMap::new();
    let mut stack = vec![(krate.root, String::new())];

    while let Some((mod_id, prefix)) = stack.pop() {
        let Some(item) = krate.index.get(&mod_id) else { continue };
        let ItemEnum::Module(m) = &item.inner else { continue };

        for child in &m.items {
            out.insert(*child, prefix.clone());
            if let Some(c) = krate.index.get(child)
                && matches!(c.inner, ItemEnum::Module(_))
                && let Some(name) = &c.name
            {
                let next = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}::{name}")
                };
                stack.push((*child, next));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load;

    fn index() -> Option<(Universe, SearchIndex)> {
        let crates = load::load_std_crates().ok()?;
        let names = load::STD_CRATES.iter().map(|s| s.to_string()).collect();
        let u = Universe::new(crates, names);
        let i = SearchIndex::build(&u);
        Some((u, i))
    }

    #[test]
    fn well_known_names_rank_first() {
        let Some((_u, idx)) = index() else {
            eprintln!("skipping: rust-docs-json not available");
            return;
        };
        // The top hit for these should be the obvious std item, spelled the
        // way the website spells it — not a `prelude::v1` re-export.
        for (query, want) in [
            ("String", "std::string::String"),
            ("Vec", "std::vec::Vec"),
            ("Option", "std::option::Option"),
            ("HashMap", "std::collections::HashMap"),
            ("push_str", "std::string::String::push_str"),
        ] {
            let hits = idx.search(query, 5);
            assert!(!hits.is_empty(), "no hits for {query}");
            let top = &idx.entry(hits[0].entry_idx).path;
            assert_eq!(top, want, "top hit for {query:?}");
        }
    }

    #[test]
    fn mirrors_core_and_alloc_modules_onto_std() {
        assert_eq!(
            std_facing_path("alloc::vec::Vec").as_deref(),
            Some("std::vec::Vec")
        );
        assert_eq!(
            std_facing_path("core::option::Option").as_deref(),
            Some("std::option::Option")
        );
        // Internals that std does not re-export keep their own name.
        assert_eq!(std_facing_path("core::core_arch::x86::foo"), None);
        assert_eq!(std_facing_path("std::fs::File"), None);
    }

    #[test]
    fn prefers_canonical_module_over_prelude() {
        assert!(alias_preference("std::vec::Vec") < alias_preference("std::prelude::v1::Vec"));
    }
}
