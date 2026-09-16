//! `power` executor: the power-menu flow.
//!
//! Port of the retired `flex-power.sh` wrapper: after the menu selects a row,
//! the executor validates the id (non-empty, no `/`, no newline) and
//! dispatches:
//!
//! - `lock` → `hyprlock`
//! - `suspend` → `systemctl suspend`
//! - `reboot` → `systemctl reboot`
//! - `poweroff` → `systemctl poweroff`
//! - `logout` → `pkill -SIGTERM Hyprland`
//!
//! The ids are the standalone provider's bash-exact arms
//! ([`crate::providers::power::ROWS`]), not the center surface's
//! `pw`-prefixed ids. There is no hash to resolve (unlike launch/theme/
//! wallpaper), so the whole plan follows from the row id: the in-process
//! resolution step the other ports perform is a no-op here.
//!
//! Danger rows (`reboot`, `poweroff`) are confirmed UI-side by the shared
//! double-Enter flow ([`flex_core::keys`]) before SELECT ever arrives: the
//! provider marks them `confirmable`, and the wrapper's case body never
//! re-prompts (`flex-power.sh:51-57`). The executor therefore never prompts
//! either — a `Chosen` `reboot`/`poweroff` is already a confirmed selection.
//!
//! `DRY_RUN=1` is reimplemented in Rust (the wrapper's blast-radius gate):
//! when `DRY_RUN` is exactly `1`, the executor builds each command and
//! emits `would run: <cmd>` on stdout instead of spawning, returning those
//! lines as the effect. This is the primary safety mechanism for the most
//! blast-radius-critical provider. The wrapper's check is
//! `[[ "$dry_run" == "1" ]]` (`flex-power.sh:27,42`), so only the exact
//! value `1` is dry — `DRY_RUN=0`, empty, and unset all execute (the safe
//! direction, and the reason the executor does not use a bare
//! "set and non-empty" test).
//!
//! `pkill` is quiet (never fails loudly); `hyprlock`/`systemctl` are loud.
//!
//! Snapshot convention (the [`exec::shot`](super::shot) template): [`plan`]
//! builds the [`Step`]s from a decided action, [`describe`] renders one step
//! as a single line — unit tests pin the lines, and the integration tests
//! diff the stub-`PATH` call logs against the same shapes. Power-specific
//! describe lines:
//!
//! - `hyprlock`
//! - `systemctl suspend|reboot|poweroff`
//! - `pkill -SIGTERM Hyprland`
//!
//! Env seams: `DRY_RUN` (the dry-run gate), `PATH` (tool resolution).
//!
//! Deliberate departures from the wrapper (all tested):
//!
//! - No bash `run()` wrapper and no `ACTION:` re-parse: the binary executes
//!   the selected id directly, so the `DRY_RUN` check is a plain Rust env
//!   read and the echo format (`would run: <cmd>`) is byte-identical.
//! - `pkill` is quiet: the wrapper runs it bare under `set -e`, so a
//!   non-zero `pkill` (no `Hyprland` process matched) would abort the
//!   script; the port swallows the failure (the logout intent is
//!   best-effort, and the command sequence is identical).
//! - Exit-code normalisation: the wrapper lets a loud tool's status
//!   propagate verbatim under `set -e`; the port maps every loud failure
//!   through the shared runner, so any failure exits `1` with the single
//!   `flex: error:` prefix.
//! - `delete`/`toggle`/`target` outcomes never reach the executor: power
//!   rows are non-deletable and carry no toggle/target, so the binary bails
//!   on them (the wrapper's case pattern also accepts `ACTION:DELETE`, but
//!   that outcome is unreachable from the power surface — the theme/launch
//!   ports bail on it for the same reason).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::spawn::RetryExec as _;

/// A validated power action id (the wrapper's two-stage check: bad id, then
/// case arm). `Unknown` is well-formed but unsupported — the wrapper's `*)`
/// arm, not its `bad id` exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerAction {
    /// `lock` → `hyprlock`.
    Lock,
    /// `suspend` → `systemctl suspend`.
    Suspend,
    /// `reboot` → `systemctl reboot`.
    Reboot,
    /// `poweroff` → `systemctl poweroff`.
    Off,
    /// `logout` → `pkill -SIGTERM Hyprland`.
    Logout,
    /// Well-formed but unsupported id (the wrapper's `unknown action` arm).
    Unknown(String),
}

