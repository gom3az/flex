//! Clip provider (M4): `cliphist.sh pick()` rows for `flex clip`.
//!
//! Ports ONLY the `pick()` menu path of
//! `scripts/.config/scripts/cliphist.sh` (the `add` / `pin` / `unpin` /
//! `read_current` entry points stay in bash; the clip executor
//! resolves hashes and performs copies/pins/deletes AFTER the TUI exits):
//!
//! - Store: `${CLIPHIST_FILE:-$HOME/.cache/cliphist}` history lines plus
//!   `${CLIPHIST_PINS:-$HOME/.cache/cliphist.pins}` pins (both env
//!   overrides already exist in bash and are honored here, following the
//!   `SCREENSHOT_DIR` / `THEME_SWITCHER` testability precedent).
//! - Lines are `<NEWLINE>`-encoded (multiline entries carry the literal
//!   placeholder; decoding back to `\n` is the wrapper's job, mirroring the
//!   bash `sed "s/$PLACEHOLDER/\n/g"` on copy).
//! - Cleaning mirrors the bash one-pass stream exactly: drop `NUL`
//!   (`0x00`), `0x1F` (the old engine record separator), and `ESC`
//!   (`0x1B`, terminal-injection risk); map `\t` to a space (row
//!   alignment); decode lossy UTF-8 (`U+FFFD`, never panic on binary
//!   garbage); drop empty lines. `LC_ALL=C` byte matching in bash becomes
//!   byte-level filtering here — argv/pipe only, no shell, so glob
//!   characters (`*?[]`) are always literal.
//! - Order: pins first (file order — deterministic; bash used an assoc
//!   scan whose order is unspecified), then history newest-first (`tac`),
//!   first occurrence wins (dedup).
//! - Row: `id` = [`flex_core::content_hash_hex`] of the full cleaned line (Q2,
//!   stable so the wrapper round-trips entries), `label` = 120-char
//!   preview (`${clean:0:120}` in bash), `meta` = `"📌 Pinned"` on pins.
//! - Tab: standard spec rows (clip is listed under standard mode in
//!   `Docs/UI_UX_doc.md`, and the `📌` meta needs the meta column),
//!   `deletable = true` (Delete+confirm flow), NAVIGATE `m` emits
//!   [`flex_core::keys::KeyOutcome::Toggle`] for pin/unpin (see `keys`).
//!
//! The library never copies, pins, or deletes; it only selects rows.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use flex_core::width::{self, Measured};
use flex_core::{content_hash_hex, Row, RowId, Tab};

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "clip";
/// Tab title (matches the bash `Clipboard` tab).
pub const TAB_NAME: &str = "Clipboard";
/// Meta text marking pinned rows (matches the bash `📌 Pinned` mark).
pub const PINNED_META: &str = "📌 Pinned";
/// Preview length in chars (`${clean:0:120}` in bash `build_clip_rows`).
pub const PREVIEW_CHARS: usize = 120;
/// Multiline placeholder: stored lines carry this literal instead of `\n`.
///
/// Decoding (`<NEWLINE>` → newline) happens in the wrapper on copy,
/// mirroring the bash `sed "s/$PLACEHOLDER/\n/g"`.
pub const PLACEHOLDER: &str = "<NEWLINE>";
/// Env override for the history file (honored by bash too; empty = default).
pub const HIST_ENV: &str = "CLIPHIST_FILE";
/// Env override for the pins file (honored by bash too; empty = default).
pub const PINS_ENV: &str = "CLIPHIST_PINS";

/// One cleaned history line plus its precomputed row rendering data.
///
/// Parsing happens once up front; [`measured`] lets the frame path truncate
/// without re-scanning the label.
#[derive(Debug, Clone)]
pub struct ClipEntry {
    /// Full cleaned stored line (`<NEWLINE>`-encoded; hashed for the id).
    pub full: String,
    /// Menu row: id = content-hash hex, label = 120-char preview.
    pub row: Row,
    /// Precomputed display measurement of the preview label.
    pub measured: Measured,
    /// Whether the line is pinned (pins-first order + `📌` meta).
    pub pinned: bool,
}

/// History file path (`$CLIPHIST_FILE`, else `$HOME/.cache/cliphist`).
#[must_use]
pub fn hist_path() -> PathBuf {
    env_override(HIST_ENV).unwrap_or_else(|| home_dir().join(".cache/cliphist"))
}

/// Pins file path (`$CLIPHIST_PINS`, else `$HOME/.cache/cliphist.pins`).
#[must_use]
pub fn pins_path() -> PathBuf {
    env_override(PINS_ENV).unwrap_or_else(|| home_dir().join(".cache/cliphist.pins"))
}

/// Load entries from the real store (env-overridden paths).
///
/// Never panics and never fails: missing/unreadable files yield no rows
/// from that file; binary garbage degrades to `U+FFFD` (see [`clean_line`]).
#[must_use]
pub fn load_entries() -> Vec<ClipEntry> {
    load_entries_in(&hist_path(), &pins_path())
}

