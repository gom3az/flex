//! `profile` executor: the power-profile selection flow (`powerprofilesctl`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::spawn::RetryExec as _;

/// A validated profile action id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileAction {
    /// `performance` → `powerprofilesctl set performance`.
    Performance,
    /// `balanced` → `powerprofilesctl set balanced`.
    Balanced,
    /// `power-saver` → `powerprofilesctl set power-saver`.
    PowerSaver,
    /// Well-formed but unsupported id.
    Unknown(String),
}

impl ProfileAction {
    /// Validate + parse a menu action id.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        if id.is_empty() || id.contains('/') || id.contains('\n') {
            return None;
        }
        Some(match id {
            "performance" | "profile:performance" => Self::Performance,
            "balanced" | "profile:balanced" => Self::Balanced,
            "power-saver" | "profile:power-saver" => Self::PowerSaver,
            _ => Self::Unknown(id.to_string()),
        })
    }
}

/// A fully-decided profile action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    /// `powerprofilesctl set performance`.
    Performance,
    /// `powerprofilesctl set balanced`.
    Balanced,
    /// `powerprofilesctl set power-saver`.
    PowerSaver,
}

impl PlannedAction {
    /// Action spelling.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Balanced => "balanced",
            Self::PowerSaver => "power-saver",
        }
    }
}

/// One profile step: a tool spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `powerprofilesctl set <profile>`.
    PowerprofilesctlSet(String),
}

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// The action id that ran.
    pub action_id: String,
    /// The profile arm spelling.
    pub detail: String,
    /// `Some(lines)` when `DRY_RUN` was on; `None` when the steps really ran.
    pub dry_run: Option<Vec<String>>,
}

/// Build the [`Step`]s for a decided [`PlannedAction`].
#[must_use]
pub fn plan(planned: &PlannedAction) -> Vec<Step> {
    vec![Step::PowerprofilesctlSet(planned.as_str().to_string())]
}

/// Render one [`Step`] as a single snapshot line.
#[must_use]
pub fn describe(step: &Step) -> String {
    match step {
        Step::PowerprofilesctlSet(profile) => format!("powerprofilesctl set {profile}"),
    }
}

/// Render a whole [`plan`] as snapshot lines.
#[must_use]
pub fn describe_plan(steps: &[Step]) -> Vec<String> {
    steps.iter().map(describe).collect()
}

fn ambient_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

#[allow(dead_code)]
fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

#[allow(dead_code)]
fn tool(path_env: &str, name: &str, args: &[String]) -> Result<bool> {
    let Some(bin) = resolve_tool(name, path_env) else {
        anyhow::bail!("profile: {name} not found on PATH");
    };
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .status_retrying()
        .with_context(|| format!("profile: failed to run {name}"))?;
    Ok(status.success())
}

/// Whether `DRY_RUN` is exactly `1`.
#[must_use]
pub fn is_dry_run() -> bool {
    std::env::var("DRY_RUN").is_ok_and(|value| value == "1")
}

fn decide(action: &ProfileAction) -> Result<PlannedAction> {
    match action {
        ProfileAction::Performance => Ok(PlannedAction::Performance),
        ProfileAction::Balanced => Ok(PlannedAction::Balanced),
        ProfileAction::PowerSaver => Ok(PlannedAction::PowerSaver),
        ProfileAction::Unknown(id) => anyhow::bail!("profile: unknown action: {id}"),
    }
}

#[allow(clippy::unnecessary_wraps)]
fn run_step(step: &Step, path_env: &str) -> Result<()> {
    match step {
        Step::PowerprofilesctlSet(profile) => {
            let tuned_profile = match profile.as_str() {
                "performance" => "throughput-performance",
                "power-saver" => "powersave",
                _ => "balanced",
            };
            let script = format!(
                "powerprofilesctl set {profile} 2>/dev/null || tuned-adm profile {tuned_profile}"
            );
            if let Some(setsid) = resolve_tool("setsid", path_env) {
                let _ = Command::new(setsid)
                    .args(["-f", "sh", "-c", &script])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status_retrying();
            } else {
                let _ = Command::new("sh")
                    .args(["-c", &script])
                    .env("PATH", path_env)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
            Ok(())
        }
    }
}

/// Run the selected profile action.
///
/// # Errors
///
/// When the id is malformed or execution fails.
pub fn execute(action_id: &str, path_env: Option<&str>) -> Result<ExecuteReport> {
    let Some(action) = ProfileAction::parse(action_id) else {
        anyhow::bail!("profile: bad id '{action_id}'");
    };
    let planned = decide(&action)?;
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    execute_with(action_id, &planned, Some(&path_env))
}

/// Effect half of [`execute`].
///
/// # Errors
///
/// When `powerprofilesctl` is missing or fails.
pub fn execute_with(
    action_id: &str,
    planned: &PlannedAction,
    path_env: Option<&str>,
) -> Result<ExecuteReport> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let steps = plan(planned);
    let detail = planned.as_str().to_string();
    if is_dry_run() {
        let mut lines = Vec::with_capacity(steps.len());
        for step in &steps {
            let line = format!("would run: {}", describe(step));
            flex_core::diag::note(&line);
            lines.push(line);
        }
        return Ok(ExecuteReport {
            action_id: action_id.to_string(),
            detail,
            dry_run: Some(lines),
        });
    }
    crate::providers::profile::save_active_profile(planned.as_str());
    for step in &steps {
        if let Err(err) = run_step(step, &path_env) {
            flex_core::diag::note(&format!("{err}"));
        }
    }
    Ok(ExecuteReport {
        action_id: action_id.to_string(),
        detail,
        dry_run: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_validate_properly() {
        for (text, expected) in [
            ("performance", ProfileAction::Performance),
            ("profile:performance", ProfileAction::Performance),
            ("balanced", ProfileAction::Balanced),
            ("profile:balanced", ProfileAction::Balanced),
            ("power-saver", ProfileAction::PowerSaver),
            ("profile:power-saver", ProfileAction::PowerSaver),
        ] {
            assert_eq!(ProfileAction::parse(text), Some(expected), "{text:?}");
        }
        assert_eq!(
            ProfileAction::parse("invalid"),
            Some(ProfileAction::Unknown(String::from("invalid"))),
        );
    }

    #[test]
    fn ids_reject_bad_set() {
        for bad in ["", "a/b", "a\nb"] {
            assert_eq!(ProfileAction::parse(bad), None);
        }
    }

    #[test]
    fn describe_matches_powerprofilesctl() {
        assert_eq!(
            describe(&Step::PowerprofilesctlSet("performance".to_string())),
            "powerprofilesctl set performance"
        );
        assert_eq!(
            describe(&Step::PowerprofilesctlSet("balanced".to_string())),
            "powerprofilesctl set balanced"
        );
        assert_eq!(
            describe(&Step::PowerprofilesctlSet("power-saver".to_string())),
            "powerprofilesctl set power-saver"
        );
    }
}
