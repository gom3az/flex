//! `theme` executor: the theme-activate flow.
//!
//! Port of the retired `flex-theme.sh` wrapper: after the menu selects a
//! row, the executor validates the id (non-empty, no `/`, no newline),
//! short-circuits
//! the `noop` empty-scan placeholder (exit `0`, B-026), resolves the row
//! hash back to the theme name (the same
//! [`resolve_name`](crate::providers::theme_::resolve_name) scan), and runs
//! the ported native activator ([`activate_theme_native`]), unless
//! `THEME_SWITCHER` names an explicit override program
//! ([`theme_switcher_override`]) that runs `<switcher> activate <name>`.
//!
//! Snapshot convention (the [`exec::shot`](super::shot) template): [`plan`]
//! builds the [`Step`]s from resolved inputs, [`describe`] renders one step
//! as a single line — `<switcher> activate <name>` — unit tests pin the
//! lines, and integration tests diff the stub-`PATH` call logs against the
//! same shapes.
//!
//! Three deliberate departures from the wrapper (all tested):
//!
//! - No subprocess for the default: the wrapper `exec`s
//!   `theme-switcher.sh activate`; the port runs its body in-process
//!   ([`activate_theme_native`]) and keeps `THEME_SWITCHER` as an explicit
//!   override seam only.
//! - No subprocess for resolution: the hash is resolved in-process with the
//!   same [`resolve_name`](crate::providers::theme_::resolve_name) the library
//!   exposes, so the resolution semantics are identical with one fewer
//!   spawn.
//! - Exit-code normalisation: the wrapper `exec`s the switcher so its exit
//!   status propagates verbatim; the port maps every failure through the
//!   shared runner, so any tool failure exits `1` with the single
//!   `flex: error:` prefix. There is no quiet-cancel step (unlike `shot`'s
//!   `slurp`): a failing switcher is always a loud error.

use std::fmt::Write as _;
use std::io::Write as _;
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

