//! `wallpaper` executor: the wallpaper-set flow.
//!
//! Port of the retired `flex-wallpaper.sh` wrapper: after the menu selects a
//! row, the executor validates the id (16 lowercase hex chars, the FNV-1a
//! path hash), resolves it back to the image path (the same
//! [`resolve`](crate::providers::wallpaper::resolve) scan), refuses paths
//! that are not files, and sets the wallpaper with the ported native setter
//! ([`set_wallpaper_native`]) unless `SET_WALLPAPER` names an explicit
//! override program ([`set_wallpaper_override`]).
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
//! Three deliberate departures from the wrapper (all tested):
//!
//! - No subprocess for the default: the wrapper `exec`s the
//!   `set-wallpaper.sh` script; the port runs its body in-process
//!   ([`set_wallpaper_native`]) and keeps `SET_WALLPAPER` as an explicit
//!   override seam only.
//! - No subprocess for resolution: the hash is resolved in-process with the
//!   same [`resolve`](crate::providers::wallpaper::resolve) the library
//!   exposes, so the resolution semantics are identical with one fewer spawn.
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

use std::ffi::OsStr;
use std::io::Write as _;
use std::os::unix::fs::FileTypeExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result};

use crate::providers::wallpaper;
use crate::spawn::RetryExec as _;

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

/// The explicit `SET_WALLPAPER` override seam: `Some` when the variable is
/// set and non-empty, `None` otherwise (the ported native setter then runs).
#[must_use]
pub fn set_wallpaper_override() -> Option<PathBuf> {
    match std::env::var("SET_WALLPAPER") {
        Ok(setter) if !setter.is_empty() => Some(PathBuf::from(setter)),
        _ => None,
    }
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
        .status_retrying()
        .with_context(|| format!("wallpaper: failed to run {name}"))?;
    Ok(status.success())
}

/// Run one optional tool with `args` (and `stdin` piped when set).
///
/// Returns `None` when `name` is not resolvable from `path_env` (or cannot
/// be spawned); otherwise `Some(status.success())`. Stdout/stderr are always
/// nulled, so a `socat` reply never reaches the popup and a missing helper
/// is quiet — the native setter ignores both `None` and `false`, exactly
/// like the wrapper's `… 2>/dev/null || true`.
fn run_optional(path_env: &str, name: &str, args: &[String], stdin: Option<&[u8]>) -> Option<bool> {
    let bin = resolve_tool(name, path_env)?;
    let mut cmd = Command::new(&bin);
    cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd.spawn_retrying().ok()?;
    if let Some(input) = stdin {
        if let Some(mut handle) = child.stdin.take() {
            let _ = handle.write_all(input);
        }
    }
    Some(child.wait().is_ok_and(|status| status.success()))
}

/// First socket named `.hyprpaper.sock` under `run_dir`, depth-first in
/// sorted directory-entry order (deterministic, unlike the wrapper's
/// `find … | head -1`).
///
/// Only real sockets count: a regular file with the same name is skipped
/// (matching `find -type s`), as is a failed directory read.
fn find_socket(run_dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(run_dir)
        .ok()?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if let Some(found) = find_socket(&entry.path()) {
                return Some(found);
            }
        } else if file_type.is_socket() && entry.file_name() == OsStr::new(".hyprpaper.sock") {
            return Some(entry.path());
        }
    }
    None
}

/// Current numeric uid: `MetadataExt::uid` on `/proc/self`, falling back to
/// `id -u` when the metadata read fails.
///
/// # Errors
///
/// When `/proc/self` cannot be read and `id -u` is missing, fails, or prints
/// no parseable integer. Messages carry no `flex:` prefix.
fn current_uid(path_env: &str) -> Result<u32> {
    if let Ok(metadata) = std::fs::metadata("/proc/self") {
        return Ok(std::os::unix::fs::MetadataExt::uid(&metadata));
    }
    let Some(id) = resolve_tool("id", path_env) else {
        anyhow::bail!("wallpaper: id not found on PATH");
    };
    let output = Command::new(id)
        .arg("-u")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output_retrying()
        .context("wallpaper: failed to run id")?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .parse()
        .context("wallpaper: cannot parse id -u output")
}

/// Spawn `hyprpaper` detached with stdio nulled: `setsid -f hyprpaper` when
/// `setsid` resolves from `path_env`, else a direct `hyprpaper` spawn.
///
/// Best-effort, like the wrapper's `nohup … &`/`disown`: a missing `setsid`
/// or `hyprpaper`, or a failed spawn, is ignored.
fn spawn_hyprpaper(path_env: &str) {
    if let Some(setsid) = resolve_tool("setsid", path_env) {
        let _ = Command::new(setsid)
            .arg("-f")
            .arg("hyprpaper")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn_retrying();
        return;
    }
    if let Some(hyprpaper) = resolve_tool("hyprpaper", path_env) {
        let _ = Command::new(hyprpaper)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn_retrying();
    }
}

/// Set `path` with the ported `set-wallpaper.sh` flow, reading `HOME` and
/// deriving the Hyprland runtime dir from `XDG_RUNTIME_DIR` (else
/// `/run/user/<uid>/hypr`).
///
/// # Errors
///
/// When `HOME` is unset (`wallpaper: HOME is not set`), the uid cannot be
/// determined, or [`set_wallpaper_native_in`] fails. Messages carry no
/// `flex:` prefix; the runner reports them.
pub fn set_wallpaper_native(path: &Path, path_env: &str) -> Result<()> {
    let home = std::env::var("HOME").context("wallpaper: HOME is not set")?;
    let run_dir = match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => Path::new(&dir).join("hypr"),
        _ => PathBuf::from(format!("/run/user/{}/hypr", current_uid(path_env)?)),
    };
    set_wallpaper_native_in(path, Path::new(&home), &run_dir, path_env)
}

