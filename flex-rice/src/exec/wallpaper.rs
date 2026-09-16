//! `wallpaper` executor: the wallpaper-set flow.
//!
//! Port of `wrappers/flex-wallpaper.sh` (which stays live until cutover):
//! after the menu selects a row, the wrapper reads the
//! `ACTION: wallpaper <id> …` line, validates the id (16 lowercase hex
//! chars, the FNV-1a path hash), resolves it back to the image path
//! (`flex wallpaper --resolve`, i.e. the same
//! [`resolve`](crate::providers::wallpaper::resolve) scan), refuses paths
//! that are not files, and `exec`s
//! `$SET_WALLPAPER <path>` (default
//! `$HOME/.config/scripts/set-wallpaper.sh`).
//!
//! Unlike `theme`, there is no `noop` placeholder: an empty store exits 130
//! in the shared runner before any menu is built, and the wrapper has no
//! `noop` arm — every parsed id resolves and sets.
//!
//! Snapshot convention (the [`exec::shot`](super::shot) template): [`plan`]
//! builds the [`Step`]s from resolved inputs, [`describe`] renders one step
//! as a single line — `<setter> <path>` — unit tests pin the lines, and
//! integration tests diff the stub-`PATH` call logs against the same shapes.
//!
//! Two deliberate departures from the wrapper (both tested):
//!
//! - No `flex wallpaper --resolve` subprocess: the hash is resolved
//!   in-process with the same [`resolve`](crate::providers::wallpaper::resolve)
//!   the hidden `--resolve` lookup uses, so the resolution semantics are
//!   identical with one fewer spawn.
//! - Exit-code normalisation: the wrapper `exec`s the setter so its exit
//!   status propagates verbatim; the port maps every failure through the
//!   shared runner, so any tool failure exits `1` with the single
//!   `flex: error:` prefix. There is no quiet-cancel step (unlike `shot`'s
//!   `slurp`): a failing setter is always a loud error.
//!
//! Kitty-encoder note: the plan's "verbatim move" is already done — the
//! encoder lives in [`flex_core::preview`] (`base64`, `place_escape`, the
//! PNG-convert cache) and the picker consumes row `preview_image`s through
//! the existing preview pane. Nothing moves in this port and no row, id, or
//! filter depends on it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::providers::wallpaper;

/// A validated wallpaper action id: the 16-char lowercase-hex row hash.
///
/// The wrapper's check is `[[ "$id" =~ ^[0-9a-f]{16}$ ]]`, so uppercase hex,
/// short/long strings, and anything bearing `/` or a newline all reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperAction {
    /// The validated hash, handed to [`wallpaper::resolve`].
    pub id: String,
}

impl WallpaperAction {
    /// Validate a menu action id, mirroring the wrapper's regex check.
    /// `None` is the wrapper's `bad id` exit.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        if id.len() == 16
            && id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            Some(Self { id: id.to_string() })
        } else {
            None
        }
    }
}

/// One wallpaper step: a tool spawn (pure data, no process started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `<setter> <path>` (the wrapper's `exec` line).
    Set {
        /// Wallpaper-setter program.
        setter: String,
        /// Resolved image path (passed as one argument, so space-bearing
        /// paths like `pick me.jpg` survive).
        path: String,
    },
}

