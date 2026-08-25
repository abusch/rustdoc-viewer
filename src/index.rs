//! The flat, searchable index over the whole universe.

use std::collections::HashMap;

use frizbee::{CaseMatching, Config, Matcher};
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
    ///
    /// Lowercased, because frizbee still docks points for a case mismatch even
    /// with [`CaseMatching::Ignore`], which buries `Vec` ~700 results deep for
    /// the query `vec`. Folding case out of the haystack keeps the fuzzy score
    /// purely about the letters, and leaves case to `rerank_score`.
    names: Vec<String>,
    paths: Vec<String>,
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
                let path = [
                    aliases.get(&canonical).cloned(),
                    std_facing_path(&canonical),
                ]
                .into_iter()
                .flatten()
                .min_by_key(|p| alias_preference(p))
                .unwrap_or(canonical);
                entries.push(Entry {
                    item: ItemRef::new(cid, *id),
                    path,
                    name,
                    kind: summary.kind,
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

        let names = entries.iter().map(|e| e.name.to_lowercase()).collect();
        let paths = entries.iter().map(|e| e.path.to_lowercase()).collect();
        Self {
            entries,
            names,
            paths,
        }
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
    /// Returns indices into [`SearchIndex::entries`], best matches first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<usize> {
        let query = query.trim();
        if query.is_empty() {
            return (0..self.entries.len().min(limit)).collect();
        }

        let by_path = query.contains("::");
        let haystack: &[String] = if by_path { &self.paths } else { &self.names };

        // Match case-insensitively against the lowercased haystack: typing
        // `vec` should find `Vec`. Case is a *ranking* signal instead (see
        // `rerank_score`), so an exact-case hit still wins without an inexact
        // one being dropped out of the running.
        let config = Config::default().casing(CaseMatching::Ignore);
        let lowered = query.to_lowercase();
        let mut matcher = Matcher::new(&lowered, &config);
        let mut matches = matcher.match_list(haystack);

        // frizbee's fuzzy score alone puts `std::vec` (the module) above
        // `std::vec::Vec`, so re-rank a candidate pool with what we know about
        // the items themselves. The pool is a flat cap rather than a multiple
        // of `limit`: the item someone wants can sit surprisingly deep in the
        // fuzzy ordering (`Vec` is ~700th for the query `vec`, behind every
        // `vec_*` internal), and a pool scaled to a small `limit` would never
        // see it.
        matches.truncate(RERANK_POOL);

        let mut scored: Vec<_> = matches
            .into_iter()
            .map(|m| {
                let idx = m.index as usize;
                (rerank_score(query, &self.entries[idx], m.score), idx)
            })
            .collect();

        // Descending score; `entries` is pre-sorted by rank, so the index
        // tiebreak still favours the better item among equals.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored.truncate(limit);
        scored.into_iter().map(|(_, entry_idx)| entry_idx).collect()
    }
}

/// How many of frizbee's top candidates to re-rank.
///
/// Large enough that the intended item is in the pool even when the fuzzy score
/// buries it, small enough that re-ranking stays trivial next to the match
/// itself (which has already scanned every name in std).
const RERANK_POOL: usize = 4096;

