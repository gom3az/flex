//! `launch` executor: the application-launch flow.
//!
//! Port of `wrappers/flex-launch.sh` (which stays live until cutover): after
//! the menu selects a row, the wrapper reads the `ACTION: launch <id> …`
//! line, validates the id (non-empty, no `/`, no newline), short-circuits
//! the `noop` empty-scan placeholder (exit `0`, B-026), resolves the row
//! hash back to the desktop-id (`flex launch --resolve`, i.e. the same
//! [`resolve_id`](crate::providers::launch::resolve_id) scan), re-resolves
//! the desktop-id to its raw `(Exec, Terminal)` pair (the same
//! [`find_exec`](crate::providers::launch::find_exec) parse the provider
//! scan uses: first `Exec` wins, last `Terminal` wins), strips `.desktop`
//! field codes (`%U`/`%F`/…, like the wrapper's `sed`), and detaches with
//! `setsid -f` — `kitty -e` prefixed for `Terminal=true` apps.
//!
//! Snapshot convention (the [`exec::shot`](super::shot) template): [`plan`]
//! builds the [`Step`]s from resolved inputs, [`describe`] renders one step
//! as a single line — `setsid -f <program> [args…]` for plain apps,
//! `setsid -f kitty -e <program> [args…]` for `Terminal=true` apps — unit
//! tests pin the lines, and integration tests diff the stub-`PATH` call logs
//! against the same shapes.
//!
//! Three deliberate departures from the wrapper (all tested):
//!
//! - No `flex launch --resolve` subprocess: the hash is resolved in-process
//!   with the same [`resolve_id`](crate::providers::launch::resolve_id)
//!   the hidden `--resolve` lookup uses, so the resolution semantics are
//!   identical with one fewer spawn.
//! - No shell word-splitting: the wrapper `eval`s the stripped `Exec` line
//!   (so shell quotes in `Exec` group arguments); the port splits the line
//!   on whitespace and spawns directly (the pilot's generated-script removal
//!   made the same trade). Plain `prog --flag` lines — the reference-data
//!   case — are byte-identical; quoted `Exec` lines would group differently.
//! - Exit-code normalisation: the wrapper `eval`s the `setsid` line so its
//!   exit status propagates verbatim; the port maps every failure through
//!   the shared runner, so any tool failure exits `1` with the single
//!   `flex: error:` prefix. There is no quiet-cancel step (unlike `shot`'s
//!   `slurp`): a failing launch is always a loud error.
//! - The `Terminal=true` branch keeps the hardcoded `kitty` spawn verbatim
//!   (`setsid -f kitty -e …`, exactly like the wrapper's
//!   `flex-launch.sh:58`). Routing terminal apps through the shared
//!   `terminal` helper (following `$TERMINAL`) is a deliberately deferred
//!   behaviour change, not smuggled into this port.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::providers::launch;

/// A validated launch action id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAction {
    /// The `noop` empty-scan placeholder: exit `0`, never resolve or
    /// launch (the wrapper exits before resolving, B-026).
    Noop,
    /// A row-hash id to resolve back to a desktop-id before launching.
    Launch(String),
}

impl LaunchAction {
    /// Validate a menu action id, mirroring the wrapper's id check
    /// (non-empty, no `/`, no newline). `None` is the wrapper's
    /// `bad id` exit; `noop` is the empty-scan placeholder.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        if id.is_empty() || id.contains('/') || id.contains('\n') {
            return None;
        }
        if id == "noop" {
            Some(Self::Noop)
        } else {
            Some(Self::Launch(id.to_string()))
        }
    }
}

/// One launch step: a detached spawn (pure data, no process started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `setsid -f [kitty -e ]<program> [args…]` (the wrapper's `eval` line,
    /// with the `kitty -e` prefix exactly when `terminal` is set).
    Launch {
        /// Whether the desktop entry sets `Terminal=true` (the hardcoded
        /// `kitty -e` prefix, kept verbatim per the deferred-change note).
        terminal: bool,
        /// Launched program (first whitespace-separated token of the
        /// field-code-stripped `Exec` line).
        program: String,
        /// Remaining tokens of the stripped `Exec` line.
        args: Vec<String>,
    },
}

