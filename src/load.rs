//! Locating and parsing rustdoc JSON.
//!
//! This module is the seam where support for arbitrary crates (downloaded and
//! cached from <https://docs.rs/about/rustdoc-json>) would slot in: everything
//! downstream works off the parsed [`Crate`] values this produces.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use rustdoc_types::Crate;

/// The std-library crates we load and merge into a single universe.
///
/// `std` alone is not enough: `String` and `Vec` live in `alloc`, `Option` in
/// `core`, and `std` merely re-exports them.
pub const STD_CRATES: [&str; 3] = ["std", "core", "alloc"];

const INSTALL_HINT: &str = "install it with:\n    rustup component add rust-docs-json --toolchain nightly";

/// Locate the directory holding the nightly toolchain's rustdoc JSON.
pub fn json_dir() -> Result<PathBuf> {
    let out = Command::new("rustc")
        .args(["+nightly", "--print", "sysroot"])
        .output()
        .context("failed to run `rustc +nightly --print sysroot`; is the nightly toolchain installed?")?;

    if !out.status.success() {
        bail!(
            "`rustc +nightly --print sysroot` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let sysroot = String::from_utf8(out.stdout).context("sysroot path was not valid UTF-8")?;
    let dir = Path::new(sysroot.trim()).join("share/doc/rust/json");

    if !dir.is_dir() {
        bail!(
            "rustdoc JSON not found at {};\nthe `rust-docs-json` component is not installed — {INSTALL_HINT}",
            dir.display()
        );
    }
    Ok(dir)
}

/// Parse one crate's JSON, verifying it speaks the format version we compiled against.
fn load_one(dir: &Path, name: &str) -> Result<Crate> {
    let path = dir.join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read {};\nthe `rust-docs-json` component may be incomplete — {INSTALL_HINT}",
            path.display()
        )
    })?;

    // Check the version before deserializing, so a format bump produces a clear
    // diagnostic rather than an obscure serde error deep in the tree.
    check_format_version(&text, name)?;

    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

/// Pull `format_version` out of the raw JSON and compare it to ours.
fn check_format_version(text: &str, name: &str) -> Result<()> {
    let found = extract_format_version(text)
        .ok_or_else(|| anyhow!("{name}.json has no `format_version` field; it may be corrupt"))?;

    let ours = rustdoc_types::FORMAT_VERSION;
    if found != ours {
        let advice = if found > ours {
            "the nightly toolchain is newer than this tool; update the `rustdoc-types` dependency"
        } else {
            "the nightly toolchain is older than this tool; update it with `rustup update nightly`"
        };
        bail!("{name}.json uses rustdoc JSON format version {found}, but this build expects {ours};\n{advice}");
    }
    Ok(())
}

/// Find `"format_version": N` without deserializing the whole document.
fn extract_format_version(text: &str) -> Option<u32> {
    let rest = text.find(r#""format_version""#).map(|i| &text[i..])?;
    let rest = rest.split_once(':')?.1;
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Load and parse `std`, `core`, and `alloc` concurrently.
///
/// Returns them in the order given by [`STD_CRATES`].
pub fn load_std_crates() -> Result<Vec<Crate>> {
    let dir = json_dir()?;

    std::thread::scope(|scope| {
        let handles: Vec<_> = STD_CRATES
            .iter()
            .map(|name| scope.spawn(|| load_one(&dir, name)))
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().map_err(|_| anyhow!("a crate-loading thread panicked"))?)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_format_version() {
        assert_eq!(extract_format_version(r#"{"format_version":61}"#), Some(61));
        assert_eq!(extract_format_version(r#"{"a":1, "format_version" : 42 , "b":2}"#), Some(42));
        assert_eq!(extract_format_version(r#"{"root":"x"}"#), None);
    }

    #[test]
    fn rejects_mismatched_version() {
        // A version far from ours must be rejected regardless of direction.
        let text = r#"{"format_version":1}"#;
        assert!(check_format_version(text, "std").is_err());
    }

    #[test]
    fn accepts_our_version() {
        let text = format!(r#"{{"format_version":{}}}"#, rustdoc_types::FORMAT_VERSION);
        assert!(check_format_version(&text, "std").is_ok());
    }
}
