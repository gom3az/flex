//! Theme provider (M3): theme-switcher `pick()` rows for `flex theme`.
//!
//! Ports ONLY the `pick()` menu path of
//! `scripts/.config/scripts/theme-switcher.sh` (the `list` / `current` /
//! `activate` / `delete` entry points stay in bash; the theme executor
//! calls back into `theme-switcher.sh activate`):
//!
//! - One row per directory directly under
//!   `$HOME/.config/themes/available` (sorted, like the bash
//!   `find … | sort`).
//! - Label = theme (directory) name; meta = wallpaper basename from
//!   `metadata.json` (`(no metadata)` / `unknown` fallbacks match bash).
//! - The active theme (from `current/metadata.json` `theme_name`)
//!   appends `  Active` to its meta — the bash renderer joins meta and
//!   status as `"$meta  $status"`, and [`Row`] has no separate status
//!   field, so the status is folded into the meta string here.
//! - Row id = [`entry_id`] hash of the theme name (space-free: a theme
//!   directory may be named `My Theme`, which would split the `ACTION:`
//!   id token — B-021); the theme executor resolves it back with the
//!   library resolver and passes the name to `activate`.
//!
//! JSON is parsed with a std-only string scan (no `serde` in v1).
//! The library never activates themes; it only selects a row.

use std::path::PathBuf;

use flex_core::{content_hash_hex, Row, RowId, Tab};

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "theme";
/// Tab title (matches the bash `Themes` tab).
pub const TAB_NAME: &str = "Themes";
/// Meta suffix marking the active theme (bash `status` field).
pub const ACTIVE_SUFFIX: &str = "  Active";

/// One theme row: directory name + wallpaper basename + active flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeEntry {
    /// Theme (directory) name; the [`Row`] id is [`entry_id`] of it.
    pub name: String,
    /// Wallpaper basename (`(no metadata)` / `unknown` fallbacks).
    pub wallpaper: String,
    /// Whether this is the active theme (`Active` status in bash).
    pub active: bool,
}

/// Directory holding the available themes (`find` root in bash `pick()`).
#[must_use]
pub fn available_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/themes/available")
}

/// Directory holding the active theme (`metadata.json` → `theme_name`).
#[must_use]
pub fn current_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/themes/current")
}

/// Read the active theme name from `current/metadata.json`.
///
/// Returns an empty string when unreadable (bash `pick()` leaves
/// `current` empty on the same failure, so no row is marked active).
#[must_use]
pub fn current_name() -> String {
    current_name_in(&current_dir())
}

/// [`current_name`] over an explicit directory (tests/fixtures).
#[must_use]
pub fn current_name_in(dir: &std::path::Path) -> String {
    let text = std::fs::read_to_string(dir.join("metadata.json")).unwrap_or_default();
    json_string_field(&text, "theme_name").unwrap_or_default()
}

/// Scan `available` (sorted, like `find … | sort`) into theme entries.
///
/// Never panics and never fails: unreadable directories yield no rows;
/// per-theme metadata failures degrade to the bash fallback labels.
#[must_use]
pub fn scan_available(available: &std::path::Path, current: &str) -> Vec<ThemeEntry> {
    let Ok(read_dir) = std::fs::read_dir(available) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = read_dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs.iter()
        .filter_map(|dir| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .map(|name| ThemeEntry {
                    name: name.to_string(),
                    wallpaper: wallpaper_base(dir),
                    active: name == current,
                })
        })
        .collect()
}

/// Load rows from the real theme directories.
#[must_use]
pub fn load() -> Vec<Row> {
    rows(&scan_available(&available_dir(), &current_name()))
}

/// Map entries to [`Row`]s: id = [`entry_id`], label = theme name,
/// meta = wallpaper basename (`  Active` appended when active).
#[must_use]
pub fn rows(entries: &[ThemeEntry]) -> Vec<Row> {
    entries
        .iter()
        .map(|entry| {
            let id = RowId::new(entry_id(&entry.name));
            let meta = if entry.active {
                format!("{}{ACTIVE_SUFFIX}", entry.wallpaper)
            } else {
                entry.wallpaper.clone()
            };
            Row::with_meta(id, entry.name.clone(), meta)
        })
        .collect()
}

/// Row/action id for a theme name: its content hash.
///
/// A theme directory name may contain whitespace (`My Theme`), and the
/// `ACTION:` protocol delimits the id with spaces, so the name cannot be
/// the id itself (B-021). [`resolve_name`] maps the hash back.
#[must_use]
pub fn entry_id(name: &str) -> String {
    content_hash_hex(name)
}

