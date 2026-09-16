use flex_rice::exec::mixer;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flex-mixer-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write stub exe");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub exe");
    }
}

fn wait_for_log(path: &Path) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Ok(body) = std::fs::read_to_string(path) {
            if !body.is_empty() {
                return body;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "stub log never appeared: {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn missing_wiremix_fails_cleanly() {
    let dir = scratch("missing-wiremix");
    let log = dir.join("notify.log");
    write_exe(
        &dir.join("notify-send"),
        &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", log.display()),
    );
    let path = dir.display().to_string();
    let res = mixer::toggle(Some(&path));
    assert!(res.is_err(), "toggle must fail when wiremix is missing");
    assert!(
        res.unwrap_err()
            .to_string()
            .contains("wiremix not found on PATH"),
        "error message names wiremix"
    );
    let notify_content = wait_for_log(&log);
    assert!(
        notify_content.contains("wiremix not installed"),
        "notification must be sent: {notify_content:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn toggle_spawns_floating_window_when_idle() {
    let dir = scratch("spawn-idle");
    let call_log = dir.join("calls.log");
    write_exe(&dir.join("wiremix"), "#!/bin/sh\nexit 0\n");
    // pgrep exits 1 (no open mixer)
    write_exe(&dir.join("pgrep"), "#!/bin/sh\nexit 1\n");
    write_exe(
        &dir.join("kitty"),
        &format!(
            "#!/bin/sh\nprintf 'kitty %s\\n' \"$*\" >> '{}'\n",
            call_log.display()
        ),
    );
    let path = dir.display().to_string();
    let res = mixer::toggle(Some(&path));
    assert!(res.is_ok(), "toggle succeeds: {res:?}");
    let calls = wait_for_log(&call_log);
    assert!(
        calls.contains("--class kitty-wiremix"),
        "spawns with class kitty-wiremix: {calls:?}"
    );
    assert!(
        calls.contains("wiremix --tab output"),
        "runs wiremix --tab output: {calls:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn toggle_kills_running_window_when_open() {
    let dir = scratch("kill-open");
    let call_log = dir.join("calls.log");
    write_exe(&dir.join("wiremix"), "#!/bin/sh\nexit 0\n");
    // pgrep exits 0 (mixer open)
    write_exe(&dir.join("pgrep"), "#!/bin/sh\nexit 0\n");
    write_exe(
        &dir.join("pkill"),
        &format!(
            "#!/bin/sh\nprintf 'pkill %s\\n' \"$*\" >> '{}'\n",
            call_log.display()
        ),
    );
    let path = dir.display().to_string();
    let res = mixer::toggle(Some(&path));
    assert!(res.is_ok(), "toggle succeeds: {res:?}");
    let calls = wait_for_log(&call_log);
    assert!(
        calls.contains("-f kitty-wiremix"),
        "pkills kitty-wiremix pattern: {calls:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
