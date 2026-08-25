# rustdoc-viewer

A terminal-based viewer for Rust documentation, driven by rustdoc's JSON output.

Browse `std` in the terminal: fuzzy-search every item, read rendered docs with
syntax-highlighted examples, and follow intra-doc links the way you would in a
browser.

## Requirements

The nightly toolchain's `rust-docs-json` component:

```sh
rustup component add rust-docs-json --toolchain nightly
```

## Usage

```sh
cargo run --release
```

The binary is named `rdv`. It opens on the `std` root module; press `/` to
search.

`std`, `core`, and `alloc` are loaded and merged into one universe, so `Vec`
and `Option` are found and shown under their familiar `std::` names rather than
the crates that define them.

## Keys

| Key | Action |
| --- | --- |
| `/` | open search |
| `⏎` | open selected · follow focused link |
| `tab` / `⇧tab` | focus next / previous link |
| `u` | go up to the parent item |
| `^o` / `backspace` | go back |
| `^f` | go forward |
| `j` / `k` / `↑` / `↓` | scroll |
| `^d` / `^u` | half page |
| `g` / `G` | top / bottom |
| `n` / `p` | next / previous section |
| `space` | fold or unfold a section |
| `?` | toggle help |
| `esc` | close search or help |
| `q` | quit |

## License

MIT OR Apache-2.0
