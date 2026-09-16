//! Shot provider (M3): Flex Capture menu rows for `flex shot`.
//!
//! Row-set parity with `rofi/.config/rofi/scripts/screenshot.sh`
//! (deleted at cutover; the wrapper owns the capture pipeline now):
//!
//! - Tab title `Capture`; standard (spec) rows with a `PNG`/`MP4` meta.
//! - Labels keep their exact bash bytes, including the two-space indent
//!   on area/window rows and the `🖥` glyph on full-capture rows.
//! - Row ids are the bash `case` arms (`area-shot`, …, `full-rec-audio`);
//!   the shot executor matches on them after the TUI exits
//!   (`slurp` needs a clean tty, so capture never runs inside the TUI).
//!
//! The library never executes captures; it only selects a row.

use flex_core::{Row, RowId, Tab};

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "shot";
/// Tab title (matches the bash `Capture` tab).
pub const TAB_NAME: &str = "Capture";

/// One capture row: bash `case` id + exact label + kind meta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShotRow {
    /// Bash `case` arm; also the [`Row`] action id.
    pub id: &'static str,
    /// Exact menu label (leading spaces / `🖥` preserved).
    pub label: &'static str,
    /// Capture kind (`PNG` screenshot vs `MP4` recording).
    pub meta: &'static str,
}

/// Capture rows in bash `CAP_ROWS` order (mode pick first, then
/// recordings; region/window/full within each group).
pub const ROWS: [ShotRow; 7] = [
    ShotRow {
        id: "area-shot",
        label: "Area Screenshot",
        meta: "PNG",
    },
    ShotRow {
        id: "full-shot",
        label: "Full Screenshot",
        meta: "PNG",
    },
    ShotRow {
        id: "win-shot",
        label: "Window Screenshot",
        meta: "PNG",
    },
    ShotRow {
        id: "area-rec",
        label: "Area Recording",
        meta: "MP4",
    },
    ShotRow {
        id: "area-rec-audio",
        label: "Area Recording + Audio",
        meta: "MP4",
    },
    ShotRow {
        id: "full-rec",
        label: "Full Recording",
        meta: "MP4",
    },
    ShotRow {
        id: "full-rec-audio",
        label: "Full Recording + Audio",
        meta: "MP4",
    },
];

/// Load rows from the static [`ROWS`] table (no subprocess; the set is
/// fixed by the capture pipeline the wrapper implements).
#[must_use]
pub fn load() -> Vec<Row> {
    ROWS.iter()
        .map(|row| Row::with_meta(RowId::new(row.id), row.label, row.meta))
        .collect()
}

/// Build the `Capture` tab: standard spec rows, non-deletable rows.
#[must_use]
pub fn shot_tab() -> Tab {
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
        assert_eq!(
            ids,
            vec![
                "area-shot",
                "full-shot",
                "win-shot",
                "area-rec",
                "area-rec-audio",
                "full-rec",
                "full-rec-audio",
            ]
        );
    }
}