impl PowerAction {
    /// Validate + parse a menu action id, mirroring the wrapper's two-stage
    /// check: `None` is the `bad id` exit (empty, `/`-bearing, or
    /// newline-bearing); [`Self::Unknown`] is the `unknown action` arm.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        if id.is_empty() || id.contains('/') || id.contains('\n') {
            return None;
        }
        Some(match id {
            "lock" => Self::Lock,
            "suspend" => Self::Suspend,
            "reboot" => Self::Reboot,
            "poweroff" => Self::Off,
            "logout" => Self::Logout,
            _ => Self::Unknown(id.to_string()),
        })
    }
}

/// A fully-decided power action: [`execute`] performs the parse/decide step
/// and [`execute_with`] runs the [`plan`] steps (the `execute`/`execute_with`
/// split from the pilot template).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    /// `hyprlock`.
    Lock,
    /// `systemctl suspend`.
    Suspend,
    /// `systemctl reboot`.
    Reboot,
    /// `systemctl poweroff`.
    Off,
    /// `pkill -SIGTERM Hyprland`.
    Logout,
}

impl PlannedAction {
    /// Bash arm spelling (also the report detail).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Suspend => "suspend",
            Self::Reboot => "reboot",
            Self::Off => "poweroff",
            Self::Logout => "logout",
        }
    }
}

/// One power step: a tool spawn (pure data, no process started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `hyprlock` (loud).
    Hyprlock,
    /// `systemctl suspend` (loud).
    SystemctlSuspend,
    /// `systemctl reboot` (loud).
    SystemctlReboot,
    /// `systemctl poweroff` (loud).
    SystemctlPoweroff,
    /// `pkill -SIGTERM Hyprland` (quiet).
    PkillHyprland,
}

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// The action id that ran.
    pub action_id: String,
    /// The power arm spelling (report detail).
    pub detail: String,
    /// `Some(lines)` when `DRY_RUN` was on: the `would run: …` lines the
    /// executor emitted instead of spawning (the returned effect); `None`
    /// when the steps really ran.
    pub dry_run: Option<Vec<String>>,
}

/// Build the [`Step`]s for a decided [`PlannedAction`] (no process started).
#[must_use]
pub fn plan(planned: &PlannedAction) -> Vec<Step> {
    match planned {
        PlannedAction::Lock => vec![Step::Hyprlock],
        PlannedAction::Suspend => vec![Step::SystemctlSuspend],
        PlannedAction::Reboot => vec![Step::SystemctlReboot],
        PlannedAction::Off => vec![Step::SystemctlPoweroff],
        PlannedAction::Logout => vec![Step::PkillHyprland],
    }
}

/// Render one [`Step`] as a single snapshot line: the argv joined by spaces.
///
/// This is the template snapshot convention: unit tests pin these lines per
/// id (pure, no spawn), and the integration tests diff the stub-`PATH` call
/// logs against the same shapes.
#[must_use]
pub fn describe(step: &Step) -> String {
    match step {
        Step::Hyprlock => String::from("hyprlock"),
        Step::SystemctlSuspend => String::from("systemctl suspend"),
        Step::SystemctlReboot => String::from("systemctl reboot"),
        Step::SystemctlPoweroff => String::from("systemctl poweroff"),
        Step::PkillHyprland => String::from("pkill -SIGTERM Hyprland"),
    }
}

/// Render a whole [`plan`] as snapshot lines (see [`describe`]).
#[must_use]
pub fn describe_plan(steps: &[Step]) -> Vec<String> {
    steps.iter().map(describe).collect()
}

/// The ambient `PATH`, empty when unset (tool resolution then fails cleanly
/// instead of inheriting a surprising default).
fn ambient_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// Resolve `name` against `path_env` (`:`-separated, shell-style).
///
/// Returns the first entry naming an existing file, so stub-`PATH` tests can
/// shadow the real tools without touching the process env.
fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

/// Run one tool with `args` (loud): stdin nulled, stdout/stderr inherited
/// like the wrapper's bare `hyprlock`/`systemctl` invocations (feedback
/// reaches the popup). Returns whether it exited `0`. Messages carry no
/// `flex:` prefix; the runner reports them.
///
/// # Errors
///
/// When the tool is missing from `path_env` or the spawn itself fails.
fn tool(path_env: &str, name: &str, args: &[String]) -> Result<bool> {
    let Some(bin) = resolve_tool(name, path_env) else {
        anyhow::bail!("power: {name} not found on PATH");
    };
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .status_retrying()
        .with_context(|| format!("power: failed to run {name}"))?;
    Ok(status.success())
}

