//! Theming, resolved from an [`opaline`] token theme.
//!
//! Opaline resolves in three layers — palette hex values, semantic tokens, then
//! named styles — and every builtin theme defines the same contract, so the
//! screens below name tokens rather than colors and follow whatever theme is
//! active. Themes load at runtime, so these are functions rather than the
//! `const` styles they replace; each is a couple of hash lookups against the
//! process-wide theme, which is cheap enough to call per rendered span.

use opaline::names::{styles, tokens};
use opaline::{Theme, load_by_name, set_theme};
use ratatui::style::{Modifier, Style};

/// The theme used unless the app is told otherwise.
///
/// Opaline identifies builtins by kebab-case id, not by the underscored name
/// of the arborium function that produces the matching syntax theme.
pub const DEFAULT_THEME: &str = "catppuccin-mocha";

/// Install the default theme as the process-wide active theme.
///
/// Falls back to opaline's own default if the named theme is somehow missing,
/// since a theme that failed to load is no reason to refuse to start.
pub fn init() {
    if let Some(theme) = load_by_name(DEFAULT_THEME) {
        set_theme(theme);
    }
}

/// The active theme.
fn theme() -> std::sync::Arc<Theme> {
    opaline::current()
}

/// A named style from the theme's `[styles]` table.
fn style(name: &str) -> Style {
    theme().style(name).into()
}

/// A foreground style built from a semantic color token.
fn fg(token: &str) -> Style {
    Style::new().fg(theme().color(token).into())
}

/// A background style built from a semantic color token.
fn bg(token: &str) -> Style {
    Style::new().bg(theme().color(token).into())
}

// ── Page text ────────────────────────────────────────────────────────────

/// An item's title.
pub fn title() -> Style {
    fg(tokens::ACCENT_PRIMARY).add_modifier(Modifier::BOLD)
}

/// A section heading within an item page.
pub fn section() -> Style {
    fg(tokens::ACCENT_SECONDARY).add_modifier(Modifier::BOLD)
}

/// A type or function signature.
pub fn signature() -> Style {
    fg(tokens::TEXT_PRIMARY)
}

/// Secondary text: paths, counts, and other supporting detail.
pub fn dim() -> Style {
    style(styles::DIMMED)
}

/// Muted text, a step brighter than [`dim`].
pub fn muted() -> Style {
    style(styles::MUTED)
}

/// A deprecation notice.
pub fn deprecated() -> Style {
    style(styles::ERROR_STYLE)
}

/// An unstable-feature notice: a caveat rather than a defect, so it reads a
/// step softer than [`deprecated`].
pub fn unstable() -> Style {
    style(styles::WARNING_STYLE)
}

// ── Markdown ─────────────────────────────────────────────────────────────

/// A markdown heading inside rendered docs.
pub fn heading() -> Style {
    fg(tokens::WARNING).add_modifier(Modifier::BOLD)
}

/// Inline `code` spans.
pub fn inline_code() -> Style {
    style(styles::INLINE_CODE)
}

/// An intra-doc link.
pub fn link() -> Style {
    fg(tokens::INFO).add_modifier(Modifier::UNDERLINED)
}

/// A block quote.
pub fn quote() -> Style {
    style(styles::DIMMED).add_modifier(Modifier::ITALIC)
}

// ── Chrome ───────────────────────────────────────────────────────────────

/// The accent used for borders, markers, and key hints.
pub fn accent() -> Style {
    fg(tokens::ACCENT_PRIMARY)
}

/// The border of a focused panel.
pub fn focused_border() -> Style {
    style(styles::FOCUSED_BORDER)
}

/// The border of a panel that is not focused.
pub fn unfocused_border() -> Style {
    style(styles::UNFOCUSED_BORDER)
}

/// The item-kind badge in a search result.
pub fn kind_badge() -> Style {
    fg(tokens::ACCENT_SECONDARY)
}

/// The name of the selected search result.
pub fn selected_name() -> Style {
    fg(tokens::TEXT_PRIMARY).add_modifier(Modifier::BOLD)
}

/// The name of a search result that is not selected.
pub fn unselected_name() -> Style {
    fg(tokens::TEXT_SECONDARY)
}

/// The background stripe behind the selected search result.
pub fn selected_row() -> Style {
    bg(tokens::BG_HIGHLIGHT)
}

/// The highlight on a focused link in the item view.
pub fn focused_link() -> Style {
    bg("bg.active")
}

/// The subtler highlight marking the section `space` will fold.
pub fn cursor_line() -> Style {
    style("cursor_line")
}

/// The status bar along the bottom of the screen.
pub fn status_bar() -> Style {
    style(styles::DIMMED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    /// The named theme must actually exist: a typo here would silently leave
    /// opaline's default theme in place.
    #[test]
    fn the_default_theme_loads() {
        assert!(
            load_by_name(DEFAULT_THEME).is_some(),
            "{DEFAULT_THEME} is not a builtin opaline theme"
        );
    }

    /// Every style this module exposes must resolve to a real color rather than
    /// falling back to a default, which is how a mistyped token name shows up.
    #[test]
    fn every_style_resolves_against_the_theme() {
        init();
        let styles: [(&str, Style); 19] = [
            ("title", title()),
            ("section", section()),
            ("signature", signature()),
            ("dim", dim()),
            ("muted", muted()),
            ("deprecated", deprecated()),
            ("unstable", unstable()),
            ("heading", heading()),
            ("inline_code", inline_code()),
            ("link", link()),
            ("quote", quote()),
            ("accent", accent()),
            ("focused_border", focused_border()),
            ("unfocused_border", unfocused_border()),
            ("kind_badge", kind_badge()),
            ("selected_name", selected_name()),
            ("unselected_name", unselected_name()),
            ("selected_row", selected_row()),
            ("cursor_line", cursor_line()),
        ];

        for (name, style) in styles {
            let color = style.fg.or(style.bg);
            assert!(
                matches!(color, Some(Color::Rgb(..))),
                "{name} resolved to {color:?}, not a theme color"
            );
        }
    }
}