/// Build the launch [`Step`]s for `program` + `args` (no process started).
///
/// `terminal` selects the hardcoded `kitty -e` prefix; `program`/`args` are
/// the whitespace-split tokens of the already-stripped `Exec` line.
#[must_use]
pub fn plan(program: &str, args: &[String], terminal: bool) -> Vec<Step> {
    vec![Step::Launch {
        terminal,
        program: program.to_string(),
        args: args.to_vec(),
    }]
}

/// Render one [`Step`] as a single snapshot line: `argv` joined by spaces.
///
/// This is the template snapshot convention: unit tests pin these lines per
/// id (pure, no spawn), and the integration tests diff the stub-`PATH` call
/// logs against the same shapes.
#[must_use]
pub fn describe(step: &Step) -> String {
    match step {
        Step::Launch {
            terminal,
            program,
            args,
        } => {
            let mut parts = vec![String::from("setsid"), String::from("-f")];
            if *terminal {
                parts.push(String::from("kitty"));
                parts.push(String::from("-e"));
            }
            parts.push(program.clone());
            parts.extend(args.iter().cloned());
            parts.join(" ")
        }
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
/// shadow the real tools without touching the process env. `setsid` is
/// always resolved this way; `kitty` (terminal branch) and the launched
/// program stay arguments of the `setsid` spawn, exactly like the wrapper's
/// `eval 'setsid -f [kitty -e ]…'` line.
fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

/// Run `setsid -f …` detached: stdio nulled, like the wrapper's
/// `</dev/null >/dev/null 2>&1` redirections. A non-zero exit is `Ok` with
/// `ok: false`, not an error — callers map it. Messages carry no `flex:`
/// prefix; the runner reports them.
///
/// # Errors
///
/// When `setsid` is missing from `path_env` or the spawn itself fails.
fn tool(path_env: &str, args: &[String]) -> Result<bool> {
    let Some(bin) = resolve_tool("setsid", path_env) else {
        anyhow::bail!("launch: setsid not found on PATH");
    };
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| String::from("launch: failed to run setsid"))?;
    Ok(status.success())
}

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// Resolved desktop-id (`None` when `noop` short-circuited first — the
    /// wrapper exits before resolving, so this does too).
    pub desktop_id: Option<String>,
    /// Launched program (`None` for `noop`).
    pub program: Option<String>,
    /// Whether the `kitty -e` terminal prefix was used.
    pub terminal: bool,
}

/// Launch the selected application: validate the id, short-circuit `noop`,
/// resolve the hash to a desktop-id, re-resolve the desktop-id to its
/// `(Exec, Terminal)` pair, strip field codes, and detach via `setsid`.
/// `path_env` shadows the ambient `PATH` when `Some` (the stub seam tests
/// use); `None` inherits it.
///
/// # Errors
///
/// When the id is malformed (`bad id`, like the wrapper), the hash resolves
/// to nothing (`unknown id`), the resolved desktop-id is malformed (`bad
/// desktop id`), the desktop file is gone (`not found`), its `Exec` is
/// empty (`empty Exec`), `setsid` is missing, or the launch fails.
/// Messages carry no `flex:` prefix; the runner reports them.
pub fn execute(action_id: &str, path_env: Option<&str>) -> Result<ExecuteReport> {
    let Some(action) = LaunchAction::parse(action_id) else {
        anyhow::bail!("launch: bad id '{action_id}'");
    };
    let LaunchAction::Launch(id) = action else {
        return Ok(ExecuteReport {
            desktop_id: None,
            program: None,
            terminal: false,
        });
    };
    let Some(desk_id) = launch::resolve_id(&id) else {
        anyhow::bail!("launch: unknown id '{id}'");
    };
    if desk_id.is_empty() || desk_id.contains('/') || desk_id.contains('\n') {
        anyhow::bail!("launch: bad desktop id: {desk_id}");
    }
    let Some((exec_raw, terminal)) = launch::find_exec(&desk_id) else {
        if desktop_file_present(&desk_id) {
            anyhow::bail!("launch: empty Exec in {desk_id}");
        }
        anyhow::bail!("launch: {desk_id} not found");
    };
    let stripped = launch::strip_field_codes(&exec_raw);
    if stripped.is_empty() {
        anyhow::bail!("launch: empty Exec in {desk_id}");
    }
    let mut tokens = stripped.split_whitespace().map(str::to_string);
    let Some(program) = tokens.next() else {
        anyhow::bail!("launch: empty Exec in {desk_id}");
    };
    let args: Vec<String> = tokens.collect();
    execute_with(&desk_id, &program, &args, terminal, path_env)
}

