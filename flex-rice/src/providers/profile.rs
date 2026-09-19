//! Power Profile provider: static rows for `flex profile`.
//!
//! Rows:
//! - Performance Profile (`profile:performance` / `powerprofilesctl set performance`)
//! - Balanced Profile (`profile:balanced` / `powerprofilesctl set balanced`)
//! - Power Saver Profile (`profile:power-saver` / `powerprofilesctl set power-saver`)

use flex_core::{Row, RowId, Tab};

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "profile";
/// Tab title.
pub const TAB_NAME: &str = "Profile";

/// One profile row: action id + exact label + command meta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileRow {
    pub id: &'static str,
    pub label: &'static str,
    pub meta: &'static str,
    pub confirmable: bool,
}

/// Power profile rows.
pub const ROWS: [ProfileRow; 3] = [
    ProfileRow {
        id: "performance",
        label: "Performance Profile",
        meta: "powerprofilesctl set performance",
        confirmable: false,
    },
    ProfileRow {
        id: "balanced",
        label: "Balanced Profile",
        meta: "powerprofilesctl set balanced",
        confirmable: false,
    },
    ProfileRow {
        id: "power-saver",
        label: "Power Saver Profile",
        meta: "powerprofilesctl set power-saver",
        confirmable: false,
    },
];

/// Seam env var for active power profile (for testing/fixtures).
pub const POWER_PROFILE_FILE_ENV: &str = "POWER_PROFILE_FILE";

/// Resolve the power profile cache file path (`$XDG_CACHE_HOME/flex/power-profile` or `~/.cache/flex/power-profile`).
#[must_use]
pub fn cache_file_path() -> std::path::PathBuf {
    let parent = std::env::var("XDG_CACHE_HOME").map_or_else(
        |_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/tmp"));
            std::path::PathBuf::from(home).join(".cache")
        },
        std::path::PathBuf::from,
    );
    parent.join("flex").join("power-profile")
}

/// Save active profile to disk cache.
pub fn save_active_profile(profile: &str) {
    let path = cache_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{profile}\n"));
}

/// Query the currently active power profile (`powerprofilesctl get`, `tuned-adm active`, EPP sysfs, seam, disk cache, or `balanced` fallback).
#[must_use]
pub fn active_profile() -> Option<String> {
    if let Ok(file) = std::env::var(POWER_PROFILE_FILE_ENV) {
        if file.is_empty() {
            return None;
        }
        return std::fs::read_to_string(file)
            .ok()
            .map(|s| s.trim().to_string());
    }

    if let Ok(out) = std::process::Command::new("powerprofilesctl")
        .arg("get")
        .output()
    {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    save_active_profile(&trimmed);
                    return Some(trimmed);
                }
            }
        }
    }

    if let Ok(out) = std::process::Command::new("tuned-adm")
        .arg("active")
        .output()
    {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                let lower = s.to_lowercase();
                let prof = if lower.contains("performance") || lower.contains("throughput") {
                    "performance"
                } else if lower.contains("power") || lower.contains("save") {
                    "power-saver"
                } else if lower.contains("balanced") || lower.contains("desktop") {
                    "balanced"
                } else {
                    ""
                };
                if !prof.is_empty() {
                    save_active_profile(prof);
                    return Some(prof.to_string());
                }
            }
        }
    }

    if let Ok(content) = std::fs::read_to_string(
        "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference",
    ) {
        let lower = content.to_lowercase();
        let prof = if lower.contains("performance") && !lower.contains("balance") {
            "performance"
        } else if lower.contains("power") {
            "power-saver"
        } else if lower.contains("balance") || lower.contains("default") {
            "balanced"
        } else {
            ""
        };
        if !prof.is_empty() {
            save_active_profile(prof);
            return Some(prof.to_string());
        }
    }

    // Disk cache fallback if powerprofilesctl/tuned-adm are missing or fail.
    if let Ok(content) = std::fs::read_to_string(cache_file_path()) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }

    Some(String::from("balanced"))
}

/// Build the `Profile` tab with optional active profile highlight.
#[must_use]
pub fn profile_tab_from(active: Option<&str>) -> Tab {
    let rows: Vec<Row> = ROWS
        .iter()
        .map(|row| {
            let is_active = active.is_some_and(|act| row.id == act);
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

    let mut tab = Tab::with_rows(TAB_NAME, rows);
    tab.filterable = false;
    if let Some(act) = active {
        if let Some(idx) = ROWS.iter().position(|r| r.id == act) {
            tab.state.focus = idx;
        }
    }
    tab
}

/// Build the `Profile` tab live (querying active profile).
#[must_use]
pub fn profile_tab() -> Tab {
    profile_tab_from(active_profile().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_table_covers_all_profile_actions() {
        let ids: Vec<&str> = ROWS.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec!["performance", "balanced", "power-saver"]);
    }

    #[test]
    fn active_profile_highlight_and_focus() {
        let tab = profile_tab_from(Some("balanced"));
        assert_eq!(tab.state.focus, 1);
        assert_eq!(
            tab.rows[1].meta.as_deref(),
            Some("powerprofilesctl set balanced  Active")
        );
    }
}
