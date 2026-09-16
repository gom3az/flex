//! `flex-theme list|current|activate|delete`: the ported
//! `theme-switcher.sh` verbs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn scratch(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "flex-theme-verbs-{}-{name}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn run_theme(home: &Path, args: &[&str], path_env: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flex-theme"))
        .args(args)
        .env("HOME", home)
        .env("PATH", path_env)
        .env_remove("THEME_SWITCHER")
        .stdin(Stdio::null())
        .output()
        .expect("run flex-theme")
}

fn available(home: &Path) -> PathBuf {
    home.join(".config/themes/available")
}

fn make_theme(home: &Path, name: &str, metadata: Option<&str>) {
    let dir = available(home).join(name);
    std::fs::create_dir_all(&dir).expect("theme dir");
    std::fs::write(dir.join("theme.css"), "body {}\n").expect("theme.css");
    if let Some(meta) = metadata {
        std::fs::write(dir.join("metadata.json"), meta).expect("metadata");
    }
}

#[test]
fn list_prints_each_theme_with_wallpaper_and_generated() {
    let home = scratch("list");
    make_theme(
        &home,
        "alpha",
        Some(r#"{"wallpaper": "/walls/a.png", "generated": "2026-01-01T00:00:00+00:00"}"#),
    );
    make_theme(&home, "beta", None);

    let output = run_theme(&home, &["list"], "");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("alpha"), "alpha listed: {stdout:?}");
    assert!(stdout.contains("a.png"), "wallpaper basename: {stdout:?}");
    assert!(
        stdout.contains("2026-01-01T00:00:00+00:00"),
        "generated: {stdout:?}"
    );
    assert!(
        stdout.contains("(no metadata)"),
        "missing metadata fallback: {stdout:?}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn current_prints_the_active_theme() {
    let home = scratch("current");
    let current = home.join(".config/themes/current");
    std::fs::create_dir_all(&current).expect("current dir");
    std::fs::write(
        current.join("metadata.json"),
        r#"{"theme_name": "alpha", "wallpaper": "/walls/a.png"}"#,
    )
    .expect("metadata");

    let output = run_theme(&home, &["current"], "");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Active theme: alpha\n  Wallpaper: a.png\n"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn activate_copies_the_theme_and_relinks() {
    let home = scratch("activate");
    make_theme(&home, "demo", Some(r#"{"theme_name": "old"}"#));

    let output = run_theme(&home, &["activate", "demo"], "");
    assert!(
        output.status.success(),
        "activate exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let current = home.join(".config/themes/current");
    assert!(current.join("theme.css").is_file(), "theme.css copied");
    let compat = current.join("colors.css");
    assert_eq!(
        std::fs::read_link(&compat).expect("colors.css symlink"),
        Path::new("theme.css"),
        "backward-compat relative link"
    );
    let meta = std::fs::read_to_string(current.join("metadata.json")).expect("metadata");
    assert!(
        meta.contains("\"theme_name\": \"demo\""),
        "metadata name updated: {meta:?}"
    );
    assert!(
        home.join(".config/waybar/theme.css").is_symlink(),
        "config symlink repointed"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn delete_removes_a_theme_and_warns_for_auto() {
    let home = scratch("delete");
    make_theme(&home, "gone", None);
    make_theme(&home, "auto-x", None);

    let output = run_theme(&home, &["delete", "gone"], "");
    assert!(output.status.success());
    assert!(!available(&home).join("gone").exists(), "theme removed");

    let output = run_theme(&home, &["delete", "auto-x"], "");
    assert!(output.status.success());
    assert!(!available(&home).join("auto-x").exists());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("auto"),
        "auto theme warned"
    );

    let output = run_theme(&home, &["delete", "missing"], "");
    assert_eq!(output.status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&home);
}

/// `flex theme list` forwards the verb to `flex-theme`.
#[test]
fn dispatcher_forwards_the_list_verb() {
    let home = scratch("dispatch");
    make_theme(&home, "alpha", None);
    let output = Command::new(env!("CARGO_BIN_EXE_flex"))
        .args(["theme", "list"])
        .env("HOME", &home)
        .env("PATH", "")
        .env_remove("THEME_SWITCHER")
        .stdin(Stdio::null())
        .output()
        .expect("run flex theme list");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("alpha"));
    let _ = std::fs::remove_dir_all(&home);
}
