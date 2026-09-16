//! Wallpaper provider: image scan into rows for `flex wallpaper`.
//!
//! Cut over from `scripts/.config/scripts/wallpaper-picker.sh`, which listed
//! `find` output into fzf. Row-set parity with that script:
//!
//! - Directories (in order): `$HOME/Pictures/Wallpapers`, then
//!   `$HOME/Pictures/Screenshots` (`WALLPAPER_DIRS`, colon-separated,
//!   overrides both).
//! - `find <dir> -maxdepth 2 -type f \( -iname '*.jpg' -o -iname '*.jpeg'
//!   -o -iname '*.png' -o -iname '*.webp' \)`: regular files only — a
//!   symlinked image is skipped, exactly like `find` without `-L` — matched
//!   by name suffix (`-iname` is case-insensitive), then `sort -u` over the
//!   full paths. Sorting is byte order here (the bash pipeline is
//!   locale-dependent; ASCII paths, the reference case, agree).
//! - Label = file name (the bash `basename`), meta = parent directory name,
//!   plus [`ACTIVE_SUFFIX`] on the wallpaper that is currently set — the
//!   same `meta + "  Active"` fold [`theme_`](crate::providers::theme_)
//!   uses, and the reason this tab is not bare-rows.
//! - Row id = [`content_hash_hex`] of the absolute path (Q2 style), so ids
//!   stay free of spaces and path separators on the `ACTION:` line; the
//!   wrapper turns the id back into a path with the hidden
//!   `flex wallpaper --resolve <id>` lookup, exactly like `flex clip`.
//! - Rows also carry [`Row::preview_image`], the path the kitty-graphics
//!   preview pane draws.
//!
//! The library never sets a wallpaper; [`set-wallpaper.sh`] runs from the
//! `flex-wallpaper.sh` wrapper after the TUI exits.
//!
//! [`set-wallpaper.sh`]: ../../../../scripts/.config/scripts/set-wallpaper.sh

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use flex_core::{content_hash_hex, Row, RowId, Tab};

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "wallpaper";
/// Tab title (the bash picker's `Wallpaper>` prompt).
pub const TAB_NAME: &str = "Wallpapers";
/// Meta suffix marking the wallpaper currently in use (theme-provider style).
pub const ACTIVE_SUFFIX: &str = "  Active";
/// Image name suffixes the bash `find -iname` list accepts.
pub const IMAGE_SUFFIXES: [&str; 4] = [".jpg", ".jpeg", ".png", ".webp"];
/// `find -maxdepth 2`.
pub const MAX_DEPTH: u32 = 2;
/// `WALLPAPER_DIRS` — colon-separated scan roots (test/override seam).
pub const DIRS_ENV: &str = "WALLPAPER_DIRS";
/// `WALLPAPER_STATE` — file holding the active wallpaper path (test seam).
pub const STATE_ENV: &str = "WALLPAPER_STATE";

/// One scanned wallpaper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperEntry {
    /// Absolute path; also the source of the [`Row`] id.
    pub path: PathBuf,
    /// File name (row label, bash `basename`).
    pub name: String,
    /// Parent directory name (row meta).
    pub dir: String,
    /// Whether this is the wallpaper currently in use.
    pub active: bool,
}