/// Whether any application directory currently holds `desk_id` (tells the
/// wrapper's `not found` apart from its `empty Exec` when the provider
/// parse yields nothing: a present-but-unparseable file is an empty `Exec`,
/// a missing one is `not found`).
fn desktop_file_present(desk_id: &str) -> bool {
    launch::app_dirs()
        .iter()
        .any(|dir| dir.join(desk_id).is_file())
}

/// Launch half of [`execute`], with the resolved inputs given (no scan):
/// the shape integration tests drive with a stub `PATH`.
///
/// # Errors
///
/// When `setsid` is missing or exits non-zero (always loud — there is
/// no quiet-cancel step). Messages carry no `flex:` prefix.
pub fn execute_with(
    desk_id: &str,
    program: &str,
    args: &[String],
    terminal: bool,
    path_env: Option<&str>,
) -> Result<ExecuteReport> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    for step in plan(program, args, terminal) {
        let Step::Launch {
            terminal,
            program,
            args,
        } = step;
        let mut cmd = vec![String::from("-f")];
        if terminal {
            cmd.push(String::from("kitty"));
            cmd.push(String::from("-e"));
        }
        cmd.push(program.clone());
        cmd.extend(args.iter().cloned());
        if !tool(&path_env, &cmd)? {
            anyhow::bail!("launch: failed to launch {desk_id}");
        }
    }
    Ok(ExecuteReport {
        desktop_id: Some(desk_id.to_string()),
        program: Some(program.to_string()),
        terminal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_validate_like_the_wrapper_check() {
        assert_eq!(LaunchAction::parse("noop"), Some(LaunchAction::Noop));
        assert_eq!(
            LaunchAction::parse("0123456789abcdef"),
            Some(LaunchAction::Launch(String::from("0123456789abcdef"))),
        );
    }

    #[test]
    fn ids_reject_the_wrapper_bad_id_set() {
        for bad in ["", "a/b", "a\nb", "/lead", "trail/", "mid\ndle"] {
            assert_eq!(LaunchAction::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn plan_snapshot_is_the_setsid_detach_line() {
        assert_eq!(
            describe_plan(&plan("myapp", &[String::from("--open")], false)),
            vec!["setsid -f myapp --open"],
        );
    }

    #[test]
    fn terminal_plan_keeps_the_hardcoded_kitty_prefix_verbatim() {
        // Deferred behaviour change (plan): terminal apps keep
        // `setsid -f kitty -e …` instead of following `$TERMINAL`.
        assert_eq!(
            describe_plan(&plan("htop", &[], true)),
            vec!["setsid -f kitty -e htop"],
        );
        assert_eq!(
            describe_plan(&plan(
                "myapp",
                &[String::from("--open"), String::from("file")],
                true
            )),
            vec!["setsid -f kitty -e myapp --open file"],
        );
    }

    #[test]
    fn plan_keeps_stripped_exec_tokens_whole() {
        let args = vec![String::from("--open")];
        let steps = plan("myapp", &args, false);
        assert_eq!(steps.len(), 1);
        assert_eq!(describe(&steps[0]), "setsid -f myapp --open");
    }
}
