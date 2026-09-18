//! `mixer` executor: the ported `audio-mixer-toggle.sh` helper for Hyprland
//! and Waybar.
//!
//! Toggles `wiremix` (`PipeWire` TUI mixer) in a floating terminal window with
//! window class [`CLASS`]. If `wiremix` is missing on `$PATH`, a desktop
//! notification is sent and the process exits with an error.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::popup;
use crate::spawn::RetryExec as _;
use crate::terminal;

pub const CLASS: &str = "kitty-wiremix";

const MIXER_BIN: &str = "wiremix";

fn ambient_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

/// Best-effort `notify-send` when a tool is missing.
fn notify_missing(path_env: &str) {
    if let Some(bin) = resolve_tool("notify-send", path_env) {
        let _ = Command::new(bin)
            .args(["wiremix not installed", "Run: sudo dnf install -y wiremix"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status_retrying();
    }
}

/// Whether a window with [`CLASS`] is open (`pgrep -f "kitty-wiremix "`).
fn is_open(path_env: &str) -> Result<bool> {
    let pattern = popup::match_pattern(CLASS);
    let Some(bin) = resolve_tool("pgrep", path_env) else {
        anyhow::bail!("mixer: pgrep not found on PATH");
    };
    let output = Command::new(bin)
        .arg("-f")
        .arg(&pattern)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output_retrying()
        .context("mixer: failed to run pgrep")?;
    Ok(output.status.success())
}

/// Close running mixer windows (`pkill -f "kitty-wiremix "`).
fn close(path_env: &str) -> Result<()> {
    let pattern = popup::match_pattern(CLASS);
    let Some(bin) = resolve_tool("pkill", path_env) else {
        anyhow::bail!("mixer: pkill not found on PATH");
    };
    let _ = Command::new(bin)
        .arg("-f")
        .arg(&pattern)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status_retrying()
        .context("mixer: failed to run pkill")?;
    Ok(())
}

fn spawn(path_env: &str) -> Result<()> {
    let kind = terminal::detect();
    let cmd = vec![
        MIXER_BIN.to_string(),
        "--tab".to_string(),
        "output".to_string(),
    ];
    let argv = terminal::spawn_argv(kind, CLASS, &cmd);
    let Some((program, rest)) = argv.split_first() else {
        anyhow::bail!("mixer: empty spawn argv");
    };
    let bin = if program.contains('/') {
        PathBuf::from(program)
    } else {
        match resolve_tool(program, path_env) {
            Some(path) => path,
            None => anyhow::bail!("mixer: terminal '{program}' not found on PATH"),
        }
    };
    Command::new(bin)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn_retrying()
        .context("mixer: failed to spawn terminal")?;
    Ok(())
}

/// Toggle the wiremix floating mixer window.
///
/// If `wiremix` is not installed on `$PATH`, triggers a notification and
/// returns an error. If the window is already running, it is killed with
/// `pkill`; otherwise it is launched detached with [`CLASS`].
///
/// # Errors
///
/// When `wiremix`, `pgrep`, `pkill`, or the terminal binary is missing or
/// fails.
pub fn toggle(path_override: Option<&str>) -> Result<()> {
    let path_env = match path_override {
        Some(path) => path.to_string(),
        None => ambient_path(),
    };
    if resolve_tool(MIXER_BIN, &path_env).is_none() {
        notify_missing(&path_env);
        anyhow::bail!("mixer: wiremix not found on PATH");
    }
    if is_open(&path_env)? {
        close(&path_env)
    } else {
        spawn(&path_env)
    }
}
