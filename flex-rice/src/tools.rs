//! Cached tool resolution + shared argv/stdout helpers (OPT-11).
//!
//! Every `exec` module used to carry its own `ambient_path` / `resolve_tool`
//! pair that re-split `$PATH` and re-`stat`ed every directory on every spawn
//! (a `stat` storm on the 1 s tick). The helpers here live exactly once:
//!
//! - [`ambient_dirs`] parses the ambient `$PATH` a single time behind a
//!   [`OnceLock`];
//! - [`resolve_ambient`] memoizes the four hot tools (`nmcli`,
//!   `bluetoothctl`, `wpctl`, `brightnessctl`) each behind its own
//!   [`OnceLock`];
//! - [`resolve_tool`] keeps the stub-`PATH` seam exact (explicit `path_env`
//!   strings are always scanned uncached so tests can shadow the real tools),
//!   but routes ambient-`PATH` lookups through the caches;
//! - [`decode_stdout`] moves valid UTF-8 instead of always lossy-copying.
//!
//! The per-tool caches only cover the ambient `PATH`. Tests pass explicit
//! stub `path_env` values that must shadow the real tools per call, so those
//! never touch the caches.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The ambient `PATH`, empty when unset (tool resolution then fails cleanly
/// instead of inheriting a surprising default).
#[must_use]
pub fn ambient_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// Split a `:`-separated `PATH` value into directories.
#[must_use]
pub fn split_dirs(path_env: &str) -> Vec<PathBuf> {
    path_env.split(':').map(PathBuf::from).collect()
}

/// Ambient `$PATH` directories, parsed once.
static AMBIENT_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Cached ambient `$PATH` directories (parsed once per process).
#[must_use]
pub fn ambient_dirs() -> &'static [PathBuf] {
    AMBIENT_DIRS.get_or_init(|| split_dirs(&ambient_path()))
}

/// Per-tool ambient memo for `nmcli` (OPT-11 hot tool).
static NMCLI_BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
/// Per-tool ambient memo for `bluetoothctl` (OPT-11 hot tool).
static BLUETOOTHCTL_BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
/// Per-tool ambient memo for `wpctl` (OPT-11 hot tool).
static WPCTL_BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
/// Per-tool ambient memo for `brightnessctl` (OPT-11 hot tool).
static BRIGHTNESSCTL_BIN: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Scan `dirs` for the first entry naming an existing file.
fn scan_dirs(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if name.contains('/') {
        let candidate = PathBuf::from(name);
        return candidate.is_file().then_some(candidate);
    }
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Resolve `name` against an explicit `:`-separated `path_env`.
///
/// Always scans uncached so stub-`PATH` tests shadow the real tools without
/// touching the process env. An absolute `name` (the usual override shape)
/// resolves to itself when it names a file.
#[must_use]
pub fn resolve_in(name: &str, path_env: &str) -> Option<PathBuf> {
    scan_dirs(name, &split_dirs(path_env))
}

/// Resolve `name` against the ambient `$PATH`, using the cached dirs and,
/// for the four hot tools, the per-tool memo.
#[must_use]
pub fn resolve_ambient(name: &str) -> Option<PathBuf> {
    let slot = match name {
        "nmcli" => Some(&NMCLI_BIN),
        "bluetoothctl" => Some(&BLUETOOTHCTL_BIN),
        "wpctl" => Some(&WPCTL_BIN),
        "brightnessctl" => Some(&BRIGHTNESSCTL_BIN),
        _ => None,
    };
    if let Some(cell) = slot {
        return cell.get_or_init(|| scan_dirs(name, ambient_dirs())).clone();
    }
    scan_dirs(name, ambient_dirs())
}

/// Resolve `name` against `path_env` (`:`-separated, shell-style).
///
/// Returns the first entry naming an existing file, so stub-`PATH` tests can
/// shadow the real tools without touching the process env. When `path_env`
/// is the ambient `PATH`, the cached dirs (and the per-tool memo for the
/// four hot tools) serve the lookup instead of re-splitting + re-`stat`ing.
#[must_use]
pub fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    if path_env == ambient_path() {
        return resolve_ambient(name);
    }
    resolve_in(name, path_env)
}

/// Decode captured stdout: move valid UTF-8, lossy-convert only on error.
///
/// Replaces the unconditional `String::from_utf8_lossy(..).into_owned()`
/// copy on every snapshot with a zero-copy move on the overwhelmingly
/// common valid-UTF-8 path.
#[must_use]
pub fn decode_stdout(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    }
}

/// Borrowed decode for `&[u8]` snapshots: borrows valid UTF-8, allocates
/// only when lossy fallback kicks in.
#[must_use]
pub fn decode_bytes(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_utf8_moves_without_loss() {
        assert_eq!(decode_stdout(b"ok".to_vec()), "ok");
    }

    #[test]
    fn invalid_utf8_falls_back_to_lossy() {
        assert_eq!(decode_stdout(vec![0xff]), "\u{fffd}");
    }

    #[test]
    fn borrowed_decode_borrows_valid_input() {
        let bytes = b"ssid";
        assert!(matches!(decode_bytes(bytes), Cow::Borrowed(_)));
    }

    #[test]
    fn stub_path_env_resolves_uncached() {
        let dir = std::env::temp_dir().join("flex-tools-test");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let tool = dir.join("flex-tool-probe");
        std::fs::write(&tool, "#!/bin/sh\n").expect("stub tool");
        let path_env = dir.display().to_string();
        assert_eq!(resolve_tool("flex-tool-probe", &path_env), Some(tool));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