/// [`load_entries`] over explicit paths (tests/fixtures, wrapper probes).
///
/// Pins come first in file order, then history newest-first (`tac`
/// semantics: last file line is shown first); the first occurrence of a
/// cleaned line wins.
#[must_use]
pub fn load_entries_in(hist: &Path, pins: &Path) -> Vec<ClipEntry> {
    let mut out: Vec<ClipEntry> = Vec::new();
    visit_unique(hist, pins, |full, hash, pinned| {
        out.push(entry(full, &hash, pinned));
    });
    out
}

/// Visit every **unique** cleaned line of the store in menu order: pins
/// first (file order), then history newest-first.
///
/// Uniqueness is checked on the content hash, not on the line: the store is
/// an unbounded user history (3.6 MB / 3227 lines on the reference host)
/// and holding a second owned copy of every line just to dedup it doubled
/// the picker's resident set (B-024). The hash is 16 hex chars, so the id
/// set is small next to the lines, and `build` takes each line by value so
/// a caller that does not need the full text can drop it immediately.
///
/// Two distinct lines colliding on the 64-bit hash would already make
/// `resolve` return the wrong entry, so this changes nothing that was not
/// already broken.
fn visit_unique(hist: &Path, pins: &Path, mut build: impl FnMut(String, String, bool)) {
    let mut seen: HashSet<String> = HashSet::new();
    for line in read_cleaned(pins) {
        let hash = content_hash_hex(&line);
        if seen.insert(hash.clone()) {
            build(line, hash, true);
        }
    }
    let mut history = read_cleaned(hist);
    history.reverse();
    for line in history {
        let hash = content_hash_hex(&line);
        if seen.insert(hash.clone()) {
            build(line, hash, false);
        }
    }
}

/// Load rows from the real store (preview labels only).
///
/// The tab needs nothing but the rows, so the full lines are dropped as they
/// are previewed instead of being collected first and thrown away (B-024).
#[must_use]
pub fn load() -> Vec<Row> {
    load_rows_in(&hist_path(), &pins_path())
}

/// [`load`] over explicit paths (unit fixtures).
fn load_rows_in(hist: &Path, pins: &Path) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    visit_unique(hist, pins, |full, hash, pinned| {
        rows.push(clip_row(&full, &hash, pinned));
    });
    rows
}

/// Map entries to [`Row`]s (id/label/meta only; full text stays in entries).
#[must_use]
pub fn rows(entries: &[ClipEntry]) -> Vec<Row> {
    entries.iter().map(|entry| entry.row.clone()).collect()
}

/// Build the `Clipboard` tab: standard spec rows, deletable, `m` = pin.
///
/// `m` handling lives in `keys` (NAVIGATE `m` on deletable tabs emits
/// `Toggle`); this constructor only opts the tab into that flow.
#[must_use]
pub fn clip_tab() -> Tab {
    tab_from_rows(load())
}

/// Build a `Clipboard` tab from pre-parsed entries (tests/replays).
#[must_use]
pub fn tab_from_entries(entries: &[ClipEntry]) -> Tab {
    tab_from_rows(rows(entries))
}

/// Wrap rows in the deletable `Clipboard` tab shape.
fn tab_from_rows(rows: Vec<Row>) -> Tab {
    let mut tab = Tab::with_rows(TAB_NAME, rows);
    tab.deletable = true;
    tab
}

/// Resolve a content-hash id back to its full stored (`<NEWLINE>`-encoded)
/// line, for the wrapper (`flex clip --resolve`). Returns `None` on unknown
/// (stale) ids or unreadable stores.
#[must_use]
pub fn resolve(hash: &str) -> Option<String> {
    resolve_in(&hist_path(), &pins_path(), hash)
}

/// [`resolve`] over explicit paths (tests/fixtures).
#[must_use]
pub fn resolve_in(hist: &Path, pins: &Path, hash: &str) -> Option<String> {
    // Walks the store with the same dedup and ordering rules, but builds
    // neither previews nor measurements: the lookup only compares hashes
    // (B-024 — `flex clip --resolve` runs on every selection).
    let mut found: Option<String> = None;
    visit_unique(hist, pins, |full, entry_hash, _pinned| {
        if found.is_none() && entry_hash == hash {
            found = Some(full);
        }
    });
    found
}

/// Decode a stored line for copying (`<NEWLINE>` → real newlines).
///
/// Mirrors the wrapper's `sed "s/$PLACEHOLDER/\n/g"`; kept here so tests
/// pin the exact decode contract.
#[must_use]
pub fn decode(encoded: &str) -> String {
    encoded.replace(PLACEHOLDER, "\n")
}

