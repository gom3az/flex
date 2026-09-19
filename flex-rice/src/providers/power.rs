//! Power provider (M6): static System / Power menu rows for `flex power`.
//!
//! Row-set parity with `waybar/.config/waybar/power-menu.sh` (deleted at
//! cutover; the last bash `flex-tui` consumer):
//!
//! - Row order, labels, and metas are exact: `Lock Screen`/`hyprlock`,
//!   `Suspend`/`systemctl suspend`, `Reboot`/`systemctl reboot`,
//!   `Power Off`/`systemctl poweroff`, `Logout`/`pkill -SIGTERM Hyprland`.
//! - Row ids are the bash `flex_on_activate` case arms (`lock`, `suspend`,
//!   `reboot`, `poweroff`, `logout`); the power executor matches on
//!   them after the TUI exits, running the same commands verbatim.
//! - `Reboot`/`Power Off` are danger rows armed/confirmed by the shared
//!   [`keys`](flex_core::keys) double-Enter flow — danger logic is never
//!   duplicated here; the provider keeps the bash-exact arms.
//!
//! The library never executes power operations; it only selects a row.

use flex_core::{Row, RowId, Tab};

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "power";
/// Tab title (matches the bash `Power` tab).
pub const TAB_POWER: &str = "Power";
pub const TAB_PROFILES: &str = "Profiles";

/// One power row: bash `case` id + exact label + truthful command meta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerRow {
    /// Bash `flex_on_activate` arm; also the [`Row`] action id.
    pub id: &'static str,
    /// Exact menu label.
    pub label: &'static str,
    /// The command the wrapper runs for this row (never truncated).
    pub meta: &'static str,
    /// Whether the row needs the armed double-Enter confirm.
    pub confirmable: bool,
}

pub const SYSTEM_ROWS: [PowerRow; 5] = [
    PowerRow {
        id: "lock",
        label: "Lock Screen",
        meta: "hyprlock",
        confirmable: false,
    },
    PowerRow {
        id: "suspend",
        label: "Suspend",
        meta: "systemctl suspend",
        confirmable: false,
    },
    PowerRow {
        id: "reboot",
        label: "Reboot",
        meta: "systemctl reboot",
        confirmable: true,
    },
    PowerRow {
        id: "poweroff",
        label: "Power Off",
        meta: "systemctl poweroff",
        confirmable: true,
    },
    PowerRow {
        id: "logout",
        label: "Logout",
        meta: "pkill -SIGTERM Hyprland",
        confirmable: false,
    },
];

pub const PROFILE_ROWS: [PowerRow; 3] = [
    PowerRow {
        id: "profile:performance",
        label: "Performance Profile",
        meta: "powerprofilesctl set performance",
        confirmable: false,
    },
    PowerRow {
        id: "profile:balanced",
        label: "Balanced Profile",
        meta: "powerprofilesctl set balanced",
        confirmable: false,
    },
    PowerRow {
        id: "profile:power-saver",
        label: "Power Saver Profile",
        meta: "powerprofilesctl set power-saver",
        confirmable: false,
    },
];

fn to_rows(rows: &[PowerRow]) -> Vec<Row> {
    rows.iter()
        .map(|row| {
            let mut out = Row::with_meta(RowId::new(row.id), row.label, row.meta);
            out.confirmable = row.confirmable;
            out
        })
        .collect()
}

/// Build the `Power` tab.
#[must_use]
pub fn power_tab() -> Tab {
    let mut tab = Tab::with_rows(TAB_POWER, to_rows(&SYSTEM_ROWS));
    tab.filterable = false;
    tab
}

/// Build the `Profiles` tab with optional active profile highlight.
#[must_use]
pub fn profiles_tab_from(active: Option<&str>) -> Tab {
    let rows: Vec<Row> = PROFILE_ROWS
        .iter()
        .map(|row| {
            let is_active = active.is_some_and(|act| row.id.ends_with(act));
            let meta = if is_active {
                format!("{}  Active", row.meta)
            } else {
                row.meta.to_string()
            };
            let mut r = Row::with_meta(RowId::new(row.id), row.label, meta);
            r.confirmable = row.confirmable;
            r.is_default = is_active;
            r
        })
        .collect();

    let mut tab = Tab::with_rows(TAB_PROFILES, rows);
    tab.filterable = false;
    if let Some(act) = active {
        if let Some(idx) = PROFILE_ROWS.iter().position(|r| r.id.ends_with(act)) {
            tab.state.focus = idx;
        }
    }
    tab
}

/// Build the `Profiles` tab live (querying active profile).
#[must_use]
pub fn profiles_tab() -> Tab {
    profiles_tab_from(super::profile::active_profile().as_deref())
}

/// Build both tabs for `flex-power`.
#[must_use]
pub fn power_tabs() -> Vec<Tab> {
    vec![power_tab(), profiles_tab()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_and_profile_rows_are_separate() {
        let sys_ids: Vec<&str> = SYSTEM_ROWS.iter().map(|row| row.id).collect();
        assert_eq!(
            sys_ids,
            vec!["lock", "suspend", "reboot", "poweroff", "logout"]
        );
        let prof_ids: Vec<&str> = PROFILE_ROWS.iter().map(|row| row.id).collect();
        assert_eq!(
            prof_ids,
            vec![
                "profile:performance",
                "profile:balanced",
                "profile:power-saver"
            ]
        );
    }
}
