//! `theme` executor: the theme-activate flow.
//!
//! Port of the retired `flex-theme.sh` wrapper: after the menu selects a
//! row, the executor validates the id (non-empty, no `/`, no newline),
//! short-circuits
//! the `noop` empty-scan placeholder (exit `0`, B-026), resolves the row
//! hash back to the theme name (the same
//! [`resolve_name`](crate::providers::theme_::resolve_name) scan), and
//! `exec`s `$THEME_SWITCHER activate <name>` (default
//! `$HOME/.config/scripts/theme-switcher.sh`).
//!
//! Snapshot convention (the [`exec::shot`](super::shot) template): [`plan`]
//! builds the [`Step`]s from resolved inputs, [`describe`] renders one step
//! as a single line — `<switcher> activate <name>` — unit tests pin the
//! lines, and integration tests diff the stub-`PATH` call logs against the
//! same shapes.
//!
//! Two deliberate departures from the wrapper (both tested):
//!
//! - No subprocess: the hash is resolved in-process with the same
//!   [`resolve_name`](crate::providers::theme_::resolve_name) the library
//!   exposes, so the resolution semantics are identical with one fewer
//!   spawn.
//! - Exit-code normalisation: the wrapper `exec`s the switcher so its exit
//!   status propagates verbatim; the port maps every failure through the
//!   shared runner, so any tool failure exits `1` with the single
//!   `flex: error:` prefix. There is no quiet-cancel step (unlike `shot`'s
//!   `slurp`): a failing switcher is always a loud error.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::providers::theme_;

/// A validated theme action id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeAction {
    /// The `noop` empty-scan placeholder: exit `0`, never resolve or
    /// activate (the wrapper exits before resolving, B-026).
    Noop,
    /// A row-hash id to resolve back to a theme name before activating.
    Activate(String),
}

impl ThemeAction {
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
            Some(Self::Activate(id.to_string()))
        }
    }
}

/// One theme step: a tool spawn (pure data, no process started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `<switcher> activate <name>` (the wrapper's `exec` line).
    Activate {
        /// Theme-switcher program.
        switcher: String,
        /// Resolved theme name (passed as one argument, so
        /// space-bearing names like `My Theme` survive — B-021).
        name: String,
    },
}

/// Build the activate [`Step`]s for `name` (no process started).
///
/// `switcher` is the `THEME_SWITCHER` program, `name` the already-resolved
/// theme name.
#[must_use]
pub fn plan(switcher: &str, name: &str) -> Vec<Step> {
    vec![Step::Activate {
        switcher: switcher.to_string(),
        name: name.to_string(),
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
        Step::Activate { switcher, name } => format!("{switcher} activate {name}"),
    }
}

/// Render a whole [`plan`] as snapshot lines (see [`describe`]).
#[must_use]
pub fn describe_plan(steps: &[Step]) -> Vec<String> {
    steps.iter().map(describe).collect()
}

/// Theme-switcher program: `THEME_SWITCHER`, else
/// `$HOME/.config/scripts/theme-switcher.sh` (empty values fall back, like
/// the wrapper's `${VAR:-default}`).
///
/// # Errors
///
/// When `HOME` is unset and no override is set.
pub fn theme_switcher() -> Result<PathBuf> {
    if let Ok(switcher) = std::env::var("THEME_SWITCHER") {
        if !switcher.is_empty() {
            return Ok(PathBuf::from(switcher));
        }
    }
    let home = std::env::var("HOME").context("theme: HOME is not set")?;
    Ok(Path::new(&home).join(".config/scripts/theme-switcher.sh"))
}

/// The ambient `PATH`, empty when unset (tool resolution then fails cleanly
/// instead of inheriting a surprising default).
fn ambient_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// Resolve `name` against `path_env` (`:`-separated, shell-style).
///
/// Returns the first entry naming an existing file, so stub-`PATH` tests can
/// shadow the real tools without touching the process env. An absolute
/// `name` (the usual `THEME_SWITCHER` shape) resolves to itself, exactly
/// like the wrapper's `exec "$theme_switcher"`.
fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

/// Run one tool with `args`.
///
/// Stdout/stderr are inherited, like the wrapper's `exec` (switcher feedback
/// reaches the popup); stdin is nulled. A non-zero exit is `Ok` with
/// `ok: false`, not an error — callers map it. Messages carry no `flex:`
/// prefix; the runner reports them.
///
/// # Errors
///
/// When the tool is missing from `path_env` or the spawn itself fails.
fn tool(path_env: &str, name: &str, args: &[String]) -> Result<bool> {
    let Some(bin) = resolve_tool(name, path_env) else {
        anyhow::bail!("theme: {name} not found on PATH");
    };
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("theme: failed to run {name}"))?;
    Ok(status.success())
}

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// Switcher invoked (`None` when `noop` short-circuited first — the
    /// wrapper exits before reading `THEME_SWITCHER`, so this does too).
    pub switcher: Option<PathBuf>,
    /// Activated theme name (`None` for `noop`).
    pub name: Option<String>,
}