/// Run one tool with `args` (quiet): stdio nulled, failures swallowed —
/// the wrapper's best-effort `pkill` (see the module docs). Returns whether
/// it exited `0`; a missing tool or failed spawn counts as failure, never
/// an error.
fn tool_quiet(path_env: &str, name: &str, args: &[String]) -> bool {
    let Some(bin) = resolve_tool(name, path_env) else {
        return false;
    };
    Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status_retrying()
        .is_ok_and(|status| status.success())
}

/// Whether `DRY_RUN` is exactly `1` (the wrapper's `[[ "$dry_run" == "1" ]]`,
/// so `0`/empty/unset all execute — the safe direction).
#[must_use]
pub fn is_dry_run() -> bool {
    std::env::var("DRY_RUN").is_ok_and(|value| value == "1")
}

/// `(tool, args)` for a power arm (the wrapper's `case` arms verbatim).
fn power_argv(kind: &PlannedAction) -> (&'static str, Vec<String>) {
    match kind {
        PlannedAction::Lock => ("hyprlock", Vec::new()),
        PlannedAction::Suspend => ("systemctl", vec![String::from("suspend")]),
        PlannedAction::Reboot => ("systemctl", vec![String::from("reboot")]),
        PlannedAction::Off => ("systemctl", vec![String::from("poweroff")]),
        PlannedAction::Logout => (
            "pkill",
            vec![String::from("-SIGTERM"), String::from("Hyprland")],
        ),
    }
}

/// Decide the [`PlannedAction`] from a validated [`PowerAction`] (pure, no
/// env reads): a direct map, since power has no planning/read phase.
///
/// # Errors
///
/// On [`PowerAction::Unknown`] (the wrapper's `unknown action` arm).
/// Messages carry no `flex:` prefix.
fn decide(action: &PowerAction) -> Result<PlannedAction> {
    match action {
        PowerAction::Lock => Ok(PlannedAction::Lock),
        PowerAction::Suspend => Ok(PlannedAction::Suspend),
        PowerAction::Reboot => Ok(PlannedAction::Reboot),
        PowerAction::Off => Ok(PlannedAction::Off),
        PowerAction::Logout => Ok(PlannedAction::Logout),
        PowerAction::Unknown(id) => anyhow::bail!("power: unknown action: {id}"),
    }
}

/// Run one loud tool (the `hyprlock`/`systemctl` arms).
///
/// # Errors
///
/// When the tool is missing or exits non-zero. Messages carry no `flex:`
/// prefix.
fn run_loud(path_env: &str, kind: &PlannedAction, step: &Step) -> Result<()> {
    let (name, args) = power_argv(kind);
    if !tool(path_env, name, &args)? {
        anyhow::bail!("power: {} failed", describe(step));
    }
    Ok(())
}

/// Run one [`Step`] (the [`execute_with`] interpreter body).
///
/// # Errors
///
/// When a loud tool (`hyprlock`, `systemctl`) is missing or exits non-zero.
/// `pkill` is quiet and never errors. Messages carry no `flex:` prefix.
fn run_step(step: &Step, path_env: &str) -> Result<()> {
    match step {
        Step::Hyprlock => run_loud(path_env, &PlannedAction::Lock, step),
        Step::SystemctlSuspend => run_loud(path_env, &PlannedAction::Suspend, step),
        Step::SystemctlReboot => run_loud(path_env, &PlannedAction::Reboot, step),
        Step::SystemctlPoweroff => run_loud(path_env, &PlannedAction::Off, step),
        Step::PkillHyprland => {
            let (name, args) = power_argv(&PlannedAction::Logout);
            tool_quiet(path_env, name, &args);
            Ok(())
        }
    }
}

/// Run the selected power action: validate the id (bad id → error; unknown
/// arm → error), decide, and run the [`Step`]s. `path_env` shadows the
/// ambient `PATH` when `Some` (the stub seam tests use); `None` inherits it.
///
/// # Errors
///
/// When the id is malformed (`bad id`), well-formed but unsupported
/// (`unknown action`), or a loud tool is missing or fails. Messages carry no
/// `flex:` prefix; the runner reports them. Quiet arms (`pkill`) always
/// succeed.
pub fn execute(action_id: &str, path_env: Option<&str>) -> Result<ExecuteReport> {
    let Some(action) = PowerAction::parse(action_id) else {
        anyhow::bail!("power: bad id '{action_id}'");
    };
    let planned = decide(&action)?;
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    execute_with(action_id, &planned, Some(&path_env))
}

