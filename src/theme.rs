//! Theming, resolved from an [`opaline`] token theme.
//!
//! Opaline resolves in three layers — palette hex values, semantic tokens, then
//! named styles — and every builtin theme defines the same contract, so the
//! screens below name tokens rather than colors and follow whatever theme is
//! active. Themes load at runtime, so these are functions rather than the
//! `const` styles they replace; each is a couple of hash lookups against the
//! process-wide theme, which is cheap enough to call per rendered span.

use std::sync::Arc;

use opaline::{
    Theme, load_by_name,
    names::{styles, tokens},
    set_theme,
};
use ratatui::style::{Modifier, Style};

/// The theme used unless the app is told otherwise.
///
/// Opaline identifies builtins by kebab-case id, not by the underscored name
/// of the arborium function that produces the matching syntax theme.
// pub const DEFAULT_THEME: &str = "catppuccin-latte";
pub const DEFAULT_THEME: &str = "catppuccin-mocha";

/// The syntax-highlighting theme matching [`DEFAULT_THEME`], so code and
/// chrome always come from the same palette.
///
/// Arborium names its themes for display ("Catppuccin Latte") where opaline
/// uses kebab-case ids, so the id is derived from the name rather than kept as
/// a second setting that could drift. Falls back to Catppuccin Mocha, the
/// default of both, when arborium has no theme by that id.
pub fn syntax() -> arborium_theme::Theme {
    let builtin = arborium_theme::builtin::all();
    builtin
        .into_iter()
        .find(|t| t.name.to_lowercase().replace(' ', "-") == DEFAULT_THEME)
        .unwrap_or_else(arborium_theme::builtin::catppuccin_mocha)
}

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
fn theme() -> Arc<Theme> {
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

/// A deprecation notice: the warning colour, in bold, so it stands out from
/// [`unstable`].
pub fn deprecated() -> Style {
    style(styles::WARNING_STYLE).add_modifier(Modifier::BOLD)
}

/// An unstable-feature notice: a caveat rather than a defect, so it reads a
/// step softer than [`deprecated`] — the same colour, without the weight.
/// Nightly docs mark a good share of items unstable, so this is also the one
/// that must not shout.
pub fn unstable() -> Style {
    style(styles::WARNING_STYLE)
}

// ── Markdown ─────────────────────────────────────────────────────────────

/// A markdown heading inside rendered docs.
pub fn heading() -> Style {
    fg(tokens::ACCENT_PRIMARY).add_modifier(Modifier::BOLD)
}

/// Inline `code` spans.
///
/// The theme's own `inline_code` carries a background, which reads as speckle
/// once whole code blocks are painted with one: a word-sized patch of the
/// block colour in the middle of a sentence looks like a rendering fault
/// rather than emphasis. The foreground alone marks it, and the backticks
/// survive in the text besides.
pub fn inline_code() -> Style {
    let mut s = style(styles::INLINE_CODE);
    s.bg = None;
    s
}

/// An intra-doc link.
pub fn link() -> Style {
    fg(tokens::INFO).add_modifier(Modifier::UNDERLINED)
}

/// A horizontal rule (`---`) between passages of docs: a divider, drawn like
/// the border of a panel that does not have focus.
pub fn rule() -> Style {
    fg(tokens::BORDER_UNFOCUSED)
}

/// A block quote.
pub fn quote() -> Style {
    style(styles::DIMMED).add_modifier(Modifier::ITALIC)
}

/// The background stripe behind a fenced code block.
///
/// Applied to the whole width of every line in the block, gutter included, so
/// an example reads as one slab rather than as ragged prose.
pub fn code_block() -> Style {
    bg(tokens::BG_CODE)
}

/// The page's own background.
///
/// Painted explicitly rather than left to fall through to the terminal's. The
/// tints below are defined as steps away from *this* colour, so leaving it
/// unset would make them steps away from whatever the user's terminal happens
/// to use — and under the matching terminal theme, `bg.panel` is exactly the
/// terminal's own background, which made [`member_docs`] invisible.
pub fn page() -> Style {
    bg(tokens::BG_BASE)
}

/// The background behind an item's own documentation, distinguishing one
/// method's description from the next.
///
/// A step *lighter* than the page, and the lowest of the three highlight
/// rungs, so the two that mean something transient — [`cursor_line`] and
/// [`focused_link`] — both stay brighter than ordinary prose.
///
/// Not `bg.code`: a good third of the builtins alias that to `bg.panel`
/// outright, which would leave a description invisible under those themes the
/// way `bg.panel` was under Catppuccin. Keeping it off `bg.code` also lets an
/// example nested in a description read as nested rather than merging with it.
pub fn member_docs() -> Style {
    bg(tokens::BG_HIGHLIGHT)
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
    bg(tokens::BG_ACTIVE)
}

/// The subtler highlight marking the section `space` will fold.
///
/// The theme's own `cursor_line` style resolves to `bg.highlight`, which
/// [`member_docs`] now uses for every line of prose on the page — the cursor
/// would disappear into it. `bg.elevated` is the next rung up, keeping it
/// brighter than the documentation it sits among and dimmer than
/// [`focused_link`], which marks the stronger thing.
pub fn cursor_line() -> Style {
    bg(tokens::BG_ELEVATED)
}

/// The status bar along the bottom of the screen.
pub fn status_bar() -> Style {
    style(styles::DIMMED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    /// A deprecation is a defect and an unstable feature only a caveat; once
    /// both notices were the same style, which said they mattered equally.
    #[test]
    fn deprecation_outweighs_instability() {
        init();
        assert_ne!(deprecated(), unstable());
    }

    /// The named theme must actually exist: a typo here would silently leave
    /// opaline's default theme in place.
    #[test]
    fn the_default_theme_loads() {
        assert!(
            load_by_name(DEFAULT_THEME).is_some(),
            "{DEFAULT_THEME} is not a builtin opaline theme"
        );
    }

    /// The syntax theme is found by name, not just defaulted: a mismatch
    /// would highlight code in one palette and draw everything else in another.
    #[test]
    fn the_syntax_theme_matches_the_default_theme() {
        let name = syntax().name.to_lowercase().replace(' ', "-");
        assert_eq!(name, DEFAULT_THEME);
    }

    /// Every style this module exposes must resolve to a real color rather than
    /// falling back to a default, which is how a mistyped token name shows up.
    #[test]
    fn every_style_resolves_against_the_theme() {
        init();
        let styles = [
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
            ("rule", rule()),
            ("code_block", code_block()),
            ("page", page()),
            ("member_docs", member_docs()),
            ("accent", accent()),
            ("focused_border", focused_border()),
            ("unfocused_border", unfocused_border()),
            ("kind_badge", kind_badge()),
            ("selected_name", selected_name()),
            ("unselected_name", unselected_name()),
            ("selected_row", selected_row()),
            ("focused_link", focused_link()),
            ("cursor_line", cursor_line()),
            ("status_bar", status_bar()),
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
