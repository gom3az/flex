//! Character sets — faithful port of wiremix `src/config/char_set.rs`
//! (upstream commit `cbdc90f`).
//!
//! Flex renders the same glyphs as wiremix for the same elements; the three
//! built-in sets (`default`, `compat`, `extracompat`) carry upstream's exact
//! values, so a menu can fall back to box-drawing-free glyphs on terminals
//! without Unicode support. Select with [`CharSetName`] (CLI `--char-set`).

use ratatui::widgets::BorderType;

/// Built-in character-set names (upstream `-s/--char-set`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CharSetName {
    /// Upstream `default`: Unicode box drawing + block elements.
    #[default]
    Default,
    /// Upstream `compat`: keeps `░▒░`, falls back to plain box drawing.
    Compat,
    /// Upstream `extracompat`: ASCII-only.
    ExtraCompat,
}

impl CharSetName {
    /// All built-in names, in upstream order.
    pub const ALL: [Self; 3] = [Self::Default, Self::Compat, Self::ExtraCompat];

    /// Upstream config name for this set.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Compat => "compat",
            Self::ExtraCompat => "extracompat",
        }
    }

    /// Parse an upstream set name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|set| set.as_str() == name)
    }
}

/// Every glyph flex draws, mirroring upstream's `CharSet` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSet {
    pub default_device: &'static str,
    pub default_stream: &'static str,
    pub selector_top: &'static str,
    pub selector_middle: &'static str,
    pub selector_bottom: &'static str,
    pub tab_marker_left: &'static str,
    pub tab_marker_right: &'static str,
    pub list_more: &'static str,
    pub volume_empty: &'static str,
    pub volume_filled: &'static str,
    pub meter_left_inactive: &'static str,
    pub meter_left_active: &'static str,
    pub meter_left_overload: &'static str,
    pub meter_right_inactive: &'static str,
    pub meter_right_active: &'static str,
    pub meter_right_overload: &'static str,
    pub meter_center_left_inactive: &'static str,
    pub meter_center_left_active: &'static str,
    pub meter_center_right_inactive: &'static str,
    pub meter_center_right_active: &'static str,
    pub dropdown_icon: &'static str,
    pub dropdown_selector: &'static str,
    pub dropdown_more: &'static str,
    pub dropdown_border: BorderType,
    pub help_more: &'static str,
    pub help_border: BorderType,
}

impl Default for CharSet {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl CharSet {
    /// Upstream `CharSet::default()` (`char_set.rs:130-160`).
    pub const DEFAULT: Self = Self {
        default_device: "◇",
        default_stream: "◇",
        selector_top: "░",
        selector_middle: "▒",
        selector_bottom: "░",
        tab_marker_left: "[",
        tab_marker_right: "]",
        list_more: "•••",
        volume_empty: "╌",
        volume_filled: "━",
        meter_left_inactive: "▮",
        meter_left_active: "▮",
        meter_left_overload: "▮",
        meter_right_inactive: "▮",
        meter_right_active: "▮",
        meter_right_overload: "▮",
        meter_center_left_inactive: "▮",
        meter_center_left_active: "▮",
        meter_center_right_inactive: "▮",
        meter_center_right_active: "▮",
        dropdown_icon: "▼",
        dropdown_selector: ">",
        dropdown_more: "•••",
        dropdown_border: BorderType::Rounded,
        help_more: "•••",
        help_border: BorderType::Rounded,
    };

    /// Upstream `CharSet::compat()` (`char_set.rs:170-205`).
    pub const COMPAT: Self = Self {
        default_device: "◊",
        default_stream: "◊",
        selector_top: "░",
        selector_middle: "▒",
        selector_bottom: "░",
        tab_marker_left: "[",
        tab_marker_right: "]",
        list_more: "•••",
        volume_empty: "─",
        volume_filled: "━",
        meter_left_inactive: "┃",
        meter_left_active: "┃",
        meter_left_overload: "┃",
        meter_right_inactive: "┃",
        meter_right_active: "┃",
        meter_right_overload: "┃",
        meter_center_left_inactive: "█",
        meter_center_left_active: "█",
        meter_center_right_inactive: "█",
        meter_center_right_active: "█",
        dropdown_icon: "▼",
        dropdown_selector: ">",
        dropdown_more: "•••",
        dropdown_border: BorderType::Plain,
        help_more: "•••",
        help_border: BorderType::Plain,
    };

