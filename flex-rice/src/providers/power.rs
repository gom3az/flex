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
//!   duplicated here (same delegation as `center::power_tab`, which uses
//!   `pw`-prefixed ids for the center surface; the standalone provider
//!   keeps the bash-exact arms).
//!
//! The library never executes power operations; it only selects a row.

use flex_core::{Row, RowId, Tab};

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "power";
/// Tab title (matches the bash `Power` tab).
pub const TAB_NAME: &str = "Power";

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

/// Power rows in bash `POWER_ROWS` order (lock first, logout last;
///
/// destructive rows in the middle, matching the deleted script).
pub const ROWS: [PowerRow; 5] = [
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

/// Load rows from the static [`ROWS`] table (no subprocess; the set is
/// fixed by the power commands the wrapper implements).
#[must_use]
pub fn load() -> Vec<Row> {
    ROWS.iter()
        .map(|row| Row {
            confirmable: row.confirmable,
            ..Row::with_meta(RowId::new(row.id), row.label, row.meta)
        })
        .collect()
}

/// Build the `Power` tab: standard spec rows, non-deletable rows
/// (`Delete` never fires here; the danger flow is Enter-arm only).
/// Five fixed rows need no search and no chrome: launch-style bare rows
/// (no title, no filter, no hints, no metas).
#[must_use]
pub fn power_tab() -> Tab {
    let mut tab = Tab::with_rows(TAB_NAME, load());
    tab.filterable = false;
    tab
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_table_covers_every_bash_case_arm() {
        let ids: Vec<&str> = ROWS.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec!["lock", "suspend", "reboot", "poweroff", "logout"]);
    }
}
