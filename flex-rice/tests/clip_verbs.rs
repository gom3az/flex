//! `flex-clip add|pin|unpin|current`: the ported `cliphist.sh` verbs.
//!
//! These verbs are non-interactive (no popup guard, no TUI), so they run
//! detached without a pty. `wl-paste`/`notify-send` are stubbed on `PATH` and
//! the three store files are pinned to a scratch dir via the
//! `CLIPHIST_FILE`/`CLIPHIST_PINS`/`CLIPHIST_CURRENT` seams.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Unique scratch dir per call (tests run in parallel).
fn scratch(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "flex-clip-verbs-it-{}-{name}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Write an executable stub script.
fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).expect("stub script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// Stub `PATH` dir: `wl-paste` cats `payload` and logs argv to `wl_paste_log`,
/// `notify-send` appends its argv to `notify.log`.
struct Stubs {
    dir: PathBuf,
    payload: PathBuf,
    notify_log: PathBuf,
    wl_paste_log: PathBuf,
}

fn install_stubs(name: &str) -> Stubs {
    let dir = scratch(name);
    let payload = dir.join("payload");
    let notify_log = dir.join("notify.log");
    let wl_paste_log = dir.join("wl_paste.log");
    std::fs::write(&payload, b"").expect("payload");
    write_exe(
        &dir.join("wl-paste"),
        &format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > '{}.tmp'\nmv '{}.tmp' '{}'\ncat '{}'\n",
            wl_paste_log.display(),
            wl_paste_log.display(),
            wl_paste_log.display(),
            payload.display(),
        ),
    );
    write_exe(
        &dir.join("notify-send"),
        &format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > '{}.tmp'\nmv '{}.tmp' '{}'\n",
            notify_log.display(),
            notify_log.display(),
            notify_log.display(),
        ),
    );
    Stubs {
        dir,
        payload,
        notify_log,
        wl_paste_log,
    }
}

/// Run `flex-clip` with a verb, the three store seams and a stub `PATH`.
fn run_verb(stubs: &Stubs, home: &Path, args: &[&str]) -> Output {
    let path_env = format!(
        "{}:{}",
        stubs.dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(env!("CARGO_BIN_EXE_flex-clip"))
        .args(args)
        .env("HOME", home)
        .env("PATH", path_env)
        .env("CLIPHIST_FILE", home.join("hist"))
        .env("CLIPHIST_PINS", home.join("pins"))
        .env("CLIPHIST_CURRENT", home.join("current"))
        .stdin(Stdio::null())
        .output()
        .expect("run flex-clip")
}

fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "verb exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn add_encodes_the_clipboard_and_dedupes() {
    let stubs = install_stubs("add");
    let home = stubs.dir.join("home");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(&stubs.payload, b"alpha\nbeta\x00\n").expect("payload");

    assert_ok(&run_verb(&stubs, &home, &["add"]));
    assert_eq!(
        std::fs::read(home.join("current")).expect("current"),
        b"alpha<NEWLINE>beta\n".to_vec()
    );
    assert_eq!(
        std::fs::read(home.join("hist")).expect("hist"),
        b"alpha<NEWLINE>beta\n".to_vec()
    );

    // Re-adding the same entry updates `current` but not the history.
    assert_ok(&run_verb(&stubs, &home, &["add"]));
    assert_eq!(
        std::fs::read(home.join("hist")).expect("hist"),
        b"alpha<NEWLINE>beta\n".to_vec()
    );
    let _ = std::fs::remove_dir_all(&stubs.dir);
}

#[test]
fn pin_unpin_and_current_round_trip() {
    let stubs = install_stubs("pin");
    let home = stubs.dir.join("home");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(&stubs.payload, b"pinned entry\n").expect("payload");

    assert_ok(&run_verb(&stubs, &home, &["add"]));
    assert_ok(&run_verb(&stubs, &home, &["pin"]));
    assert_eq!(
        std::fs::read(home.join("pins")).expect("pins"),
        b"pinned entry\n".to_vec()
    );

    let current = run_verb(&stubs, &home, &["current"]);
    assert_ok(&current);
    assert_eq!(current.stdout, b"pinned entry\n".to_vec());

    let notified = std::fs::read_to_string(&stubs.notify_log).expect("notify log");
    assert!(
        notified.contains("Pinned to history"),
        "pin notified: {notified:?}"
    );

    assert_ok(&run_verb(&stubs, &home, &["unpin"]));
    assert_eq!(
        std::fs::read(home.join("pins")).expect("pins"),
        b"".to_vec(),
        "unpin scrubbed the entry"
    );
    let _ = std::fs::remove_dir_all(&stubs.dir);
}