/// The explicit `THEME_SWITCHER` override seam: `Some` when the variable is
/// set and non-empty, `None` otherwise (the ported native activator then
/// runs).
#[must_use]
pub fn theme_switcher_override() -> Option<PathBuf> {
    match std::env::var("THEME_SWITCHER") {
        Ok(switcher) if !switcher.is_empty() => Some(PathBuf::from(switcher)),
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

/// Run one optional tool with `args` (and `stdin` piped when set).
///
/// Returns `None` when `name` is not resolvable from `path_env` (or cannot
/// be spawned); otherwise `Some(status.success())`. Stdout/stderr are always
/// nulled, so a missing helper is quiet — the native activator ignores both
/// `None` and `false`, exactly like the wrapper's `… 2>/dev/null || true`.
fn run_optional(path_env: &str, name: &str, args: &[String], stdin: Option<&[u8]>) -> Option<bool> {
    let bin = resolve_tool(name, path_env)?;
    let mut cmd = Command::new(&bin);
    cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd.spawn().ok()?;
    if let Some(input) = stdin {
        if let Some(mut handle) = child.stdin.take() {
            let _ = handle.write_all(input);
        }
    }
    Some(child.wait().is_ok_and(|status| status.success()))
}

/// Copy every regular file at the top level of `src` into `current`
/// (overwrite, no recursion — the wrapper's `for f in "$src"/*`).
///
/// # Errors
///
/// When `src` cannot be read or a file cannot be copied.
fn copy_theme_files(src: &Path, current: &Path) -> Result<()> {
    let entries =
        std::fs::read_dir(src).with_context(|| format!("theme: cannot read {}", src.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        std::fs::copy(&path, current.join(entry.file_name()))
            .with_context(|| format!("theme: cannot copy {}", path.display()))?;
    }
    Ok(())
}

/// Best-effort relative symlink `target -> source` (the wrapper's
/// `ln -sf "theme.css" …`): an existing target is removed first, and every
/// failure is ignored.
fn relink_relative(source: &str, target: &Path) {
    let _ = std::fs::remove_file(target);
    let _ = std::os::unix::fs::symlink(source, target);
}

/// Best-effort `metadata.json` `theme_name` update (the wrapper's
/// `python3 … || true`): every read, parse, and write failure is ignored.
fn update_metadata(meta: &Path, name: &str) {
    let Ok(text) = std::fs::read_to_string(meta) else {
        return;
    };
    let Some(updated) = update_theme_name(&text, name) else {
        return;
    };
    let _ = std::fs::write(meta, updated);
}

/// Correctly JSON-escape `value` (quotes, backslashes, and control chars).
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out
}

/// Replace the top-level `"theme_name"` value in `text` with `name`, insert
/// the key before the final `}`, or return `None` when there is no `{`/`}`.
///
/// Std-only textual update (no `serde`): the key's `:` and JSON string value
/// are located directly and only the quoted value is rewritten. A key hit
/// without a recognizable string value leaves `text` unchanged (`None`).
fn update_theme_name(text: &str, name: &str) -> Option<String> {
    let open = text.find('{')?;
    let close = text.rfind('}')?;
    if close < open {
        return None;
    }
    let escaped = json_escape(name);
    let quoted = "\"theme_name\"";
    if let Some(offset) = text[open + 1..close].find(quoted) {
        let key_at = open + 1 + offset;
        let bytes = text.as_bytes();
        let mut cursor = key_at + quoted.len();
        while cursor < close && matches!(bytes[cursor], b' ' | b'\t' | b'\r' | b'\n') {
            cursor += 1;
        }
        if cursor >= close || bytes[cursor] != b':' {
            return None;
        }
        cursor += 1;
        while cursor < close && matches!(bytes[cursor], b' ' | b'\t' | b'\r' | b'\n') {
            cursor += 1;
        }
        if cursor >= close || bytes[cursor] != b'"' {
            return None;
        }
        let value_start = cursor;
        cursor += 1;
        while cursor < close {
            match bytes[cursor] {
                b'\\' => cursor += 2,
                b'"' => break,
                _ => cursor += 1,
            }
        }
        if cursor >= close || bytes[cursor] != b'"' {
            return None;
        }
        let value_end = cursor + 1;
        let mut out = String::with_capacity(text.len() + escaped.len());
        out.push_str(&text[..value_start]);
        out.push('"');
        out.push_str(&escaped);
        out.push('"');
        out.push_str(&text[value_end..]);
        return Some(out);
    }
    let mut out = String::with_capacity(text.len() + escaped.len() + 24);
    out.push_str(&text[..close]);
    if !text[open + 1..close].trim().is_empty() {
        out.push(',');
    }
    out.push_str("\n    \"theme_name\": \"");
    out.push_str(&escaped);
    out.push('"');
    out.push_str(&text[close..]);
    Some(out)
}

/// Activate `name` with the ported `activate_theme.sh` flow, reading `HOME`.
///
/// # Errors
///
/// When `HOME` is unset (`theme: HOME is not set`), `name` is malformed, the
/// theme is missing or has no theme files, or a copy/link fails. Messages
/// carry no `flex:` prefix; the runner reports them.
pub fn activate_theme_native(name: &str, path_env: &str) -> Result<()> {
    let home = std::env::var("HOME").context("theme: HOME is not set")?;
    activate_theme_native_in(name, Path::new(&home), path_env)
}

/// Native activator body with the resolved inputs given (the test seam).
///
/// Copies the theme files, refreshes the compat and config symlinks, updates
/// the metadata name, and best-effort reloads waybar/hyprland/kitty and
/// notifies. The wrapper's exact order and its `|| true` arms are preserved.
///
/// # Errors
///
/// When `name` is empty or contains `/`/newline (`theme: bad name`), the
/// theme is missing (`theme: theme '<name>' not found in <available>`), has
/// no `theme.css`/`colors.css` (`has no theme files`), or a copy/link fails.
pub fn activate_theme_native_in(name: &str, home: &Path, path_env: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\n') {
        anyhow::bail!("theme: bad name: {name}");
    }
    let available = home.join(".config/themes/available");
    let current = home.join(".config/themes/current");
    let src = available.join(name);
    if !src.is_dir() {
        anyhow::bail!("theme: theme '{name}' not found in {}", available.display());
    }
    if !src.join("theme.css").is_file() && !src.join("colors.css").is_file() {
        anyhow::bail!("theme: theme '{name}' has no theme files");
    }

    std::fs::create_dir_all(&current)
        .with_context(|| format!("theme: cannot create {}", current.display()))?;
    copy_theme_files(&src, &current)?;

    relink_relative("theme.css", &current.join("colors.css"));
    relink_relative("theme.lua", &current.join("colors.lua"));

    update_metadata(&current.join("metadata.json"), name);

    for (target_rel, source_name) in CONFIG_LINKS {
        let source = current.join(source_name);
        if !source.is_file() {
            continue;
        }
        let target = home.join(target_rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("theme: cannot create {}", parent.display()))?;
        }
        let _ = std::fs::remove_file(&target);
        std::os::unix::fs::symlink(&source, &target)
            .with_context(|| format!("theme: cannot link {}", target.display()))?;
    }

    if run_optional(
        path_env,
        "pgrep",
        &[String::from("-x"), String::from("waybar")],
        None,
    ) == Some(true)
    {
        let _ = run_optional(
            path_env,
            "pkill",
            &[String::from("-SIGUSR2"), String::from("waybar")],
            None,
        );
    }
    if resolve_tool("hyprctl", path_env).is_some() {
        let _ = run_optional(path_env, "hyprctl", &[String::from("reload")], None);
    }
    if run_optional(
        path_env,
        "pgrep",
        &[String::from("-x"), String::from("kitty")],
        None,
    ) == Some(true)
    {
        let _ = run_optional(
            path_env,
            "killall",
            &[String::from("-SIGUSR1"), String::from("kitty")],
            None,
        );
    }
    let _ = run_optional(
        path_env,
        "notify-send",
        &[
            String::from("-a"),
            String::from("Theme Switcher"),
            String::from("-i"),
            String::from("preferences-desktop-color"),
            String::from("Theme Activated"),
            format!("Switched to: {name}"),
        ],
        None,
    );
    Ok(())
}

