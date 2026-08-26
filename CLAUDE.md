# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A terminal browser for Rust documentation, driven by rustdoc's JSON output. The
binary is `rdv`; it opens on the `std` root module and fuzzy-searches every item.

## Commands

```sh
cargo run --release          # run it (debug is usable; deps build at opt-level 3)
cargo test                   # full suite
cargo test <substring>       # one test, e.g. cargo test page::tests::option_variants_render
cargo test <name> -- --nocapture   # see println! output
cargo fmt --all --check      # CI runs this
cargo clippy --locked --all-targets -- -D warnings   # CI runs this; warnings fail
cargo deny check             # licenses/bans/sources, also a CI job
```

CI additionally runs `cargo test --locked --all-targets`. Clippy is `-D warnings`,
so a warning is a build failure.

### Testing what actually gets drawn

To check rendering — colors, highlights, layout — draw into ratatui's
`TestBackend` and assert on the resulting buffer. **Do not drive the binary in a
pty and parse ANSI escapes**; the frame diffing makes that unreliable, and none
of it is necessary:

```rust
use ratatui::Terminal;
use ratatui::backend::TestBackend;

let mut term = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
term.draw(|f| draw(f, &mut app)).expect("draw");
let buf = term.backend().buffer();

let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
assert_eq!(buf[(6, y)].bg, theme::focused_link().bg.expect("focus bg"));
```

`buf[(x, y)]` gives a `Cell` with `symbol()`, `fg`, `bg`, and modifiers, so
theme colors can be compared against `theme.rs` directly rather than against hex
values. See `ui::tests::an_opened_method_row_is_highlighted_on_screen`.

This is also the only way to catch bugs that need a real frame: it is what
surfaced the focused row scrolling off screen after a width change, which the
model-level state alone looked fine for.

## The rustdoc JSON dependency

Everything downstream reads the `rust-docs-json` component of a *nightly*
toolchain:

```sh
rustup component add rust-docs-json --toolchain nightly
```

`RDV_TOOLCHAIN` selects which toolchain's JSON to read (`RDV_TOOLCHAIN=nightly-2026-08-23 rdv`).
Use it when the current nightly has moved past the format version this build
supports, rather than downgrading the toolchain.

`load.rs` compares the JSON's `format_version` against `rustdoc_types::FORMAT_VERSION`
and refuses to load on a mismatch. **The `rustdoc-types` dependency and
`DOCS_JSON_NIGHTLY` in `.github/workflows/ci.yml` are coupled — bump them
together.** A scheduled canary job files an issue when the latest nightly drifts
past the pin.

Tests assert against real `std` documentation, not fixtures. They **panic rather
than skip** when the component is missing, so a test that cannot run reports as a
failure. This makes the whole suite depend on the component being installed.

## Architecture

Data flows in one direction: `load` → `docs` → `index` → `page` → `ui`.

- **`load.rs`** finds and parses the JSON for `std`, `core`, and `alloc`. This is
  the seam where support for arbitrary crates would slot in.
- **`docs.rs`** — `Universe` merges those three crates into one browsable whole,
  and owns all cross-crate identity. Items are addressed by `ItemRef`
  (crate + id), never by bare `Id`, because ids are only meaningful within their
  crate; `resolve` translates a foreign id through its canonical path.
- **`index.rs`** — `SearchIndex` is the flat fuzzy-searchable view (frizbee).
- **`page.rs`** builds a `Page`: styled lines plus `Target`s and foldable sections.
- **`render.rs`** turns rustdoc markdown into styled lines and syntax-highlights
  code via arborium.
- **`app.rs`** holds all state and navigation; **`ui.rs`** only draws. `ui.rs`
  should stay free of decisions about *what* to show.
- **`theme.rs`** is the single styling vocabulary — see below.

### Three ideas worth knowing before editing

**Display paths vs. canonical paths.** `Vec` really lives at `alloc::vec::Vec`
but must be *shown* as `std::vec::Vec`. `main` builds the index, then calls
`universe.set_display_paths(index.display_paths())` to install the reader-facing
spelling. Both spellings exist for most items, and lookups often have to try each
— `by_path` is keyed canonically, so a display path will not resolve against it.

**Not every item is in `paths`.** Trait-impl members (`String`'s `clone`) are
absent from rustdoc's `paths` map entirely: no path, no kind, so `kind_of`,
`path_of`, and `parent_of` all return `None` for them. `Universe::owner_of`
covers this via a reverse index built by walking every impl's `items` at load.
Anything reasoning about associated items needs both routes.

**Methods have no page of their own.** Opening one navigates to its owning type
and focuses the row declaring it (`App::member_of` / `reveal` / `focus_on`),
unfolding the impl section that holds it. Note a method and a free function are
both `ItemKind::Function` — only the parent distinguishes them (`String::push_str`
hangs off a type, `mem::swap` off a module).

Two related traps: an item can be named *twice* on a page, by the row declaring
it and by an intra-doc link pointing at it, so target lookup prefers the
declaration (targets owning a whole line have `spans: None`; links carry a
range). And `rebuild_page` re-wraps at a new width, moving every line, so a
focused target must be followed to its new position or it scrolls off screen.

### Section folding

`HistoryEntry.toggled` records sections flipped *away from their default*, not
absolute state; read it through `page::is_expanded`. Trait/auto/blanket impl
sections start folded, so a freshly built page often does not contain a target
until its section is opened.

## Styling

All styling goes through `theme.rs`, which resolves opaline's semantic tokens
(`accent.primary`, `text.dim`, `bg.highlight`) and named styles (`dimmed`,
`inline_code`, `focused_border`). **Do not write `Color::*` literals in UI code**
— add a named style to `theme.rs` instead. Both opaline and arborium default to
Catppuccin Mocha.

Opaline identifies builtin themes by *kebab*-case id (`catppuccin-mocha`), while
arborium's function is `catppuccin_mocha()`. Getting the id wrong falls back to
opaline's default silently — everything still renders, just in the wrong colors,
which is why `theme.rs` has a test asserting the named theme loads.

Themes load at runtime, so styles are functions, not `const`s.

## Conventions

- **Conventional Commits** (`feat:`, `fix:`, `ci:`, `test:`). release-plz parses
  these prefixes to build the changelog and drive versioning, so the prefix
  matters. `style`/`test`/`chore`/`ci` are skipped in the changelog.
- This repo is **jj-backed** (`.jj/` alongside `.git/`). jj auto-snapshots the
  working copy into the current commit, so new edits may land inside an existing
  described commit rather than a fresh one — check `jj st` before assuming.
- Comments explain *why*, not what. Existing prose is dense and specific about
  rationale (see the doc comments in `docs.rs` and `index.rs`); match that
  register rather than adding narration.
