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

/// The toolchain whose rustdoc JSON we read, overridable for when the current
/// nightly has drifted past the format version we support.
///
/// A `+toolchain` argument takes precedence over `RUSTUP_TOOLCHAIN`, and rustup
/// refuses to alias the reserved name `nightly`, so this is the only way to
/// point the viewer at a specific nightly.
fn toolchain() -> String {
    std::env::var("RDV_TOOLCHAIN").unwrap_or_else(|_| "nightly".to_string())
}

fn install_hint(toolchain: &str) -> String {
    format!("install it with:\n    rustup component add rust-docs-json --toolchain {toolchain}")
}

/// Locate the directory holding the nightly toolchain's rustdoc JSON.
pub fn json_dir() -> Result<PathBuf> {
    let toolchain = toolchain();
    let out = Command::new("rustc")
        .args([&format!("+{toolchain}"), "--print", "sysroot"])
        .output()
        .with_context(|| {
            format!(
                "failed to run `rustc +{toolchain} --print sysroot`; is the {toolchain} toolchain installed?"
            )
        })?;

    if !out.status.success() {
        bail!(
            "`rustc +{toolchain} --print sysroot` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let sysroot = String::from_utf8(out.stdout).context("sysroot path was not valid UTF-8")?;
    let dir = Path::new(sysroot.trim()).join("share/doc/rust/json");

    if !dir.is_dir() {
        bail!(
            "rustdoc JSON not found at {};\nthe `rust-docs-json` component is not installed — {}",
            dir.display(),
            install_hint(&toolchain)
        );
    }
    Ok(dir)
}

/// Parse one crate's JSON, verifying it speaks the format version we compiled against.
fn load_one(dir: &Path, name: &str) -> Result<Crate> {
    let path = dir.join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read {};\nthe `rust-docs-json` component may be incomplete — {}",
            path.display(),
            install_hint(&toolchain())
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
        bail!(
            "{name}.json uses rustdoc JSON format version {found}, but this build expects {ours};\n{advice}"
        );
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
            .map(|h| {
                h.join()
                    .map_err(|_| anyhow!("a crate-loading thread panicked"))?
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_format_version() {
        assert_eq!(extract_format_version(r#"{"format_version":61}"#), Some(61));
        assert_eq!(
            extract_format_version(r#"{"a":1, "format_version" : 42 , "b":2}"#),
            Some(42)
        );
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

#[cfg(test)]
mod toolchain_tests {
    /// The override exists so CI can pin a nightly; make sure the default is
    /// still the plain `nightly` channel.
    #[test]
    fn defaults_to_nightly() {
        // Only meaningful when the caller has not set an override.
        if std::env::var_os("RDV_TOOLCHAIN").is_none() {
            assert_eq!(super::toolchain(), "nightly");
        }
    }
}
