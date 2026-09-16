//! A diagnostic write to a dead stream must not abort a provider.
//!
//! Regression: `eprintln!` panics when the write fails (a hung-up popup pty
//! gives `EIO`, a closed pipe `EPIPE`) and the release profile is
//! `panic = "abort"`, so a provider whose stderr was gone died with SIGABRT
//! instead of falling back to kitty. Live case: a Waybar started before
//! `hl.env("TERMINAL", …)` kept a deleted `/dev/pts/N` on fd 2, and every
//! `flex-wifi`/`flex-power` on-click aborted before spawning its popup.
//!
//! `/dev/full` fails every write (Linux `ENOSPC`), which stands in for the
//! dead stream. `flex-power` is used because it needs no external tools.

use std::process::{Command, Stdio};

#[test]
fn a_dead_stderr_does_not_abort_the_provider() {
    // Empty PATH: `pgrep` is missing, so the toggle errors after the
    // `detect()` warning already tried to write.
    let dir = std::env::temp_dir().join(format!("flex-diag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("empty PATH dir");
    let full = std::fs::File::create("/dev/full").expect("open /dev/full");

    let mut child = Command::new(env!("CARGO_BIN_EXE_flex-power"))
        .env_remove("TERMINAL")
        .env_remove("POPUP_KITTY")
        .env_remove("DRY_RUN")
        .env("PATH", &dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(full))
        .spawn()
        .expect("run flex-power with a failing stderr");
    let status = child.wait().expect("wait for flex-power");
    let _ = std::fs::remove_dir_all(&dir);

    // Before the fix the `detect()` warning panicked and the process died on
    // SIGABRT — `code()` is `None` for a signal-killed process.
    assert_eq!(
        status.code(),
        Some(1),
        "must exit 1 (pgrep missing), not die by signal: {status:?}"
    );
}