/// Effect half of [`execute`], with the decided [`PlannedAction`] given (no
/// env reads except the `DRY_RUN` gate, no parse): the shape integration
/// tests drive with a stub `PATH`.
///
/// When `DRY_RUN` is exactly `1`, each `would run: <cmd>` line is printed
/// and returned in [`ExecuteReport::dry_run`]; nothing is spawned. Otherwise
/// the steps run and `dry_run` is `None`.
///
/// # Errors
///
/// When a loud tool (`hyprlock`, `systemctl`) is missing or exits non-zero
/// (always loud — the quiet `pkill` arm never fails). Messages carry no
/// `flex:` prefix.
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
    for step in &steps {
        run_step(step, &path_env)?;
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
    fn ids_validate_like_the_wrapper_two_stage_check() {
        for (text, expected) in [
            ("lock", PowerAction::Lock),
            ("suspend", PowerAction::Suspend),
            ("reboot", PowerAction::Reboot),
            ("poweroff", PowerAction::Off),
            ("logout", PowerAction::Logout),
        ] {
            assert_eq!(PowerAction::parse(text), Some(expected), "{text:?}");
        }
        // Well-formed but unsupported parses to `Unknown` (the `*)` arm).
        assert_eq!(
            PowerAction::parse("format"),
            Some(PowerAction::Unknown(String::from("format"))),
        );
    }

    #[test]
    fn ids_reject_the_wrapper_bad_id_set() {
        for bad in ["", "a/b", "a\nb", "/lead", "trail/", "mid\ndle"] {
            assert_eq!(PowerAction::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn planned_actions_round_trip_through_as_str() {
        for (text, planned) in [
            ("lock", PlannedAction::Lock),
            ("suspend", PlannedAction::Suspend),
            ("reboot", PlannedAction::Reboot),
            ("poweroff", PlannedAction::Off),
            ("logout", PlannedAction::Logout),
        ] {
            assert_eq!(planned.as_str(), text, "round-trips through as_str");
            assert_eq!(PowerAction::parse(text), Some(planned_as_action(&planned)));
        }
    }

    /// Map a [`PlannedAction`] back to its [`PowerAction`] (test-only helper:
    /// the production direction is [`decide`]).
    fn planned_as_action(planned: &PlannedAction) -> PowerAction {
        match planned {
            PlannedAction::Lock => PowerAction::Lock,
            PlannedAction::Suspend => PowerAction::Suspend,
            PlannedAction::Reboot => PowerAction::Reboot,
            PlannedAction::Off => PowerAction::Off,
            PlannedAction::Logout => PowerAction::Logout,
        }
    }

    #[test]
    fn unknown_ids_do_not_decide() {
        let err = decide(&PowerAction::Unknown(String::from("format")))
            .expect_err("unknown must not decide");
        assert_eq!(format!("{err:#}"), "power: unknown action: format");
    }

    #[test]
    fn plan_snapshot_every_arm() {
        for (planned, line) in [
            (PlannedAction::Lock, "hyprlock"),
            (PlannedAction::Suspend, "systemctl suspend"),
            (PlannedAction::Reboot, "systemctl reboot"),
            (PlannedAction::Off, "systemctl poweroff"),
            (PlannedAction::Logout, "pkill -SIGTERM Hyprland"),
        ] {
            assert_eq!(describe_plan(&plan(&planned)), vec![line.to_string()]);
        }
    }

    #[test]
    fn describe_matches_the_wrapper_commands() {
        assert_eq!(describe(&Step::Hyprlock), "hyprlock");
        assert_eq!(describe(&Step::SystemctlSuspend), "systemctl suspend");
        assert_eq!(describe(&Step::SystemctlReboot), "systemctl reboot");
        assert_eq!(describe(&Step::SystemctlPoweroff), "systemctl poweroff");
        assert_eq!(describe(&Step::PkillHyprland), "pkill -SIGTERM Hyprland");
    }

    /// `DRY_RUN` is dry only for the exact value `1` (the wrapper's check),
    /// so `0`/empty/unset execute. Held under a lock because the process env
    /// is shared by every unit test in this binary.
    #[test]
    fn dry_run_is_exactly_one() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = ENV_LOCK.lock().expect("env lock");
        let saved = std::env::var("DRY_RUN").ok();
        for (value, expected) in [
            (Some("1"), true),
            (Some("0"), false),
            (Some(""), false),
            (Some("yes"), false),
            (None, false),
        ] {
            match value {
                Some(value) => std::env::set_var("DRY_RUN", value),
                None => std::env::remove_var("DRY_RUN"),
            }
            assert_eq!(is_dry_run(), expected, "DRY_RUN={value:?}");
        }
        match saved {
            Some(value) => std::env::set_var("DRY_RUN", value),
            None => std::env::remove_var("DRY_RUN"),
        }
    }
}
