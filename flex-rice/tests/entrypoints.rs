//! Entry-point contract: `--help` on all nine binaries, `--version` on the
//! dispatcher.
//!
//! No test here opens a real TUI (no pty available): `--help`/`--version`
//! exit before any menu construction.

use std::path::PathBuf;
use std::process::Output;

/// Unique scratch `HOME` per call: these tests run in parallel and must not
/// share a path.
fn scratch_home(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("flex-entrypoints-{name}-{seq}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch HOME");
    dir
}

/// Run a binary detached from any ctty (`setsid`, so `backend::init` fails
/// deterministically), inside a popup, with a scratch `HOME`; stdout and
/// stderr are captured for byte comparison.
fn run_detached(bin: &str, args: &[&str], home: &std::path::Path) -> Output {
    std::process::Command::new("setsid")
        .arg(bin)
        .args(args)
        .env("HOME", home)
        .env("POPUP_KITTY", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run binary without a controlling terminal")
}

/// All nine entry points answer `--help` with exit 0.
#[test]
fn every_binary_answers_help_with_exit_0() {
    let bins: &[&str] = &[
        env!("CARGO_BIN_EXE_flex"),
        env!("CARGO_BIN_EXE_flex-power"),
        env!("CARGO_BIN_EXE_flex-launch"),
        env!("CARGO_BIN_EXE_flex-shot"),
        env!("CARGO_BIN_EXE_flex-theme"),
        env!("CARGO_BIN_EXE_flex-clip"),
        env!("CARGO_BIN_EXE_flex-center"),
        env!("CARGO_BIN_EXE_flex-wallpaper"),
        env!("CARGO_BIN_EXE_flex-wifi"),
        env!("CARGO_BIN_EXE_flex-proc"),
        env!("CARGO_BIN_EXE_flex-record"),
        env!("CARGO_BIN_EXE_flex-mixer"),
        env!("CARGO_BIN_EXE_flex-net"),
    ];
    for bin in bins {
        let output = std::process::Command::new(bin)
            .arg("--help")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run --help");
        assert!(
            output.status.success(),
            "{bin} --help exits 0: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.stdout.is_empty(),
            "{bin} --help prints usage to stdout"
        );
    }
}

/// The dispatcher answers `--version` with exit 0.
#[test]
fn dispatcher_version_exits_0() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex"))
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex --version");
    assert!(
        output.status.success(),
        "flex --version exits 0: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("flex"),
        "version names the binary"
    );
}

/// Every provider binary answers `--version` with exit 0.
#[test]
fn provider_binaries_answer_version_with_exit_0() {
    let bins: &[&str] = &[
        env!("CARGO_BIN_EXE_flex-power"),
        env!("CARGO_BIN_EXE_flex-launch"),
        env!("CARGO_BIN_EXE_flex-shot"),
        env!("CARGO_BIN_EXE_flex-theme"),
        env!("CARGO_BIN_EXE_flex-clip"),
        env!("CARGO_BIN_EXE_flex-center"),
        env!("CARGO_BIN_EXE_flex-wallpaper"),
        env!("CARGO_BIN_EXE_flex-wifi"),
        env!("CARGO_BIN_EXE_flex-proc"),
        env!("CARGO_BIN_EXE_flex-record"),
        env!("CARGO_BIN_EXE_flex-mixer"),
        env!("CARGO_BIN_EXE_flex-net"),
    ];
    for bin in bins {
        let output = std::process::Command::new(bin)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run --version");
        assert!(
            output.status.success(),
            "{bin} --version exits 0: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `flex launch --print-action` and `flex-launch --print-action` agree: the
/// dispatcher's canonical re-exec reaches the same probe path, so the two
/// runs fail identically on the tty probe (`setsid`, no controlling
/// terminal) with byte-identical stdout/stderr and the same exit code.
#[test]
fn dispatcher_and_provider_agree_on_the_print_action_probe() {
    let home = scratch_home("print-action");
    let direct = run_detached(
        env!("CARGO_BIN_EXE_flex-launch"),
        &["--print-action"],
        &home,
    );
    let via_dispatcher = run_detached(
        env!("CARGO_BIN_EXE_flex"),
        &["launch", "--print-action"],
        &home,
    );
    std::fs::remove_dir_all(&home).expect("cleanup");

    for (label, output) in [("flex-launch", &direct), ("flex launch", &via_dispatcher)] {
        assert!(
            !output.status.success(),
            "{label} --print-action exits non-zero without a tty"
        );
        assert!(!output.stderr.is_empty(), "{label} stderr is non-empty");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.matches("flex:").count(),
            1,
            "{label}: exactly one `flex:` prefix: {stderr:?}"
        );
    }
    assert_eq!(
        direct.stdout, via_dispatcher.stdout,
        "dispatcher and provider print identical probe output"
    );
    assert_eq!(
        direct.stderr, via_dispatcher.stderr,
        "dispatcher and provider emit identical stderr"
    );
    assert_eq!(
        direct.status.code(),
        via_dispatcher.status.code(),
        "dispatcher forwards the provider's exit code"
    );
}