/// The seven `current/…` sources and their `$HOME/…` config targets (the
/// wrapper's parallel `config_targets` / `theme_files` arrays).
const CONFIG_LINKS: [(&str, &str); 7] = [
    (".config/waybar/theme.css", "theme.css"),
    (".config/hypr/theme.lua", "theme.lua"),
    (".config/kitty/current-theme.conf", "kitty.conf"),
    (".config/yazi/theme.toml", "yazi.toml"),
    (".config/tmux/tmux-colors.conf", "tmux-colors.conf"),
    (".config/nvim/lua/theme.lua", "nvim-colors.lua"),
    (".config/nvim/lua/nvim-hl.lua", "nvim-hl.lua"),
];

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// Override switcher invoked (`None` for `noop` and for the native
    /// activator — the wrapper exits before reading `THEME_SWITCHER` on
    /// `noop`, and the native path has no switcher).
    pub switcher: Option<PathBuf>,
    /// Activated theme name (`None` for `noop`).
    pub name: Option<String>,
}

/// Activate the selected theme: validate the id, short-circuit `noop`,
/// resolve the hash to a name, then run the `THEME_SWITCHER` override if set,
/// else the ported native activator. `path_env` shadows the ambient `PATH`
/// when `Some` (the stub seam tests use); `None` inherits it.
///
/// # Errors
///
/// When the id is malformed (`bad id`, like the wrapper), the hash resolves
/// to nothing (`unknown id`), the resolved name is malformed (`bad name`),
/// the override switcher is missing or fails, or the native activator fails.
/// Messages carry no `flex:` prefix; the runner reports them.
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
    if let Some(switcher) = theme_switcher_override() {
        return execute_with(&name, &switcher, path_env);
    }
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    activate_theme_native(&name, &path_env)?;
    Ok(ExecuteReport {
        switcher: None,
        name: Some(name),
    })
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
    fn theme_switcher_override_only_returns_a_nonempty_override() {
        let saved_switcher = std::env::var("THEME_SWITCHER").ok();
        let saved_home = std::env::var("HOME").ok();
        std::env::set_var("THEME_SWITCHER", "/tmp/stub-switcher.sh");
        assert_eq!(
            theme_switcher_override().as_deref(),
            Some(Path::new("/tmp/stub-switcher.sh"))
        );
        // Empty means native: no HOME fallback to the retired script.
        std::env::set_var("THEME_SWITCHER", "");
        std::env::set_var("HOME", "/tmp/fake-home");
        assert_eq!(theme_switcher_override(), None);
        std::env::remove_var("THEME_SWITCHER");
        assert_eq!(theme_switcher_override(), None);
        match saved_switcher {
            Some(value) => std::env::set_var("THEME_SWITCHER", value),
            None => std::env::remove_var("THEME_SWITCHER"),
        }
        match saved_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn activate_theme_native_rejects_bad_names() {
        for bad in ["", "a/b", "a\nb"] {
            let err = activate_theme_native_in(bad, Path::new("/tmp/flex-theme-native-home"), "")
                .expect_err("bad name");
            assert_eq!(format!("{err:#}"), format!("theme: bad name: {bad}"));
        }
    }

    #[test]
    fn update_theme_name_replaces_an_existing_value() {
        let text = "{\n    \"theme_name\": \"old\",\n    \"wallpaper\": \"/walls/old.png\"\n}\n";
        let updated = update_theme_name(text, "demo").expect("updated");
        assert_eq!(
            updated,
            "{\n    \"theme_name\": \"demo\",\n    \"wallpaper\": \"/walls/old.png\"\n}\n"
        );
    }

    #[test]
    fn update_theme_name_inserts_a_missing_key_before_the_final_brace() {
        let updated = update_theme_name("{\"wallpaper\": \"/w.png\"}", "demo").expect("updated");
        assert_eq!(
            updated,
            "{\"wallpaper\": \"/w.png\",\n    \"theme_name\": \"demo\"}"
        );
    }

    #[test]
    fn update_theme_name_inserts_into_an_empty_object_without_a_leading_comma() {
        assert_eq!(
            update_theme_name("{}", "demo").expect("updated"),
            "{\n    \"theme_name\": \"demo\"}"
        );
        assert_eq!(
            update_theme_name("{  }", "demo").expect("updated"),
            "{  \n    \"theme_name\": \"demo\"}"
        );
    }

    #[test]
    fn update_theme_name_json_escapes_quotes_and_backslashes() {
        let updated = update_theme_name("{\"theme_name\": \"old\"}", "a\"b\\c").expect("updated");
        assert_eq!(updated, "{\"theme_name\": \"a\\\"b\\\\c\"}");
    }

    #[test]
    fn update_theme_name_leaves_non_json_unchanged() {
        assert_eq!(update_theme_name("not json", "demo"), None);
        assert_eq!(update_theme_name("{\"theme_name\": \"old\"", "demo"), None);
        // A key hit without a string value is left untouched.
        assert_eq!(update_theme_name("{\"theme_name\": 3}", "demo"), None);
    }
}
