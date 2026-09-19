//! Error-prefix contract across all eight provider binaries (B-022/B-027).
//!
//! The shared [`runner`] owns the single `flex: error:` prefix plus
//! `exit(1)`; errors below it carry no `flex:` prefix of their own. Each
//! provider binary is run detached from any ctty (`setsid`, so
//! `backend::init` fails deterministically) with `POPUP_KITTY=1` (past the
//! popup guard, into the menu) and a scratch `HOME` (empty stores, no
//! diagnostics), asserting exactly one `flex:` on stderr.
//!
//! No test here opens a real TUI (no pty available).
//!
//! [`runner`]: flex_rice::runner

use std::path::PathBuf;
use std::process::Output;

/// Unique scratch `HOME` per call: these tests run in parallel and must not
/// share a path.
fn scratch_home(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("flex-prefix-{name}-{seq}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch HOME");
    dir
}

/// Run a provider binary detached from any ctty, inside a popup, with a
/// scratch `HOME`: menu construction sees empty stores and the TTY probe
/// fails deterministically.
fn run_detached(bin: &str, home: &std::path::Path) -> Output {
    std::process::Command::new("setsid")
        .arg(bin)
        .env("HOME", home)
        .env("POPUP_KITTY", "1")
        .env_remove("CLIPHIST_FILE")
        .env_remove("CLIPHIST_PINS")
        .env_remove("WALLPAPER_DIRS")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output()
        .expect("run provider binary without a controlling terminal")
}

/// Six providers build a menu and then fail the TTY probe: one `flex:`
/// prefix, no output, non-zero exit.
#[test]
fn providers_report_tty_errors_with_a_single_prefix() {
    let cases: &[&str] = &[
        env!("CARGO_BIN_EXE_flex-power"),
        env!("CARGO_BIN_EXE_flex-launch"),
        env!("CARGO_BIN_EXE_flex-shot"),
        env!("CARGO_BIN_EXE_flex-theme"),
        env!("CARGO_BIN_EXE_flex-wifi"),
        env!("CARGO_BIN_EXE_flex-proc"),
        env!("CARGO_BIN_EXE_flex-profile"),
    ];
    for bin in cases {
        let home = scratch_home("tty");
        let output = run_detached(bin, &home);
        std::fs::remove_dir_all(&home).expect("cleanup");
        assert!(
            !output.status.success(),
            "{bin} exits non-zero without a tty"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.matches("flex:").count(),
            1,
            "{bin}: exactly one `flex:` prefix: {stderr:?}"
        );
        assert!(
            stderr
                .starts_with("flex: error: cannot open /dev/tty (needs a controlling terminal): "),
            "{bin}: engine message carries no prefix of its own: {stderr:?}"
        );
    }
}

/// Empty `clip`/`wallpaper` stores keep the historical early exit: one
/// `flex: <provider>:` diagnostic line, exit 130, no menu.
#[test]
fn empty_store_providers_exit_130_with_a_single_prefix() {
    let cases: &[(&str, &str, i32)] = &[
        (
            env!("CARGO_BIN_EXE_flex-clip"),
            "flex: clip: no history yet\n",
            130,
        ),
        (
            env!("CARGO_BIN_EXE_flex-wallpaper"),
            "flex: wallpaper: no wallpapers found\n",
            130,
        ),
    ];
    for (bin, expected, code) in cases {
        let home = scratch_home("empty");
        let output = run_detached(bin, &home);
        std::fs::remove_dir_all(&home).expect("cleanup");
        assert_eq!(
            output.status.code(),
            Some(*code),
            "{bin} exits {code} on an empty store"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(stderr, *expected, "{bin}: exact early-exit line");
        assert_eq!(
            stderr.matches("flex:").count(),
            1,
            "{bin}: exactly one `flex:` prefix: {stderr:?}"
        );
    }
}