/// Scan roots: `$HOME/Pictures/Wallpapers` + `$HOME/Pictures/Screenshots`
/// (bash `WALLPAPER_DIRS`), unless `WALLPAPER_DIRS` overrides them.
#[must_use]
pub fn wallpaper_dirs() -> Vec<PathBuf> {
    if let Ok(value) = std::env::var(DIRS_ENV) {
        let dirs: Vec<PathBuf> = value
            .split(':')
            .filter(|part| !part.is_empty())
            .map(PathBuf::from)
            .collect();
        if !dirs.is_empty() {
            return dirs;
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        PathBuf::from(&home).join("Pictures/Wallpapers"),
        PathBuf::from(&home).join("Pictures/Screenshots"),
    ]
}

/// File holding the active wallpaper path (`set-wallpaper.sh` writes here).
#[must_use]
pub fn state_file() -> PathBuf {
    if let Ok(path) = std::env::var(STATE_ENV) {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cache/ml4w/hyprland-dotfiles/current_wallpaper")
}

/// Path of the wallpaper currently in use.
///
/// Reads the ml4w cache file `set-wallpaper.sh` writes, falling back to
/// `hyprpaper.conf`; `None` when neither is readable (no row is then marked
/// active, and nothing else changes).
#[must_use]
pub fn current_wallpaper() -> Option<PathBuf> {
    if let Ok(text) = std::fs::read_to_string(state_file()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    hyprpaper_wallpaper()
}

/// `wallpaper = , <path>` from `~/.config/hypr/hyprpaper.conf`.
fn hyprpaper_wallpaper() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let text =
        std::fs::read_to_string(PathBuf::from(home).join(".config/hypr/hyprpaper.conf")).ok()?;
    parse_hyprpaper_conf(&text)
}

/// First usable `wallpaper = …` value in a `hyprpaper.conf`.
///
/// The file format is `wallpaper = , <path>` (empty monitor list = all
/// monitors); `preload = …` lines are ignored, as are unparsable ones.
#[must_use]
pub fn parse_hyprpaper_conf(text: &str) -> Option<PathBuf> {
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("wallpaper") else {
            continue;
        };
        let Some((_, value)) = rest.split_once('=') else {
            continue;
        };
        let path = value.rsplit(',').next().unwrap_or_default().trim();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

/// Scan `dirs` (in order) for wallpapers, sorted and deduped like
/// `find … | sort -u`. `current` marks the active row's meta.
///
/// Never panics and never fails: unreadable directories are skipped, so the
/// result may be empty (the caller reports that and exits cancelled).
#[must_use]
pub fn scan(dirs: &[PathBuf], current: Option<&Path>) -> Vec<WallpaperEntry> {
    let current = current.map(|path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        collect(dir, 1, &mut found);
    }
    found.sort();
    found.dedup();
    found
        .iter()
        .map(|path| entry_for(path, current.as_deref()))
        .collect()
}

/// Load rows from the real directories and the real active-wallpaper state.
#[must_use]
pub fn load() -> Vec<Row> {
    rows(&scan(&wallpaper_dirs(), current_wallpaper().as_deref()))
}

/// Map entries to [`Row`]s.
///
/// id = path hash, label = file name, meta = parent directory name (+
/// [`ACTIVE_SUFFIX`] when active), and [`Row::preview_image`] = the path when
/// it is valid UTF-8 (a non-UTF-8 path keeps its row, just without a preview).
#[must_use]
pub fn rows(entries: &[WallpaperEntry]) -> Vec<Row> {
    entries
        .iter()
        .map(|entry| {
            let id = RowId::new(entry_id(&entry.path));
            let meta = if entry.active {
                format!("{}{ACTIVE_SUFFIX}", entry.dir)
            } else {
                entry.dir.clone()
            };
            let row = Row::with_meta(id, entry.name.clone(), meta);
            match entry.path.to_str() {
                Some(path) => row.with_preview_image(path),
                None => row,
            }
        })
        .collect()
}

/// Build the `Wallpapers` tab: standard spec rows (meta column, tab bar),
/// non-deletable.
#[must_use]
pub fn wallpaper_tab() -> Tab {
    let mut tab = Tab::with_rows(TAB_NAME, load());
    tab.bare_rows = false;
    tab
}

/// Build a `Wallpapers` tab from pre-scanned entries (tests/replays).
#[must_use]
pub fn tab_from_entries(entries: &[WallpaperEntry]) -> Tab {
    let mut tab = Tab::with_rows(TAB_NAME, rows(entries));
    tab.bare_rows = false;
    tab
}

/// The row id for `path` (FNV-1a hex of the path).
///
/// Wrappers hand the id back to
/// [`resolve`](crate::providers::wallpaper::resolve); ids stay space-free so
/// the space-delimited `ACTION:` line parses with plain word splitting, just
/// like the `clip` provider's content hashes.
#[must_use]
pub fn entry_id(path: &Path) -> String {
    content_hash_hex(&path.to_string_lossy())
}

/// Resolve a row id back to its wallpaper path (hidden wrapper lookup).
///
/// Returns `None` for unknown ids — including ids whose file disappeared
/// since the menu was shown.
#[must_use]
pub fn resolve(id: &str) -> Option<PathBuf> {
    resolve_in(&wallpaper_dirs(), id)
}

/// [`resolve`](crate::providers::wallpaper::resolve) over explicit roots
/// (tests/fixtures).
#[must_use]
pub fn resolve_in(dirs: &[PathBuf], id: &str) -> Option<PathBuf> {
    scan(dirs, None)
        .into_iter()
        .find(|entry| entry_id(&entry.path) == id)
        .map(|entry| entry.path)
}

/// Recursively collect image files up to [`MAX_DEPTH`].
fn collect(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.filter_map(Result::ok) {
        let path = entry.path();
        // `symlink_metadata`: `find -type f` does not follow symlinks, so a
        // symlinked wallpaper is invisible to bash and to flex alike.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                collect(&path, depth + 1, out);
            }
        } else if meta.is_file() && is_image_name(&path) {
            out.push(path);
        }
    }
}

/// Whether the file name ends with one of [`IMAGE_SUFFIXES`]
/// (`find -iname '*.jpg'` semantics: case-insensitive suffix, `.jpg` itself
/// matches).
#[must_use]
pub fn is_image_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    IMAGE_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// Build one entry (parent directory name, active flag).
fn entry_for(path: &Path, current: Option<&Path>) -> WallpaperEntry {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string();
    let dir = path
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string();
    WallpaperEntry {
        path: path.to_path_buf(),
        name,
        dir,
        active: is_active(path, current),
    }
}

/// Whether `path` is the wallpaper in use (path or resolved-path equality).
fn is_active(path: &Path, current: Option<&Path>) -> bool {
    let Some(current) = current else {
        return false;
    };
    if path == current {
        return true;
    }
    path.canonicalize()
        .is_ok_and(|resolved| resolved == current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flex-wallpaper-unit-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn image_suffixes_match_find_iname() {
        for name in [
            "a.jpg", "a.JPG", "a.jpeg", "a.JPEG", "a.png", "a.PnG", "a.webp", ".jpg",
        ] {
            assert!(is_image_name(Path::new(name)), "{name} matches");
        }
        for name in ["a.gif", "a.jpgg", "jpg", "a.jpg.txt", "a.PNGX", "noext"] {
            assert!(!is_image_name(Path::new(name)), "{name} does not match");
        }
    }

    #[test]
    fn scan_stops_at_maxdepth_two_and_sorts() {
        let root = scratch("depth");
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).expect("deep dir");
        std::fs::write(root.join("z.png"), b"x").expect("write");
        std::fs::write(root.join("a/m.jpg"), b"x").expect("write");
        std::fs::write(root.join("a/b/too-deep.png"), b"x").expect("write");
        std::fs::write(root.join("a/b/c/way-too-deep.png"), b"x").expect("write");
        std::fs::write(root.join("a/notes.txt"), b"x").expect("write");

        let entries = scan(std::slice::from_ref(&root), None);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["m.jpg", "z.png"], "depth 1 + 2, sorted by path");

        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn hyprpaper_conf_parsing_is_tolerant() {
        let conf = "preload = /tmp/a.jpg\nwallpaper = , /home/u/Pictures/Wallpapers/a b.jpg\n";
        assert_eq!(
            parse_hyprpaper_conf(conf),
            Some(PathBuf::from("/home/u/Pictures/Wallpapers/a b.jpg")),
            "empty monitor list keeps the path, spaces and all"
        );
        assert_eq!(
            parse_hyprpaper_conf("wallpaper = DP-1, /w/x.png"),
            Some(PathBuf::from("/w/x.png")),
            "explicit monitor list"
        );
        assert_eq!(parse_hyprpaper_conf("preload = /only/preload.jpg"), None);
        assert_eq!(parse_hyprpaper_conf("wallpaper = ,\n"), None);
        assert_eq!(parse_hyprpaper_conf(""), None);
    }
}