/// Score a candidate, higher first.
///
/// frizbee answers "does this string fuzzily contain the query"; it has no way
/// to know that `Vec` the struct is what someone typing `vec` wants, and
/// `std::vec` the module is not. This adds that judgement on top: how squarely
/// the name matches the query, then what kind of item it is and whether it is
/// public API, with the fuzzy score breaking the remaining ties.
fn rerank_score(query: &str, e: &Entry, fuzzy: u16) -> i32 {
    // Weights are spread far enough apart that a stronger name match always
    // beats a better kind, which always beats a better fuzzy score.
    //
    // Case is deliberately *not* a tier of its own here. Rust spells types in
    // `CamelCase` and modules/macros in `snake_case`, so a lowercase query like
    // `vec` matches the macro `vec!` exactly and the struct `Vec` only
    // case-insensitively — ranking case above kind would hand `vec` to the
    // macro, when the type is nearly always what was meant. Case is worth a
    // nudge, not a tier, so it goes in below alongside kind.
    let m = name_match(query, &e.name);
    let name_score = match m {
        NameMatch::Exact | NameMatch::ExactIgnoreCase => 3,
        NameMatch::Prefix | NameMatch::PrefixIgnoreCase => 1,
        NameMatch::Fuzzy => 0,
    };
    let exact_case = matches!(m, NameMatch::Exact | NameMatch::Prefix);

    // `rank` counts penalties (lower is better); flip it into a bonus.
    let rank_score = i32::from(MAX_RANK - rank(e));

    name_score * 10_000 + rank_score * 500 + i32::from(exact_case) * 200
        - obscurity_penalty(e) * 2_000
        + i32::from(fuzzy)
}

/// How squarely the query lines up with an item's bare name.
#[derive(Clone, Copy)]
enum NameMatch {
    Exact,
    ExactIgnoreCase,
    Prefix,
    PrefixIgnoreCase,
    Fuzzy,
}

fn name_match(query: &str, name: &str) -> NameMatch {
    // The query may be a path (`vec::Vec`); only its last segment can match a
    // bare name, and matching that is still a much stronger signal than fuzz.
    let needle = query.rsplit("::").next().unwrap_or(query);
    if needle.is_empty() {
        return NameMatch::Fuzzy;
    }
    if name == needle {
        NameMatch::Exact
    } else if name.eq_ignore_ascii_case(needle) {
        NameMatch::ExactIgnoreCase
    } else if name.starts_with(needle) {
        NameMatch::Prefix
    } else if name.len() >= needle.len() && name[..needle.len()].eq_ignore_ascii_case(needle) {
        NameMatch::PrefixIgnoreCase
    } else {
        NameMatch::Fuzzy
    }
}

/// Penalty for items that are technically in the JSON but are not the public
/// API anyone is searching for.
///
/// std's rustdoc JSON is built with private items included, so internals like
/// `std::sys::*` and `core::core_arch::*` sit in `paths` alongside real API and
/// drown out the results for short queries. They cannot be filtered out
/// structurally — rustdoc marks them `Public` and they are reachable in the
/// module tree — so they are demoted by the module they live in instead.
fn obscurity_penalty(e: &Entry) -> i32 {
    const INTERNAL_MODULES: [&str; 6] = [
        "sys",
        "sys_common",
        "core_arch",
        "stdarch",
        "intrinsics",
        "macros",
    ];

    let mut segments = e.path.split("::");
    let _krate = segments.next();
    let mut penalty = 0;
    for seg in segments {
        // A private-looking module anywhere in the path taints everything below
        // it: `core::core_arch::s390x::vector::sealed::VectorOr` is not API.
        if INTERNAL_MODULES.contains(&seg) || seg == "sealed" {
            penalty += 2;
        }
    }
    penalty
}

