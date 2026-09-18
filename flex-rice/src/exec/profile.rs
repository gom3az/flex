//! `profile` executor: the power-profile selection flow (`powerprofilesctl`
//! with `tuned-adm` as a fallback).
//!
//! Both backends are direct `Command` spawns — no shell is involved.
//! [`run_step`] attempts `powerprofilesctl set <profile>` first; only when
//! that binary is absent or exits non-zero does it try
//! `tuned-adm profile <tuned-profile>`.

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

    /// `tuned-adm` profile name that corresponds to this action.
    ///
    /// Used as a fallback when `powerprofilesctl` is absent.
    #[must_use]
    pub fn tuned_profile(&self) -> &'static str {
        match self {
            Self::Performance => "throughput-performance",
            Self::Balanced => "balanced",
            Self::PowerSaver => "powersave",
        }
    }
}

/// One profile step: a tool spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `powerprofilesctl set <profile>` — preferred backend.
    PowerprofilesctlSet(String),
    /// `tuned-adm profile <profile>` — fallback when `powerprofilesctl` is absent.
    TunedAdmProfile(String),
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
///
/// Returns the preferred step first (`powerprofilesctl`) followed by the
/// fallback (`tuned-adm`). [`run_step`] stops after the first success, so
/// only one tool runs at runtime; dry-run output shows both to be transparent
/// about what *would* run.
#[must_use]
pub fn plan(planned: &PlannedAction) -> Vec<Step> {
    vec![
        Step::PowerprofilesctlSet(planned.as_str().to_string()),
        Step::TunedAdmProfile(planned.tuned_profile().to_string()),
    ]
}

/// Render one [`Step`] as a single snapshot line.
#[must_use]
pub fn describe(step: &Step) -> String {
    match step {
        Step::PowerprofilesctlSet(profile) => format!("powerprofilesctl set {profile}"),
        Step::TunedAdmProfile(profile) => format!("tuned-adm profile {profile}"),
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

/// Attempt one step and report whether it succeeded.
///
/// Both backends are direct `Command` spawns — no shell. The caller iterates
/// steps in order and stops after the first success.
fn run_step(step: &Step, path_env: &str) -> bool {
    match step {
        Step::PowerprofilesctlSet(profile) => {
            let Some(bin) = resolve_tool("powerprofilesctl", path_env) else {
                return false;
            };
            Command::new(&bin)
                .args(["set", profile])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status_retrying()
                .is_ok_and(|s| s.success())
        }
        Step::TunedAdmProfile(profile) => {
            let Some(bin) = resolve_tool("tuned-adm", path_env) else {
                return false;
            };
            Command::new(&bin)
                .args(["profile", profile])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status_retrying()
                .is_ok_and(|s| s.success())
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
/// Tries each step in [`plan`] order and stops after the first success.
/// On a system with `powerprofilesctl`, only that step runs. On a system
/// without it, `tuned-adm` is tried next.
///
/// # Errors
///
/// Only when the id is malformed or the report cannot be built; individual
/// tool spawns that fail are silently skipped (both tools may be absent).
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
        if run_step(step, &path_env) {
            break;
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

    #[test]
    fn describe_matches_tuned_adm() {
        assert_eq!(
            describe(&Step::TunedAdmProfile("throughput-performance".to_string())),
            "tuned-adm profile throughput-performance"
        );
        assert_eq!(
            describe(&Step::TunedAdmProfile("balanced".to_string())),
            "tuned-adm profile balanced"
        );
        assert_eq!(
            describe(&Step::TunedAdmProfile("powersave".to_string())),
            "tuned-adm profile powersave"
        );
    }

    #[test]
    fn plan_has_two_steps_preferred_then_fallback() {
        for (action, ppc_profile, tuned) in [
            (
                PlannedAction::Performance,
                "performance",
                "throughput-performance",
            ),
            (PlannedAction::Balanced, "balanced", "balanced"),
            (PlannedAction::PowerSaver, "power-saver", "powersave"),
        ] {
            let steps = plan(&action);
            assert_eq!(steps.len(), 2, "plan always has preferred + fallback");
            assert_eq!(
                steps[0],
                Step::PowerprofilesctlSet(ppc_profile.to_string()),
                "first step is powerprofilesctl"
            );
            assert_eq!(
                steps[1],
                Step::TunedAdmProfile(tuned.to_string()),
                "second step is tuned-adm fallback"
            );
        }
    }

    #[test]
    fn tuned_profile_mappings_are_correct() {
        assert_eq!(
            PlannedAction::Performance.tuned_profile(),
            "throughput-performance"
        );
        assert_eq!(PlannedAction::Balanced.tuned_profile(), "balanced");
        assert_eq!(PlannedAction::PowerSaver.tuned_profile(), "powersave");
    }
}
