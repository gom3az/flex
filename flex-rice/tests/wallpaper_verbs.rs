//! `flex-wallpaper set <path>`: the ported `set-wallpaper.sh`.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn scratch(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "flex-wallpaper-set-{}-{name}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn set_writes_the_hyprpaper_conf() {
    let home = scratch("set");
    let wall = home.join("wall.png");
    std::fs::write(&wall, b"png").expect("wallpaper");
    let runtime = home.join("run");
    std::fs::create_dir_all(&runtime).expect("runtime dir");
    std::fs::create_dir_all(home.join(".config/hypr")).expect("hypr dir");

    // No hyprpaper/socat/notify tools: the setter is best-effort and still
    // persists the conf.
    let output = Command::new(env!("CARGO_BIN_EXE_flex-wallpaper"))
        .args(["set", wall.to_str().expect("utf8")])
        .env("HOME", &home)
        .env("PATH", "")
        .env("XDG_RUNTIME_DIR", &runtime)
        .stdin(Stdio::null())
        .output()
        .expect("run flex-wallpaper set");
    assert!(
        output.status.success(),
        "set exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let conf = std::fs::read_to_string(home.join(".config/hypr/hyprpaper.conf"))
        .expect("hyprpaper.conf written");
    let absolute = std::fs::canonicalize(&wall).expect("canonical");
    assert_eq!(
        conf,
        format!(
            "preload = {}\nwallpaper = , {}\n",
            absolute.display(),
            absolute.display()
        ),
        "conf bytes match the wrapper"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn set_rejects_a_missing_file() {
    let home = scratch("missing");
    let output = Command::new(env!("CARGO_BIN_EXE_flex-wallpaper"))
        .args(["set", "/no/such/wall.png"])
        .env("HOME", &home)
        .env("PATH", "")
        .stdin(Stdio::null())
        .output()
        .expect("run flex-wallpaper set");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("flex: error: "),
        "runner prefix"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// `flex wallpaper set` forwards the verb to `flex-wallpaper`.
#[test]
fn dispatcher_forwards_the_set_verb() {
    let home = scratch("dispatch");
    let wall = home.join("wall.png");
    std::fs::write(&wall, b"png").expect("wallpaper");
    let runtime = home.join("run");
    std::fs::create_dir_all(&runtime).expect("runtime dir");
    std::fs::create_dir_all(home.join(".config/hypr")).expect("hypr dir");
    let output = Command::new(env!("CARGO_BIN_EXE_flex"))
        .args(["wallpaper", "set", wall.to_str().expect("utf8")])
        .env("HOME", &home)
        .env("PATH", "")
        .env("XDG_RUNTIME_DIR", &runtime)
        .stdin(Stdio::null())
        .output()
        .expect("run flex wallpaper set");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.join(".config/hypr/hyprpaper.conf").is_file());
    let _ = std::fs::remove_dir_all(&home);
}
