//! `proc` executor: signal the process selected in the native process list.
//!
//! Ports the kill action the retired `kill-menu.sh` delegated to htop, using
//! the same three engine outcomes the other danger providers use:
//!
//! - `Chosen` (Enter, armed because rows are `confirmable`) → **SIGTERM**;
//! - `Delete` (Delete, armed by the `deletable` tab) → **SIGKILL**;
//! - `Toggle` (NAVIGATE `m`) → **SIGSTOP**, or **SIGCONT** when the process
//!   is already stopped (`T`/`t`).
//!
//! Signalling goes through `kill(1)` because `unsafe_code = "deny"` forbids
//! the `libc::kill` syscall; this is a subprocess call, not a shell. The pid
//! comes from the row id and is validated as decimal before use.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::providers::proc as proc_provider;
use crate::spawn::RetryExec as _;

/// A signal the process manager can send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Graceful termination (the `Chosen`/Enter action).
    Term,
    /// Forced termination (the `Delete` action).
    Kill,
    /// Suspend (the `Toggle` action on a running process).
    Stop,
    /// Resume (the `Toggle` action on a stopped process).
    Cont,
}

impl Signal {
    /// The `kill -s` spelling.
    #[must_use]
    fn as_arg(self) -> &'static str {
        match self {
            Self::Term => "TERM",
            Self::Kill => "KILL",
            Self::Stop => "STOP",
            Self::Cont => "CONT",
        }
    }
}

/// The ambient `PATH`, empty when unset (tool resolution then fails cleanly).
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

/// Validate a row id as a decimal pid.
///
/// # Errors
///
/// When `pid` is empty or not all ASCII digits.
fn parse_pid(pid: &str) -> Result<u32> {
    if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
        anyhow::bail!("proc: bad pid '{pid}'");
    }
    pid.parse::<u32>()
        .map_err(|_| anyhow::anyhow!("proc: bad pid '{pid}'"))
}

/// Send `sig` to `pid` via `kill -s`, with an explicit `PATH` seam.
///
/// # Errors
///
/// When the pid is malformed, `kill` is missing, or it exits non-zero
/// (including a vanished process). Messages carry no `flex:` prefix; the
/// runner reports them.
pub fn signal(pid: &str, sig: Signal, path_env: Option<&str>) -> Result<()> {
    let _ = parse_pid(pid)?;
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let Some(bin) = resolve_tool("kill", &path_env) else {
        anyhow::bail!("proc: kill not found on PATH");
    };
    let status = Command::new(&bin)
        .args(["-s", sig.as_arg(), pid])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status_retrying()
        .context("proc: failed to run kill")?;
    if !status.success() {
        anyhow::bail!("proc: kill -{} {pid} failed", sig.as_arg());
    }
    Ok(())
}

/// `Toggle`: SIGCONT a stopped process, else SIGSTOP.
///
/// # Errors
///
/// When the pid is malformed or the signal fails.
pub fn toggle(pid: &str, path_env: Option<&str>) -> Result<()> {
    let state = parse_pid(pid)?;
    let sig = match proc_provider::process_state(state) {
        Some('T' | 't') => Signal::Cont,
        _ => Signal::Stop,
    };
    signal(pid, sig, path_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_validation_accepts_only_decimal() {
        assert_eq!(parse_pid("1234").expect("valid"), 1234);
        for bad in ["", "12a", "-1", "1.5", " 1", "1 ", "0x10"] {
            assert!(parse_pid(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn signal_names_are_the_kill_spellings() {
        assert_eq!(Signal::Term.as_arg(), "TERM");
        assert_eq!(Signal::Kill.as_arg(), "KILL");
        assert_eq!(Signal::Stop.as_arg(), "STOP");
        assert_eq!(Signal::Cont.as_arg(), "CONT");
    }

    #[test]
    fn signal_rejects_a_bad_pid_before_spawning() {
        assert!(signal("nope", Signal::Term, Some("/nonexistent")).is_err());
    }
}
