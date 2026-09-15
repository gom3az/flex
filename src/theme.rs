//! Theme tokens — faithful port of wiremix `src/config/theme.rs`
//! (upstream commit `cbdc90f`).
//!
//! Token names, colors and modifiers match upstream's `Theme` struct and its
//! three built-in themes (`default`, `nocolor`, `plain`), so a flex menu can
//! render exactly like wiremix's default look or drop color entirely for
//! monochrome terminals. Select with [`ThemeName`] (CLI `--theme`).
//!
//! Flex extensions (elements wiremix has no counterpart for: the filter
//! prompt, hint line, gauge and offline dimming) live at the bottom of the
//! struct and are marked as such — they are the only tokens without an
//! upstream line reference.

use ratatui::style::{Color, Modifier, Style};

/// Built-in theme names (upstream `-t/--theme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeName {
    /// Upstream `default`: ANSI named colors.
    #[default]
    Default,
    /// Upstream `nocolor`: modifiers only, no colors.
    NoColor,
    /// Upstream `plain`: everything default.
    Plain,
}

impl ThemeName {
    /// All built-in names, in upstream order.
    pub const ALL: [Self; 3] = [Self::Default, Self::NoColor, Self::Plain];

    /// Upstream config name for this theme.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::NoColor => "nocolor",
            Self::Plain => "plain",
        }
    }

    /// Parse an upstream theme name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.as_str() == name)
    }
}