/// Activate the selected theme: validate the id, short-circuit `noop`,
/// resolve the hash to a name, and run `<switcher> activate <name>`.
/// `path_env` shadows the ambient `PATH` when `Some` (the stub seam tests
/// use); `None` inherits it.
///
/// # Errors
///
/// When the id is malformed (`bad id`, like the wrapper), the hash resolves
/// to nothing (`unknown id`), the resolved name is malformed (`bad name`),
/// the switcher is missing, or the switcher fails. Messages carry no `flex:`
/// prefix; the runner reports them.
pub fn execute(action_id: &str, path_env: Option<&str>) -> Result<ExecuteReport> {
    let Some(action) = ThemeAction::parse(action_id) else {
        anyhow::bail!("theme: bad id '{action_id}'");
    };
    let ThemeAction::Activate(id) = action else {
        return Ok(ExecuteReport {
            switcher: None,
            name: None,
        });
    };
    let Some(name) = theme_::resolve_name(&id) else {
        anyhow::bail!("theme: unknown id '{id}'");
    };
    if name.is_empty() || name.contains('/') || name.contains('\n') {
        anyhow::bail!("theme: bad name: {name}");
    }
    let switcher = theme_switcher()?;
    execute_with(&name, &switcher, path_env)
}

/// Activate half of [`execute`], with the resolved inputs given (no env
/// reads): the shape integration tests drive with a stub `PATH`.
///
/// # Errors
///
/// When the switcher is missing or exits non-zero (always loud — there is
/// no quiet-cancel step). Messages carry no `flex:` prefix.
pub fn execute_with(name: &str, switcher: &Path, path_env: Option<&str>) -> Result<ExecuteReport> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let switcher_str = switcher.to_string_lossy().into_owned();
    for step in plan(&switcher_str, name) {
        match step {
            Step::Activate { switcher, name } => {
                let args = vec![String::from("activate"), name.clone()];
                if !tool(&path_env, &switcher, &args)? {
                    anyhow::bail!("theme: activate {name} failed");
                }
            }
        }
    }
    Ok(ExecuteReport {
        switcher: Some(switcher.to_path_buf()),
        name: Some(name.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_validate_like_the_wrapper_check() {
        assert_eq!(ThemeAction::parse("noop"), Some(ThemeAction::Noop));
        assert_eq!(
            ThemeAction::parse("0123456789abcdef"),
            Some(ThemeAction::Activate(String::from("0123456789abcdef"))),
        );
    }

    #[test]
    fn ids_reject_the_wrapper_bad_id_set() {
        for bad in ["", "a/b", "a\nb", "/lead", "trail/", "mid\ndle"] {
            assert_eq!(ThemeAction::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn plan_snapshot_is_the_switcher_activate_line() {
        assert_eq!(
            describe_plan(&plan("/sw/theme-switcher.sh", "Tokyo Night")),
            vec!["/sw/theme-switcher.sh activate Tokyo Night"],
        );
    }

    #[test]
    fn plan_keeps_a_space_bearing_name_whole() {
        let steps = plan("/sw/theme-switcher.sh", "My Theme");
        assert_eq!(steps.len(), 1);
        assert_eq!(
            describe(&steps[0]),
            "/sw/theme-switcher.sh activate My Theme"
        );
    }

    #[test]
    fn theme_switcher_prefers_the_override() {
        let saved = std::env::var("THEME_SWITCHER").ok();
        std::env::set_var("THEME_SWITCHER", "/tmp/stub-switcher.sh");
        let switcher = theme_switcher().expect("override switcher");
        assert_eq!(switcher, Path::new("/tmp/stub-switcher.sh"));
        match saved {
            Some(value) => std::env::set_var("THEME_SWITCHER", value),
            None => std::env::remove_var("THEME_SWITCHER"),
        }
    }

    #[test]
    fn theme_switcher_empty_falls_back_to_the_home_default() {
        let saved_switcher = std::env::var("THEME_SWITCHER").ok();
        let saved_home = std::env::var("HOME").ok();
        std::env::set_var("THEME_SWITCHER", "");
        std::env::set_var("HOME", "/tmp/fake-home");
        let switcher = theme_switcher().expect("default switcher");
        assert_eq!(
            switcher,
            Path::new("/tmp/fake-home/.config/scripts/theme-switcher.sh")
        );
        match saved_switcher {
            Some(value) => std::env::set_var("THEME_SWITCHER", value),
            None => std::env::remove_var("THEME_SWITCHER"),
        }
        match saved_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}