/// Largest value [`rank`] can return, so it can be flipped into a bonus.
const MAX_RANK: u8 = 2 * 4 + 3;

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
        let ItemEnum::Use(u) = &item.inner else {
            continue;
        };
        if u.is_glob {
            continue;
        }
        let Some(target) = u.id else { continue };
        // The target id is meaningful only inside std; `paths` translates it.
        let Some(summary) = krate.paths.get(&target) else {
            continue;
        };
        let canonical = summary.path.join("::");

        let Some(parent) = module_path.get(id) else {
            continue;
        };
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
    const MIRRORED: &[&str] = &[
        "alloc",
        "any",
        "array",
        "ascii",
        "borrow",
        "boxed",
        "cell",
        "char",
        "clone",
        "cmp",
        "convert",
        "default",
        "error",
        "f32",
        "f64",
        "ffi",
        "fmt",
        "future",
        "hash",
        "hint",
        "isize",
        "iter",
        "marker",
        "mem",
        "num",
        "ops",
        "option",
        "panic",
        "pin",
        "primitive",
        "ptr",
        "rc",
        "result",
        "slice",
        "str",
        "string",
        "sync",
        "task",
        "time",
        "usize",
        "vec",
    ];

    let (krate, rest) = canonical.split_once("::")?;
    if krate != "core" && krate != "alloc" {
        return None;
    }
    let module = rest.split("::").next()?;
    if MIRRORED.contains(&module) {
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
        let Some(item) = krate.index.get(&mod_id) else {
            continue;
        };
        let ItemEnum::Module(m) = &item.inner else {
            continue;
        };

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

    fn index() -> (Universe, SearchIndex) {
        crate::testdocs::indexed_raw()
    }

    #[test]
    fn well_known_names_rank_first() {
        let (_u, idx) = index();
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
            let top = &idx.entry(hits[0]).path;
            assert_eq!(top, want, "top hit for {query:?}");
        }
    }

    /// Typing all-lowercase is the common case, and it must not cost you the
    /// type you were looking for. Note that several of these queries match a
    /// module or macro of exactly that name (`std::vec`, `vec!`); the type is
    /// still what was meant.
    #[test]
    fn lowercase_queries_find_camel_case_types() {
        let (_u, idx) = index();
        for (query, want) in [
            ("vec", "std::vec::Vec"),
            ("string", "std::string::String"),
            ("option", "std::option::Option"),
            ("result", "std::result::Result"),
            ("hashmap", "std::collections::HashMap"),
            ("btreemap", "std::collections::BTreeMap"),
            ("rc", "std::rc::Rc"),
            ("arc", "std::sync::Arc"),
            ("box", "std::boxed::Box"),
            ("cow", "std::borrow::Cow"),
            ("pathbuf", "std::path::PathBuf"),
            ("refcell", "std::cell::RefCell"),
            ("duration", "std::time::Duration"),
            ("iterator", "std::iter::traits::iterator::Iterator"),
        ] {
            let hits = idx.search(query, 5);
            assert!(!hits.is_empty(), "no hits for {query}");
            let top = &idx.entry(hits[0]).path;
            assert_eq!(top, want, "top hit for {query:?}");
        }
    }

    #[test]
    fn exact_name_beats_prefix_beats_fuzz() {
        let e = |name: &str, kind| Entry {
            item: ItemRef::new(CrateId(0), rustdoc_types::Id(0)),
            path: format!("std::x::{name}"),
            name: name.to_string(),
            kind,
        };
        let vec = e("Vec", ItemKind::Struct);
        let vec_deque = e("VecDeque", ItemKind::Struct);
        // Same kind and same fuzzy score: the exact name must still win.
        assert!(rerank_score("vec", &vec, 60) > rerank_score("vec", &vec_deque, 60));
        // ...even when the prefix match scores much better on fuzz alone.
        assert!(rerank_score("vec", &vec, 10) > rerank_score("vec", &vec_deque, 200));
    }

    #[test]
    fn internals_are_demoted_below_public_api() {
        let e = |path: &str, name: &str| Entry {
            item: ItemRef::new(CrateId(0), rustdoc_types::Id(0)),
            path: path.to_string(),
            name: name.to_string(),
            kind: ItemKind::Struct,
        };
        assert_eq!(obscurity_penalty(&e("std::vec::Vec", "Vec")), 0);
        assert!(obscurity_penalty(&e("std::sys::random::Foo", "Foo")) > 0);
        assert!(
            obscurity_penalty(&e("core::core_arch::s390x::vector::sealed::Vec", "Vec"))
                > obscurity_penalty(&e("std::sys::Vec", "Vec"))
        );
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
        assert_eq!(
            std_facing_path("alloc::rc::Rc").as_deref(),
            Some("std::rc::Rc")
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