/// Style tokens for every element flex draws.
///
/// Field names mirror upstream's `Theme` so a reviewer can diff the two
/// structs directly; the last block is flex-only (documented in
/// `Docs/design_system.md` §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub default_device: Style,
    pub default_stream: Style,
    pub selector: Style,
    pub tab: Style,
    pub tab_selected: Style,
    pub tab_marker: Style,
    pub list_more: Style,
    pub node_title: Style,
    pub node_target: Style,
    pub volume: Style,
    pub volume_empty: Style,
    pub volume_filled: Style,
    pub meter_inactive: Style,
    pub meter_active: Style,
    pub meter_overload: Style,
    pub meter_center_inactive: Style,
    pub meter_center_active: Style,
    pub config_device: Style,
    pub config_profile: Style,
    pub dropdown_icon: Style,
    pub dropdown_border: Style,
    pub dropdown_item: Style,
    pub dropdown_selected: Style,
    pub dropdown_more: Style,
    pub help_border: Style,
    pub help_item: Style,
    pub help_more: Style,

    // --- flex extensions (no wiremix counterpart) ---------------------
    /// Filter prompt `›` (flex's filter line).
    pub filter_prompt: Style,
    /// Filter placeholder + hint line.
    pub hint: Style,
    /// Gauge fill (flex's `center` gauge widget).
    pub gauge_fill: Style,
    /// Offline rows/menus (`— offline`), dimmed like the old flex UI.
    pub offline: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Theme {
    /// Upstream `Theme::default()` (`theme.rs:110-155`).
    pub const DEFAULT: Self = Self {
        default_device: Style::new(),
        default_stream: Style::new(),
        selector: Style::new().fg(Color::LightCyan),
        tab: Style::new(),
        tab_selected: Style::new().fg(Color::LightCyan),
        tab_marker: Style::new().fg(Color::LightCyan),
        list_more: Style::new().fg(Color::DarkGray),
        node_title: Style::new(),
        node_target: Style::new(),
        volume: Style::new(),
        volume_empty: Style::new().fg(Color::DarkGray),
        volume_filled: Style::new().fg(Color::LightBlue),
        meter_inactive: Style::new().fg(Color::DarkGray),
        meter_active: Style::new().fg(Color::LightGreen),
        meter_overload: Style::new().fg(Color::Red),
        meter_center_inactive: Style::new().fg(Color::DarkGray),
        meter_center_active: Style::new().fg(Color::LightGreen),
        config_device: Style::new(),
        config_profile: Style::new(),
        dropdown_icon: Style::new(),
        dropdown_border: Style::new(),
        dropdown_item: Style::new(),
        dropdown_selected: Style::new()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::REVERSED),
        dropdown_more: Style::new().fg(Color::DarkGray),
        help_border: Style::new(),
        help_item: Style::new(),
        help_more: Style::new().fg(Color::DarkGray),
        // flex extensions
        filter_prompt: Style::new().fg(Color::LightCyan),
        hint: Style::new().fg(Color::DarkGray),
        gauge_fill: Style::new().fg(Color::LightBlue),
        offline: Style::new().fg(Color::DarkGray),
    };

    /// Upstream `Theme::nocolor()` (`theme.rs:167-198`).
    pub const NOCOLOR: Self = Self {
        default_device: Style::new(),
        default_stream: Style::new(),
        selector: Style::new().add_modifier(Modifier::BOLD),
        tab: Style::new(),
        tab_selected: Style::new().add_modifier(Modifier::BOLD),
        tab_marker: Style::new().add_modifier(Modifier::BOLD),
        list_more: Style::new(),
        node_title: Style::new(),
        node_target: Style::new(),
        volume: Style::new(),
        volume_empty: Style::new().add_modifier(Modifier::DIM),
        volume_filled: Style::new().add_modifier(Modifier::BOLD),
        meter_inactive: Style::new().add_modifier(Modifier::DIM),
        meter_active: Style::new().add_modifier(Modifier::BOLD),
        meter_overload: Style::new().add_modifier(Modifier::BOLD),
        meter_center_inactive: Style::new().add_modifier(Modifier::DIM),
        meter_center_active: Style::new().add_modifier(Modifier::BOLD),
        config_device: Style::new(),
        config_profile: Style::new(),
        dropdown_icon: Style::new(),
        dropdown_border: Style::new(),
        dropdown_item: Style::new(),
        dropdown_selected: Style::new().add_modifier(Modifier::REVERSED.union(Modifier::BOLD)),
        dropdown_more: Style::new(),
        help_border: Style::new(),
        help_item: Style::new(),
        help_more: Style::new(),
        // flex extensions
        filter_prompt: Style::new().add_modifier(Modifier::BOLD),
        hint: Style::new().add_modifier(Modifier::DIM),
        gauge_fill: Style::new().add_modifier(Modifier::BOLD),
        offline: Style::new().add_modifier(Modifier::DIM),
    };

    /// Upstream `Theme::plain()` (`theme.rs:202-230`).
    pub const PLAIN: Self = Self {
        default_device: Style::new(),
        default_stream: Style::new(),
        selector: Style::new(),
        tab: Style::new(),
        tab_selected: Style::new(),
        tab_marker: Style::new(),
        list_more: Style::new(),
        node_title: Style::new(),
        node_target: Style::new(),
        volume: Style::new(),
        volume_empty: Style::new(),
        volume_filled: Style::new(),
        meter_inactive: Style::new(),
        meter_active: Style::new(),
        meter_overload: Style::new(),
        meter_center_inactive: Style::new(),
        meter_center_active: Style::new(),
        config_device: Style::new(),
        config_profile: Style::new(),
        dropdown_icon: Style::new(),
        dropdown_border: Style::new(),
        dropdown_item: Style::new(),
        dropdown_selected: Style::new(),
        dropdown_more: Style::new(),
        help_border: Style::new(),
        help_item: Style::new(),
        help_more: Style::new(),
        // flex extensions
        filter_prompt: Style::new(),
        hint: Style::new(),
        gauge_fill: Style::new(),
        offline: Style::new(),
    };

    /// Look up a built-in theme by name.
    #[must_use]
    pub const fn get(name: ThemeName) -> Self {
        match name {
            ThemeName::Default => Self::DEFAULT,
            ThemeName::NoColor => Self::NOCOLOR,
            ThemeName::Plain => Self::PLAIN,
        }
    }

    /// Background style used by flex's full-frame fill.
    ///
    /// Upstream never paints a background (terminal default); flex clears the
    /// frame explicitly, which is equivalent on a default-background terminal
    /// and keeps the TUI opaque over stray output.
    #[must_use]
    pub const fn background() -> Style {
        Style::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_matches_wiremix() {
        let t = Theme::DEFAULT;
        assert_eq!(t.selector, Style::new().fg(Color::LightCyan));
        assert_eq!(t.tab_selected, Style::new().fg(Color::LightCyan));
        assert_eq!(t.tab_marker, Style::new().fg(Color::LightCyan));
        assert_eq!(t.tab, Style::new(), "inactive tab keeps the default fg");
        assert_eq!(t.list_more, Style::new().fg(Color::DarkGray));
        assert_eq!(t.node_title, Style::new());
        assert_eq!(t.node_target, Style::new());
        assert_eq!(t.volume, Style::new());
        assert_eq!(t.volume_empty, Style::new().fg(Color::DarkGray));
        assert_eq!(t.volume_filled, Style::new().fg(Color::LightBlue));
        assert_eq!(t.meter_inactive, Style::new().fg(Color::DarkGray));
        assert_eq!(t.meter_active, Style::new().fg(Color::LightGreen));
        assert_eq!(t.meter_overload, Style::new().fg(Color::Red));
        assert_eq!(
            t.dropdown_selected,
            Style::new()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::REVERSED)
        );
        assert_eq!(t.help_border, Style::new(), "help border is not dimmed");
        assert_eq!(t.help_more, Style::new().fg(Color::DarkGray));
    }

    #[test]
    fn nocolor_theme_uses_modifiers_only() {
        let t = Theme::NOCOLOR;
        for style in [
            t.selector,
            t.tab_selected,
            t.tab_marker,
            t.volume_empty,
            t.volume_filled,
            t.meter_inactive,
            t.meter_active,
            t.meter_overload,
            t.dropdown_selected,
        ] {
            assert_eq!(style.fg, None, "nocolor must not set a foreground");
            assert_eq!(style.bg, None, "nocolor must not set a background");
        }
        assert!(t.selector.add_modifier.contains(Modifier::BOLD));
        assert!(t.volume_empty.add_modifier.contains(Modifier::DIM));
        assert!(t
            .dropdown_selected
            .add_modifier
            .contains(Modifier::REVERSED));
    }

    #[test]
    fn plain_theme_is_all_default() {
        let t = Theme::PLAIN;
        for style in [
            t.selector,
            t.tab_selected,
            t.list_more,
            t.volume_empty,
            t.volume_filled,
            t.meter_active,
            t.dropdown_selected,
            t.help_more,
        ] {
            assert_eq!(style, Style::new());
        }
    }

    #[test]
    fn theme_names_round_trip() {
        for name in ThemeName::ALL {
            assert_eq!(ThemeName::parse(name.as_str()), Some(name));
        }
        assert_eq!(ThemeName::parse("nope"), None);
        assert_eq!(Theme::get(ThemeName::NoColor), Theme::NOCOLOR);
    }
}