/// Resolve a row id back to its theme name (the theme executor's
/// in-process lookup, like `clip::resolve`).
///
/// Searched over the same entry set [`rows`] is built from. Returns `None`
/// for unknown ids and for ids containing `/`.
#[must_use]
pub fn resolve_name(hash: &str) -> Option<String> {
    resolve_name_in(&available_dir(), hash)
}

/// [`resolve_name`] over an explicit directory (tests/fixtures).
#[must_use]
pub fn resolve_name_in(available: &std::path::Path, hash: &str) -> Option<String> {
    if hash.is_empty() || hash.contains('/') {
        return None;
    }
    scan_available(available, "")
        .into_iter()
        .find(|entry| entry_id(&entry.name) == hash)
        .map(|entry| entry.name)
}

/// Placeholder label for an empty `available/` directory (no bash
/// equivalent — the deleted `pick()` simply showed nothing).
pub const NO_THEMES_LABEL: &str = "(No themes found)";

/// Build the `Themes` tab: standard spec rows, non-deletable rows.
///
/// An empty theme directory yields the `noop` placeholder row rather than a
/// blank menu (B-026): `Enter` on it is a no-op in the theme executor.
#[must_use]
pub fn theme_tab() -> Tab {
    Tab::with_rows(
        TAB_NAME,
        tab_rows(&scan_available(&available_dir(), &current_name())),
    )
}

/// Build a `Themes` tab from pre-scanned entries (tests/replays).
#[must_use]
pub fn tab_from_entries(entries: &[ThemeEntry]) -> Tab {
    Tab::with_rows(TAB_NAME, tab_rows(entries))
}

/// [`rows`] plus the empty-scan placeholder (B-026).
fn tab_rows(entries: &[ThemeEntry]) -> Vec<Row> {
    let mut rows = rows(entries);
    if rows.is_empty() {
        rows.push(super::empty_row(NO_THEMES_LABEL));
    }
    rows
}

/// Wallpaper basename for one theme dir (`(no metadata)` fallback).
fn wallpaper_base(dir: &std::path::Path) -> String {
    let text = std::fs::read_to_string(dir.join("metadata.json")).unwrap_or_default();
    if text.trim().is_empty() && !dir.join("metadata.json").exists() {
        return "(no metadata)".to_string();
    }
    let wallpaper = json_string_field(&text, "wallpaper").unwrap_or_else(|| "unknown".to_string());
    wallpaper
        .rsplit('/')
        .next()
        .unwrap_or("unknown")
        .to_string()
}

/// Extract a top-level JSON string field (`"key": "value"`).
///
/// Whitespace between tokens is skipped; escapes are passed through
/// unresolved (theme names/wallpapers never contain them, and the value
/// is display-only). Returns `None` when the key or a well-formed
/// value is absent.
fn json_string_field(text: &str, key: &str) -> Option<String> {
    let quoted = format!("\"{key}\"");
    let mut rest = text;
    while let Some(at) = rest.find(&quoted) {
        rest = &rest[at + quoted.len()..];
        let Some(after_colon) = rest
            .trim_start_matches([' ', '\t', '\r', '\n'])
            .strip_prefix(':')
        else {
            continue;
        };
        let value = after_colon.trim_start_matches([' ', '\t', '\r', '\n']);
        let Some(body) = value.strip_prefix('"') else {
            continue;
        };
        let mut out = String::with_capacity(body.len());
        let mut chars = body.chars();
        loop {
            match chars.next()? {
                '"' => return Some(out),
                '\\' => {
                    let escaped = chars.next()?;
                    out.push('\\');
                    out.push(escaped);
                }
                char => out.push(char),
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_field_skips_whitespace_and_missing_keys() {
        let text = "{\"theme_name\" : \"mocha\", \"wallpaper\":\"/a/wall.png\"}";
        assert_eq!(
            json_string_field(text, "theme_name").as_deref(),
            Some("mocha")
        );
        assert_eq!(
            json_string_field(text, "wallpaper").as_deref(),
            Some("/a/wall.png")
        );
        assert_eq!(json_string_field(text, "generated"), None);
        assert_eq!(json_string_field("not json", "theme_name"), None);
    }

    #[test]
    fn wallpaper_basenames_match_bash_dirname() {
        assert_eq!(
            wallpaper_base(std::path::Path::new("/nonexistent")),
            "(no metadata)"
        );
    }
}