/// Read a store file into cleaned non-empty lines (file order).
///
/// Missing/unreadable files yield no lines (the bash `pick()` notifies on a
/// missing history file instead; the binary owns that UX, not the parser).
fn read_cleaned(path: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    bytes
        .split(|byte| *byte == b'\n')
        .filter_map(clean_line)
        .collect()
}

/// Clean one raw file line: drop `NUL`/`0x1F`/`ESC`, map `\t` to space,
/// decode lossy UTF-8. Returns `None` for lines left empty (bash
/// `grep -av '^$'` drops those from the merged stream).
///
/// Byte-level on purpose: bash matches with `LC_ALL=C` so invalid multibyte
/// data never breaks matching, and glob characters stay literal (no shell
/// ever sees this text — Rust argv/pipe only).
fn clean_line(raw: &[u8]) -> Option<String> {
    let mut kept: Vec<u8> = Vec::with_capacity(raw.len());
    for byte in raw {
        match byte {
            0x00 | 0x1f | 0x1b => {}
            0x09 => kept.push(b' '),
            _ => kept.push(*byte),
        }
    }
    let line = String::from_utf8_lossy(&kept).into_owned();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Build one [`ClipEntry`]: hash id, 120-char preview, pin meta, widths.
///
/// `hash` is the caller's already-computed [`content_hash_hex`] of `full`
/// (the dedup in [`visit_unique`] needs it anyway — hashing twice was pure
/// waste).
fn entry(full: String, hash: &str, pinned: bool) -> ClipEntry {
    let row = clip_row(&full, hash, pinned);
    let measured = width::measure(&row.label);
    ClipEntry {
        full,
        row,
        measured,
        pinned,
    }
}

/// Preview row for one cleaned line: id = content-hash hex, label = the
/// first [`PREVIEW_CHARS`] characters, meta = `📌` when pinned.
fn clip_row(full: &str, hash: &str, pinned: bool) -> Row {
    let preview: String = full.chars().take(PREVIEW_CHARS).collect();
    let id = RowId::new(hash.to_string());
    if pinned {
        Row::with_meta(id, preview, PINNED_META)
    } else {
        Row::new(id, preview)
    }
}

/// Non-empty env override, if set.
fn env_override(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `$HOME` (empty when unset, matching the theme provider's fallback).
fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleaning_drops_nul_unit_separator_esc_and_expands_tabs() {
        let raw = b"a\x00b\x1fc\x1bd\te";
        assert_eq!(clean_line(raw).as_deref(), Some("abcd e"));
    }

    #[test]
    fn cleaning_is_lossy_never_panics_and_drops_empties() {
        assert_eq!(clean_line(b"\x00\x1f\x1b").as_deref(), None);
        assert_eq!(clean_line(b"").as_deref(), None);
        assert_eq!(clean_line(b"\xff\xfe").as_deref(), Some("��"));
    }

    #[test]
    fn preview_truncates_at_120_chars_but_full_is_kept() {
        let full = "x".repeat(200);
        let hash = content_hash_hex(&full);
        let got = entry(full.clone(), &hash, false);
        assert_eq!(got.row.label.chars().count(), PREVIEW_CHARS);
        assert_eq!(got.full, full);
        assert_eq!(got.row.id.as_str(), hash.as_str());
    }

    #[test]
    fn row_only_and_entry_paths_agree() {
        // The tab builds rows straight from the store (`load_rows_in`,
        // dropping each full line as it previews it) while
        // `load_entries_in` keeps the full lines; both must yield the same
        // menu, or the memory shortcut changed what the picker shows
        // (B-024 refactor guard).
        let dir = std::env::temp_dir().join(format!("flex-clip-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let hist = dir.join("hist");
        let pins = dir.join("pins");
        std::fs::write(&hist, "one\ntwo\none\nthree\n").expect("hist");
        std::fs::write(&pins, "pinned\n").expect("pins");
        let from_entries = rows(&load_entries_in(&hist, &pins));
        let tab_rows = load_rows_in(&hist, &pins);
        std::fs::remove_dir_all(&dir).expect("cleanup");
        let shape: Vec<(&str, &str, Option<&str>)> = tab_rows
            .iter()
            .map(|row| (row.id.as_str(), row.label.as_str(), row.meta.as_deref()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (
                    content_hash_hex("pinned").as_str(),
                    "pinned",
                    Some(PINNED_META)
                ),
                (content_hash_hex("three").as_str(), "three", None),
                (content_hash_hex("one").as_str(), "one", None),
                (content_hash_hex("two").as_str(), "two", None),
            ],
            "pins first, history newest-first, duplicates collapsed to the first"
        );
        assert_eq!(
            from_entries
                .iter()
                .map(|row| (row.id.as_str(), row.label.as_str(), row.meta.as_deref()))
                .collect::<Vec<_>>(),
            shape
        );
    }

    #[test]
    fn decode_restores_newlines() {
        assert_eq!(decode("a<NEWLINE>b"), "a\nb");
    }
}
