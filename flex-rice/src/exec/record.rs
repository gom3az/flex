//! `record` executor: the ported `recording-start.sh` / `recording-status.sh`
//! / `recording-stop.sh` helpers for `flex-record` and `flex-shot`.
//!
//! `start` spawns `wf-recorder` detached and records `pid|filepath` in
//! [`REC_INFO`] (`/tmp/recording.info`); `stop` interrupts the recorded pid
//! (or every `wf-recorder`); `status` prints the Waybar `REC` badge when one
//! is running (exit 1 otherwise, like the bash `&&`).
//!
//! The original scripts have no `set -e`, so every notify/`kill` failure is
//! best-effort; only the missing-file and already-recording guards are hard
//! errors.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::spawn::RetryExec as _;

/// The pid/file registry (`recording-start.sh`'s `/tmp/recording.info`).
pub const REC_INFO: &str = "/tmp/recording.info";

/// Env override for the registry path (test seam; empty = default).
pub const REC_INFO_ENV: &str = "FLEX_RECORD_INFO";

/// The recorder the helper drives.
const RECORDER: &str = "wf-recorder";

/// The registry path (`$FLEX_RECORD_INFO`, else [`REC_INFO`]).
fn info_path() -> PathBuf {
    match std::env::var(REC_INFO_ENV) {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(REC_INFO),
    }
}

/// The ambient `PATH`, empty when unset.
fn ambient_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// Resolve `name` against `path_env` (`:`-separated, shell-style).
fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

/// Best-effort `notify-send` with the given args (never errors).
fn notify(path_env: &str, args: &[String]) {
    if let Some(bin) = resolve_tool("notify-send", path_env) {
        let _ = Command::new(bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status_retrying();
    }
}

/// Whether a `wf-recorder` process is running (`pgrep -x wf-recorder`).
fn recorder_running(path_env: &str) -> bool {
    let Some(bin) = resolve_tool("pgrep", path_env) else {
        return false;
    };
    Command::new(bin)
        .args(["-x", RECORDER])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status_retrying()
        .is_ok_and(|status| status.success())
}

/// Send `signal` to `pid` via `kill` (best-effort).
fn signal(path_env: &str, signal: &str, pid: &str) {
    if let Some(bin) = resolve_tool("kill", path_env) {
        let _ = Command::new(bin)
            .args(["-s", signal, pid])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status_retrying();
    }
}

/// `start [-a] [-g GEOM] FILE`: spawn `wf-recorder` detached and register it.
///
/// `args` is the recording-start arg vector (`-a`/`-g GEOM`/file), the same
/// shape the `RECORDING_START` seam passes.
///
/// # Errors
///
/// When no file is given or a recording is already running (both notify and
/// bail, like the wrapper), the recorder is missing, or the spawn fails.
pub fn start(args: &[String], path_env: Option<&str>) -> Result<()> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let mut audio = false;
    let mut geometry: Option<String> = None;
    let mut file: Option<String> = None;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "-a" | "--audio" => audio = true,
            "-g" | "--geometry" => {
                index += 1;
                if let Some(value) = args.get(index) {
                    geometry = Some(value.clone());
                }
            }
            other => file = Some(other.to_string()),
        }
        index += 1;
    }
    let Some(file) = file else {
        notify(
            &path_env,
            &[
                String::from("Recording error"),
                String::from("No file path specified"),
            ],
        );
        anyhow::bail!("record: no file path specified");
    };
    if recorder_running(&path_env) {
        notify(
            &path_env,
            &[
                String::from("Recording error"),
                String::from("A recording is already in progress"),
            ],
        );
        anyhow::bail!("record: a recording is already in progress");
    }
    let Some(bin) = resolve_tool(RECORDER, &path_env) else {
        anyhow::bail!("record: {RECORDER} not found on PATH");
    };

    let mut recorder_args: Vec<String> = [
        "-c",
        "av1_vaapi",
        "-r",
        "30",
        "-p",
        "b=5M",
        "-p",
        "maxrate=5M",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    if let Some(geom) = &geometry {
        recorder_args.push(String::from("-g"));
        recorder_args.push(geom.clone());
    }
    if audio {
        recorder_args.push(String::from("--audio-backend=pipewire"));
        recorder_args.push(String::from("-a"));
    }
    recorder_args.push(String::from("-f"));
    recorder_args.push(file.clone());

    if let Some(parent) = Path::new(&file).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("record: cannot create {}", parent.display()))?;
        }
    }

    let child = Command::new(&bin)
        .args(&recorder_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn_retrying()
        .with_context(|| format!("record: failed to start {RECORDER}"))?;
    let info = info_path();
    std::fs::write(&info, format!("{}|{}\n", child.id(), file))
        .with_context(|| format!("record: cannot write {}", info.display()))?;
    notify(&path_env, &[String::from("Recording started"), file]);
    Ok(())
}

/// `status`: print the Waybar `REC` badge when a recorder is running.
///
/// Exits `1` (silently, no runner prefix) when idle, matching the bash
/// `pgrep … && echo`.
///
/// # Errors
///
/// When stdout cannot be written.
pub fn status(path_env: Option<&str>) -> Result<()> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    if !recorder_running(&path_env) {
        std::process::exit(1);
    }
    let mut out = std::io::stdout();
    writeln!(out, "󰓛  REC").context("record: cannot write status")?;
    Ok(())
}

/// `stop`: interrupt the recorded pid (or every `wf-recorder`) and notify.
///
/// # Errors
///
/// When the registry file cannot be removed.
pub fn stop(path_env: Option<&str>) -> Result<()> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let info = info_path();
    if let Ok(body) = std::fs::read_to_string(&info) {
        let _ = std::fs::remove_file(&info);
        let mut parts = body.trim_end_matches('\n').splitn(2, '|');
        let pid = parts.next().unwrap_or_default();
        let file = parts.next().unwrap_or_default();
        if !pid.is_empty() {
            signal(&path_env, "INT", pid);
        }
        let body = if file.is_empty() {
            default_screenshots()
        } else {
            file.to_string()
        };
        notify(&path_env, &[String::from("Recording saved"), body]);
        return Ok(());
    }
    if !recorder_running(&path_env) {
        return Ok(());
    }
    if let Some(bin) = resolve_tool("killall", &path_env) {
        let _ = Command::new(bin)
            .args(["-INT", RECORDER])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status_retrying();
    }
    notify(&path_env, &[String::from("Recording saved")]);
    Ok(())
}

/// `$HOME/Pictures/Screenshots` (the bash fallback notification body).
fn default_screenshots() -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => format!("{home}/Pictures/Screenshots"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_without_a_file_notifies_and_bails() {
        // Empty PATH: no notify/pgrep/wf-recorder, so only the guard fires.
        let err = start(&[], Some("")).expect_err("no file");
        assert!(format!("{err:#}").contains("no file path specified"));
    }
}
