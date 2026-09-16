//! Entry-point contract: `--help` on all nine binaries, `--version` on the
//! dispatcher.
//!
//! No test here opens a real TUI (no pty available): `--help`/`--version`
//! exit before any menu construction.

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