#[test]
fn pin_of_an_empty_clipboard_reports_nothing_to_pin() {
    let stubs = install_stubs("pin-empty");
    let home = stubs.dir.join("home");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(&stubs.payload, b"").expect("payload");

    assert_ok(&run_verb(&stubs, &home, &["add"]));
    assert_ok(&run_verb(&stubs, &home, &["pin"]));
    assert!(
        !home.join("pins").exists(),
        "nothing to pin must not create the pins file"
    );
    let notified = std::fs::read_to_string(&stubs.notify_log).expect("notify log");
    assert!(
        notified.contains("Nothing to pin"),
        "empty pin notified: {notified:?}"
    );
    let _ = std::fs::remove_dir_all(&stubs.dir);
}

/// `flex clip add` forwards the verb to `flex-clip` (the dispatcher re-exec).
#[test]
fn dispatcher_forwards_the_add_verb() {
    let stubs = install_stubs("dispatch");
    let home = stubs.dir.join("home");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(&stubs.payload, b"via dispatcher\n").expect("payload");

    let path_env = format!(
        "{}:{}",
        stubs.dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_flex"))
        .args(["clip", "add"])
        .env("HOME", &home)
        .env("PATH", path_env)
        .env("CLIPHIST_FILE", home.join("hist"))
        .env("CLIPHIST_PINS", home.join("pins"))
        .env("CLIPHIST_CURRENT", home.join("current"))
        .stdin(Stdio::null())
        .output()
        .expect("run flex clip add");
    assert!(
        output.status.success(),
        "dispatcher verb exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(home.join("hist")).expect("hist"),
        b"via dispatcher\n".to_vec()
    );
    let _ = std::fs::remove_dir_all(&stubs.dir);
}

#[test]
fn watch_verb_spawns_wl_paste_watcher() {
    let stubs = install_stubs("watch");
    let home = stubs.dir.join("home");
    std::fs::create_dir_all(&home).expect("home");

    assert_ok(&run_verb(&stubs, &home, &["watch"]));
    let logged = std::fs::read_to_string(&stubs.wl_paste_log).expect("wl-paste log");
    assert!(
        logged.contains("--type") && logged.contains("text") && logged.contains("--watch"),
        "wl-paste received watcher args: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&stubs.dir);
}

#[test]
fn daemon_verb_alias_spawns_wl_paste_watcher() {
    let stubs = install_stubs("daemon");
    let home = stubs.dir.join("home");
    std::fs::create_dir_all(&home).expect("home");

    assert_ok(&run_verb(&stubs, &home, &["daemon"]));
    let logged = std::fs::read_to_string(&stubs.wl_paste_log).expect("wl-paste log");
    assert!(
        logged.contains("--type") && logged.contains("text") && logged.contains("--watch"),
        "daemon alias invoked watcher: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&stubs.dir);
}

#[test]
fn dispatcher_forwards_the_watch_verb() {
    let stubs = install_stubs("dispatch-watch");
    let home = stubs.dir.join("home");
    std::fs::create_dir_all(&home).expect("home");

    let path_env = format!(
        "{}:{}",
        stubs.dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_flex"))
        .args(["clip", "watch"])
        .env("HOME", &home)
        .env("PATH", path_env)
        .env("CLIPHIST_FILE", home.join("hist"))
        .env("CLIPHIST_PINS", home.join("pins"))
        .env("CLIPHIST_CURRENT", home.join("current"))
        .stdin(Stdio::null())
        .output()
        .expect("run flex clip watch");
    assert!(
        output.status.success(),
        "dispatcher watch verb exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let logged = std::fs::read_to_string(&stubs.wl_paste_log).expect("wl-paste log");
    assert!(
        logged.contains("--type") && logged.contains("text") && logged.contains("--watch"),
        "dispatcher watch spawned watcher: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&stubs.dir);
}