/// Build the set [`Step`]s for `path` (no process started).
///
/// `setter` is the `SET_WALLPAPER` program, `path` the already-resolved
/// image path.
#[must_use]
pub fn plan(setter: &str, path: &str) -> Vec<Step> {
    vec![Step::Set {
        setter: setter.to_string(),
        path: path.to_string(),
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
        Step::Set { setter, path } => format!("{setter} {path}"),
    }
}

/// Render a whole [`plan`] as snapshot lines (see [`describe`]).
#[must_use]
pub fn describe_plan(steps: &[Step]) -> Vec<String> {
    steps.iter().map(describe).collect()
}

/// Wallpaper-setter program: `SET_WALLPAPER`, else
/// `$HOME/.config/scripts/set-wallpaper.sh` (empty values fall back, like
/// the wrapper's `${VAR:-default}`).
///
/// # Errors
///
/// When `HOME` is unset and no override is set.
pub fn set_wallpaper() -> Result<PathBuf> {
    if let Ok(setter) = std::env::var("SET_WALLPAPER") {
        if !setter.is_empty() {
            return Ok(PathBuf::from(setter));
        }
    }
    let home = std::env::var("HOME").context("wallpaper: HOME is not set")?;
    Ok(Path::new(&home).join(".config/scripts/set-wallpaper.sh"))
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
/// `name` (the usual `SET_WALLPAPER` shape) resolves to itself, exactly
/// like the wrapper's `exec "$setter"`.
fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

/// Run one tool with `args`.
///
/// Stdout/stderr are inherited, like the wrapper's `exec` (setter feedback
/// reaches the popup); stdin is nulled. A non-zero exit is `Ok` with
/// `ok: false`, not an error — callers map it. Messages carry no `flex:`
/// prefix; the runner reports them.
///
/// # Errors
///
/// When the tool is missing from `path_env` or the spawn itself fails.
fn tool(path_env: &str, name: &str, args: &[String]) -> Result<bool> {
    let Some(bin) = resolve_tool(name, path_env) else {
        anyhow::bail!("wallpaper: {name} not found on PATH");
    };
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("wallpaper: failed to run {name}"))?;
    Ok(status.success())
}

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// Setter invoked.
    pub setter: PathBuf,
    /// Image path handed to the setter.
    pub path: PathBuf,
}

/// Set the selected wallpaper: validate the id, resolve the hash to a path,
/// refuse non-files, and run `<setter> <path>`. `path_env` shadows the
/// ambient `PATH` when `Some` (the stub seam tests use); `None` inherits it.
///
/// # Errors
///
/// When the id is malformed (`bad id`, like the wrapper), the hash resolves
/// to nothing (`unknown id`), the resolved path is not a file (`not a
/// file`), the setter is missing, or the setter fails. Messages carry no
/// `flex:` prefix; the runner reports them.
pub fn execute(action_id: &str, path_env: Option<&str>) -> Result<ExecuteReport> {
    let Some(action) = WallpaperAction::parse(action_id) else {
        anyhow::bail!("wallpaper: bad id '{action_id}'");
    };
    let Some(path) = wallpaper::resolve(&action.id) else {
        anyhow::bail!("wallpaper: unknown id '{}'", action.id);
    };
    if !path.is_file() {
        anyhow::bail!("wallpaper: not a file: {}", path.display());
    }
    let setter = set_wallpaper()?;
    execute_with(&path, &setter, path_env)
}

/// Set half of [`execute`], with the resolved inputs given (no env reads):
/// the shape integration tests drive with a stub `PATH`.
///
/// # Errors
///
/// When the setter is missing or exits non-zero (always loud — there is
/// no quiet-cancel step). Messages carry no `flex:` prefix.
pub fn execute_with(path: &Path, setter: &Path, path_env: Option<&str>) -> Result<ExecuteReport> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let setter_str = setter.to_string_lossy().into_owned();
    let path_str = path.to_string_lossy().into_owned();
    for step in plan(&setter_str, &path_str) {
        match step {
            Step::Set { setter, path } => {
                let args = vec![path.clone()];
                if !tool(&path_env, &setter, &args)? {
                    anyhow::bail!("wallpaper: set {path} failed");
                }
            }
        }
    }
    Ok(ExecuteReport {
        setter: setter.to_path_buf(),
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_accept_exactly_sixteen_lowercase_hex_chars() {
        let parsed = WallpaperAction::parse("0123456789abcdef").expect("valid id parses");
        assert_eq!(parsed.id, "0123456789abcdef");
    }

    #[test]
    fn ids_reject_the_wrapper_bad_id_set() {
        for bad in [
            "",
            "nope",
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdef0",
            "0123456789abcdef ",
            " 0123456789abcdef",
            "../../etc/passwd",
            "a/b",
            "a\nb",
            "0123456789abcde\n",
            "noop",
        ] {
            assert_eq!(WallpaperAction::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn plan_snapshot_is_the_setter_path_line() {
        assert_eq!(
            describe_plan(&plan("/sw/set-wallpaper.sh", "/walls/sunset.jpg")),
            vec!["/sw/set-wallpaper.sh /walls/sunset.jpg"],
        );
    }

    #[test]
    fn plan_keeps_a_space_bearing_path_whole() {
        let steps = plan("/sw/set-wallpaper.sh", "/walls/pick me.jpg");
        assert_eq!(steps.len(), 1);
        assert_eq!(
            describe(&steps[0]),
            "/sw/set-wallpaper.sh /walls/pick me.jpg"
        );
    }
}