/// Native setter body with the resolved inputs given (the test seam).
///
/// Preserves the wrapper's order: best-effort `swaync-client` inhibitor,
/// hyprpaper socket (test → `preload` → 200 ms → `wallpaper`), restart when
/// `pgrep -x hyprpaper` fails, then the persisted `hyprpaper.conf` and the
/// optional ml4w cache file, and finally the inhibitor release. The conf
/// write does not create parent directories (a missing parent errors, like
/// `cat >`).
///
/// # Errors
///
/// When the conf or the existing cache file cannot be written. Messages
/// carry no `flex:` prefix.
pub fn set_wallpaper_native_in(
    path: &Path,
    home: &Path,
    run_dir: &Path,
    path_env: &str,
) -> Result<()> {
    let _ = run_optional(
        path_env,
        "swaync-client",
        &[String::from("-Ia"), String::from("wallpaper-setter")],
        None,
    );

    let hyprpaper_running = |path_env: &str| {
        run_optional(
            path_env,
            "pgrep",
            &[String::from("-x"), String::from("hyprpaper")],
            None,
        )
        .unwrap_or(false)
    };

    if hyprpaper_running(path_env) {
        if let Some(socket) = find_socket(run_dir) {
            let target = vec![
                String::from("-"),
                format!("UNIX-CONNECT:{}", socket.display()),
            ];
            let connected = run_optional(path_env, "socat", &target, Some(b"\n")).unwrap_or(false);
            if connected {
                let preload = format!("preload {}\n", path.display());
                let _ = run_optional(path_env, "socat", &target, Some(preload.as_bytes()));
                std::thread::sleep(Duration::from_millis(200));
                let wallpaper = format!("wallpaper ,{}\n", path.display());
                let _ = run_optional(path_env, "socat", &target, Some(wallpaper.as_bytes()));
            }
        }
    }

    if !hyprpaper_running(path_env) {
        let _ = run_optional(path_env, "killall", &[String::from("hyprpaper")], None);
        std::thread::sleep(Duration::from_millis(300));
        spawn_hyprpaper(path_env);
        std::thread::sleep(Duration::from_secs(1));
    }

    let conf = home.join(".config/hypr/hyprpaper.conf");
    let conf_body = format!(
        "preload = {}\nwallpaper = , {}\n",
        path.display(),
        path.display()
    );
    std::fs::write(&conf, conf_body)
        .with_context(|| format!("wallpaper: cannot write {}", conf.display()))?;

    let cache_dir = home.join(".cache/ml4w/hyprland-dotfiles");
    if cache_dir.is_dir() {
        let cache = cache_dir.join("current_wallpaper");
        std::fs::write(&cache, format!("{}\n", path.display()))
            .with_context(|| format!("wallpaper: cannot write {}", cache.display()))?;
    }

    let _ = run_optional(
        path_env,
        "swaync-client",
        &[String::from("-Ir"), String::from("wallpaper-setter")],
        None,
    );
    Ok(())
}

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// Override setter invoked (`None` when the native setter ran).
    pub setter: Option<PathBuf>,
    /// Image path handed to the setter.
    pub path: PathBuf,
}

/// Set the selected wallpaper: validate the id, resolve the hash to a path,
/// refuse non-files, then run the `SET_WALLPAPER` override if set, else the
/// ported native setter. `path_env` shadows the ambient `PATH` when `Some`
/// (the stub seam tests use); `None` inherits it.
///
/// # Errors
///
/// When the id is malformed (`bad id`, like the wrapper), the hash resolves
/// to nothing (`unknown id`), the resolved path is not a file (`not a
/// file`), the override setter is missing or fails, or the native setter
/// fails. Messages carry no `flex:` prefix; the runner reports them.
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
    if let Some(setter) = set_wallpaper_override() {
        return execute_with(&path, &setter, path_env);
    }
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    set_wallpaper_native(&path, &path_env)?;
    Ok(ExecuteReport { setter: None, path })
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
        setter: Some(setter.to_path_buf()),
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

    fn unit_scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flex-wallpaper-unit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn find_socket_finds_a_nested_socket_and_ignores_a_regular_file() {
        let dir = unit_scratch("find-socket");
        let nested = dir.join("nested/deeper");
        std::fs::create_dir_all(&nested).expect("nested dir");
        // A regular file with the socket name must not satisfy `-type s`.
        std::fs::write(dir.join(".hyprpaper.sock"), b"not a socket").expect("regular file");
        assert_eq!(find_socket(&dir), None, "regular files are not sockets");

        let listener = std::os::unix::net::UnixListener::bind(nested.join(".hyprpaper.sock"))
            .expect("bind socket");
        assert_eq!(
            find_socket(&dir).as_deref(),
            Some(nested.join(".hyprpaper.sock").as_path())
        );
        drop(listener);
        assert_eq!(
            find_socket(&dir.join("missing")),
            None,
            "absent dir is None"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_optional_returns_none_for_a_missing_tool() {
        let dir = unit_scratch("run-optional");
        let path_env = dir.to_string_lossy().into_owned();
        assert_eq!(
            run_optional(&path_env, "definitely-not-a-tool", &[], None),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