    /// Upstream `CharSet::extracompat()` (`char_set.rs:207-235`).
    pub const EXTRA_COMPAT: Self = Self {
        default_device: "*",
        default_stream: "*",
        selector_top: "-",
        selector_middle: "=",
        selector_bottom: "-",
        tab_marker_left: "[",
        tab_marker_right: "]",
        list_more: "~~~",
        volume_empty: "-",
        volume_filled: "=",
        meter_left_inactive: "=",
        meter_left_active: "#",
        meter_left_overload: "!",
        meter_right_inactive: "=",
        meter_right_active: "#",
        meter_right_overload: "!",
        meter_center_left_inactive: "[",
        meter_center_left_active: "[",
        meter_center_right_inactive: "]",
        meter_center_right_active: "]",
        dropdown_icon: "\\",
        dropdown_selector: ">",
        dropdown_more: "~~~",
        dropdown_border: BorderType::Plain,
        help_more: "~~~",
        help_border: BorderType::Plain,
    };

    /// Look up a built-in set by name.
    #[must_use]
    pub const fn get(name: CharSetName) -> Self {
        match name {
            CharSetName::Default => Self::DEFAULT,
            CharSetName::Compat => Self::COMPAT,
            CharSetName::ExtraCompat => Self::EXTRA_COMPAT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_set_matches_wiremix() {
        let cs = CharSet::DEFAULT;
        assert_eq!(cs.default_device, "◇");
        assert_eq!(cs.default_stream, "◇");
        assert_eq!(
            (cs.selector_top, cs.selector_middle, cs.selector_bottom),
            ("░", "▒", "░")
        );
        assert_eq!((cs.tab_marker_left, cs.tab_marker_right), ("[", "]"));
        assert_eq!(cs.list_more, "•••");
        assert_eq!((cs.volume_empty, cs.volume_filled), ("╌", "━"));
        for glyph in [
            cs.meter_left_inactive,
            cs.meter_left_active,
            cs.meter_left_overload,
            cs.meter_right_inactive,
            cs.meter_right_active,
            cs.meter_right_overload,
            cs.meter_center_left_inactive,
            cs.meter_center_left_active,
            cs.meter_center_right_inactive,
            cs.meter_center_right_active,
        ] {
            assert_eq!(glyph, "▮", "default meters are all ▮");
        }
        assert_eq!((cs.dropdown_icon, cs.dropdown_selector), ("▼", ">"));
        assert_eq!(cs.dropdown_more, "•••");
        assert_eq!(cs.help_more, "•••");
        assert_eq!(cs.dropdown_border, BorderType::Rounded);
        assert_eq!(cs.help_border, BorderType::Rounded);
    }

    #[test]
    fn compat_set_matches_wiremix() {
        let cs = CharSet::COMPAT;
        assert_eq!(cs.default_device, "◊");
        assert_eq!((cs.volume_empty, cs.volume_filled), ("─", "━"));
        assert_eq!(cs.meter_left_active, "┃");
        assert_eq!(cs.meter_center_left_active, "█");
        assert_eq!(cs.dropdown_icon, "▼");
        assert_eq!(cs.list_more, "•••");
        assert_eq!(cs.dropdown_border, BorderType::Plain);
        assert_eq!(cs.help_border, BorderType::Plain);
        // compat keeps the selector glyphs
        assert_eq!(cs.selector_middle, "▒");
    }

    #[test]
    fn extracompat_set_is_ascii() {
        let cs = CharSet::EXTRA_COMPAT;
        assert_eq!(cs.default_device, "*");
        assert_eq!((cs.selector_top, cs.selector_middle), ("-", "="));
        assert_eq!(cs.volume_empty, "-");
        assert_eq!(cs.volume_filled, "=");
        assert_eq!(
            (
                cs.meter_left_inactive,
                cs.meter_left_active,
                cs.meter_left_overload
            ),
            ("=", "#", "!")
        );
        assert_eq!(cs.dropdown_icon, "\\");
        assert_eq!(cs.list_more, "~~~");
        assert_eq!(cs.dropdown_border, BorderType::Plain);
        for glyph in [
            cs.selector_top,
            cs.selector_middle,
            cs.selector_bottom,
            cs.volume_empty,
            cs.volume_filled,
            cs.list_more,
            cs.dropdown_icon,
            cs.meter_left_overload,
        ] {
            assert!(glyph.is_ascii(), "{glyph:?} must be ASCII");
        }
    }

    #[test]
    fn names_round_trip() {
        for name in CharSetName::ALL {
            assert_eq!(CharSetName::parse(name.as_str()), Some(name));
        }
        assert_eq!(CharSetName::parse("nope"), None);
        assert_eq!(CharSet::get(CharSetName::default()), CharSet::DEFAULT);
    }
}
