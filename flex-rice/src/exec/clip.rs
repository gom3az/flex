//! `clip` executor: the clipboard-history copy/delete/pin-toggle flow.
//!
//! Port of the retired `flex-clip.sh` wrapper: after the menu selects a row,
//! the executor handles the select, delete and toggle outcomes (clip rows are
//! deletable, so unlike `shot`/`theme`/`wallpaper`/`launch` all three are
//! live), validates the id (`^[0-9a-f]+$`, the content-hash hex), resolves it
//! back to the stored line (the same
//! [`resolve`](crate::providers::clip::resolve) scan), and:
//!
//! - `select`: decodes `<NEWLINE>` back to newlines and pipes the entry
//!   (plus a trailing newline, the wrapper's `printf '%s\n'`) into `wl-copy`
//!   (stdout/stderr to `/dev/null`), then
//!   `notify-send -a Cliphist "Copied to clipboard" <preview>`;
//! - `delete`: removes every line exactly equal to the entry from both the
//!   history and pins files (`LC_ALL=C grep -aFxv`, via a `.tmp` + `mv`
//!   rewrite, skipped per file when the file is absent), then
//!   `notify-send -a Cliphist "Deleted" <preview>`;
//! - `toggle`: creates the pins parent dir, touches a missing pins file,
//!   then removes the entry (`Unpinned`) or appends it (`Pinned to history`)
//!   depending on whether it is already pinned, plus the matching notify.
//!
//! The preview is the first 50 bytes of the entry (the wrapper's
//! `head -c 50`).
//!
//! Snapshot convention (the [`exec::shot`](super::shot) template): [`plan`]
//! builds the [`Step`]s from resolved inputs, [`describe`] renders one step
//! as a single line — `wl-copy` (decoded entry on stdin), `remove <raw>
//! from history and pins` / `remove <raw> from pins` (exact-line file
//! scrubs), `append <raw> to pins` (pin), and
//! `notify-send -a Cliphist <summary> <preview>` — unit tests pin the lines,
//! and integration tests diff the stub-`PATH` call logs against the same
//! shapes plus byte-compare the resulting store files.
//!
//! Five deliberate departures from the wrapper (all tested):
//!
//! - No subprocess: the hash is resolved in-process with the same
//!   [`resolve`](crate::providers::clip::resolve) the library exposes, so
//!   the resolution semantics are identical with one fewer spawn.
//! - Exit-code normalisation: the wrapper `exec`s under `set -e` so a tool
//!   failure propagates verbatim; the port maps every failure through the
//!   shared runner, so any tool failure exits `1` with the single
//!   `flex: error:` prefix. There is no quiet-cancel step (unlike `shot`'s
//!   `slurp`): a failing copy/delete/toggle is always a loud error.
//! - `noop` short-circuits to `Ok` (the B-026 convention shared with
//!   `theme`/`launch`); the wrapper would reject it as a `bad id` since it
//!   is not hex. Unreachable in practice: an empty store exits 130 in the
//!   shared runner before any menu is built, so no `noop` row ever exists.
//! - Byte-split multibyte previews are lossy: `head -c 50` can cut a UTF-8
//!   sequence mid-character and pass the raw split bytes on; the port
//!   lossy-decodes the 50-byte prefix instead, so the notify arg is always
//!   valid UTF-8 (`U+FFFD` at a split point). ASCII previews — every real
//!   history line's first 50 bytes — are byte-identical.
//! - Unchanged store files are left untouched (mtime preserved): the wrapper
//!   always rewrites via `.tmp` + `mv`, which is byte-identical content
//!   whenever the file ends with a newline; the port skips the write when
//!   the scrubbed bytes equal the input bytes.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::providers::clip;

/// Which menu outcome is being executed: clip is the only provider (besides
/// `center`/`wifi`) whose rows are deletable, so the binary
/// maps `Chosen`/`Delete`/`Toggle` onto these three ops and bails on
/// `Target` (clip rows carry no dropdown targets, and the wrapper has no
/// `ACTION:TARGET` arm either).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipOp {
    /// Copy the entry to the clipboard (`Chosen`).
    Copy,
    /// Remove the entry from history and pins (`Delete`).
    Delete,
    /// Pin/unpin the entry (`Toggle`).
    Toggle,
}

/// A validated clip action id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipAction {
    /// The `noop` empty-scan placeholder: exit `0`, never resolve or touch
    /// the store (the B-026 convention; see the module docs for why the
    /// wrapper would disagree).
    Noop,
    /// A content-hash hex id to resolve back to the stored line.
    Id(String),
}

impl ClipAction {
    /// Validate a menu action id, mirroring the wrapper's id check
    /// (`^[0-9a-f]+$`). `None` is the wrapper's `bad id` exit; `noop` is
    /// the empty-scan placeholder the other executors short-circuit.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        if id == "noop" {
            return Some(Self::Noop);
        }
        if !id.is_empty()
            && id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            Some(Self::Id(id.to_string()))
        } else {
            None
        }
    }
}

/// One clip step: a tool spawn or an exact-line store edit (pure data, no
/// process started, no file touched).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Pipe the decoded entry (plus trailing newline) into `wl-copy`
    /// (stdout/stderr to `/dev/null`, like the wrapper's redirect).
    WlCopy,
    /// Remove every line exactly equal to the entry from the history file
    /// and (`pins_only == false`) or only (`pins_only == true`) the pins
    /// file (the wrapper's `grep -aFxv` + `.tmp` + `mv` per file).
    Scrub {
        /// Full stored (`<NEWLINE>`-encoded) line to remove.
        raw: String,
        /// Whether only the pins file is scrubbed (unpin) rather than both
        /// files (delete).
        pins_only: bool,
    },
    /// Append the entry to the pins file (the wrapper appends the
    /// `printf '%s\n'` output to the pins file).
    AppendPin {
        /// Full stored (`<NEWLINE>`-encoded) line to append.
        raw: String,
    },
    /// `notify-send -a Cliphist <summary> <preview>`.
    Notify {
        /// `Copied to clipboard` / `Deleted` / `Pinned to history` /
        /// `Unpinned` (the wrapper's four summaries, verbatim).
        summary: String,
        /// First-50-bytes preview of the entry.
        preview: String,
    },
}

/// Build the [`Step`]s for `op` over the already-resolved `raw` line (no
/// process started, no file touched).
///
/// `pinned` is the entry's current pin state and is read only for
/// [`ClipOp::Toggle`] (`true` → unpin scrub, `false` → pin append).
#[must_use]
pub fn plan(op: ClipOp, raw: &str, pinned: bool) -> Vec<Step> {
    let preview = preview(raw);
    match op {
        ClipOp::Copy => vec![
            Step::WlCopy,
            Step::Notify {
                summary: String::from("Copied to clipboard"),
                preview,
            },
        ],
        ClipOp::Delete => vec![
            Step::Scrub {
                raw: raw.to_string(),
                pins_only: false,
            },
            Step::Notify {
                summary: String::from("Deleted"),
                preview,
            },
        ],
        ClipOp::Toggle if pinned => vec![
            Step::Scrub {
                raw: raw.to_string(),
                pins_only: true,
            },
            Step::Notify {
                summary: String::from("Unpinned"),
                preview,
            },
        ],
        ClipOp::Toggle => vec![
            Step::AppendPin {
                raw: raw.to_string(),
            },
            Step::Notify {
                summary: String::from("Pinned to history"),
                preview,
            },
        ],
    }
}

/// Render one [`Step`] as a single snapshot line.
///
/// This is the template snapshot convention: unit tests pin these lines per
/// id (pure, no spawn), and the integration tests diff the stub-`PATH` call
/// logs against the same shapes. `WlCopy` carries its payload on stdin (the
/// decoded entry plus trailing newline), so the line is just `wl-copy` and
/// the stdin bytes are pinned separately by the `copied.bin` assertions.
#[must_use]
pub fn describe(step: &Step) -> String {
    match step {
        Step::WlCopy => String::from("wl-copy"),
        Step::Scrub { raw, pins_only } => {
            if *pins_only {
                format!("remove {raw} from pins")
            } else {
                format!("remove {raw} from history and pins")
            }
        }
        Step::AppendPin { raw } => format!("append {raw} to pins"),
        Step::Notify { summary, preview } => {
            format!("notify-send -a Cliphist {summary} {preview}")
        }
    }
}

/// Render a whole [`plan`] as snapshot lines (see [`describe`]).
#[must_use]
pub fn describe_plan(steps: &[Step]) -> Vec<String> {
    steps.iter().map(describe).collect()
}

/// First-50-bytes preview of `raw` (the wrapper's `head -c 50`).
///
/// Byte-based on purpose, lossy-decoded so the notify arg is always valid
/// UTF-8 (see the module docs: a multibyte character straddling byte 50
/// surfaces as `U+FFFD` instead of raw split bytes).
#[must_use]
pub fn preview(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let end = bytes.len().min(50);
    match bytes.get(..end) {
        Some(prefix) => String::from_utf8_lossy(prefix).into_owned(),
        None => String::new(),
    }
}

/// `wl-copy` stdin for `raw`: `<NEWLINE>` decoded back to newlines plus the
/// wrapper's `printf '%s\n'` trailing newline.
#[must_use]
pub fn copy_stdin(raw: &str) -> Vec<u8> {
    let mut decoded = clip::decode(raw);
    decoded.push('\n');
    decoded.into_bytes()
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

/// Run one tool with `args` and `stdin_bytes` on stdin.
///
/// Stdout/stderr go to `/dev/null` (the wrapper's `wl-copy` redirect). A
/// non-zero exit is `Ok` with `ok: false`, not an error — callers map it.
/// Messages carry no `flex:` prefix; the runner reports them.
///
/// # Errors
///
/// When the tool is missing from `path_env`, stdin cannot be piped, or the
/// spawn/wait itself fails.
fn tool_piped(path_env: &str, name: &str, args: &[String], stdin_bytes: &[u8]) -> Result<bool> {
    let Some(bin) = resolve_tool(name, path_env) else {
        anyhow::bail!("clip: {name} not found on PATH");
    };
    let mut child = Command::new(&bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("clip: failed to run {name}"))?;
    {
        let Some(stdin) = child.stdin.as_mut() else {
            anyhow::bail!("clip: failed to pipe to {name}");
        };
        stdin
            .write_all(stdin_bytes)
            .with_context(|| format!("clip: failed to pipe to {name}"))?;
    }
    let status = child
        .wait()
        .with_context(|| format!("clip: failed to run {name}"))?;
    Ok(status.success())
}

/// Run one tool with `args`.
///
/// Stdout/stderr are inherited, like the wrapper's bare `notify-send`
/// (feedback reaches the popup); stdin is nulled. A non-zero exit is `Ok`
/// with `ok: false`, not an error — callers map it. Messages carry no
/// `flex:` prefix; the runner reports them.
///
/// # Errors
///
/// When the tool is missing from `path_env` or the spawn itself fails.
fn tool(path_env: &str, name: &str, args: &[String]) -> Result<bool> {
    let Some(bin) = resolve_tool(name, path_env) else {
        anyhow::bail!("clip: {name} not found on PATH");
    };
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("clip: failed to run {name}"))?;
    Ok(status.success())
}

/// `.tmp` sibling the scrub/append rewrites go through (the wrapper's
/// `"${HISTFILE}.tmp"` / `"${PINFILE}.tmp"` + `mv`).
fn tmp_for(path: &Path) -> PathBuf {
    let mut buf = path.as_os_str().to_owned();
    buf.push(".tmp");
    PathBuf::from(buf)
}

/// Split store bytes into content lines: file order, trailing-newline
/// agnostic (a missing trailing newline is normalised to one on rewrite,
/// exactly like the wrapper's `grep` + `mv` output).
fn store_lines(bytes: &[u8]) -> Vec<&[u8]> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&[u8]> = bytes.split(|byte| *byte == b'\n').collect();
    if lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    lines
}

/// Whether any store line is exactly `raw` (the wrapper's `grep -qFx`).
fn store_contains(bytes: &[u8], raw: &[u8]) -> bool {
    store_lines(bytes).contains(&raw)
}

/// Remove every line exactly equal to `raw` from `path` (the wrapper's
/// `LC_ALL=C grep -aFxv -- "$raw" file > file.tmp || true; mv file.tmp
/// file`).
///
/// A missing/unreadable file is skipped (the wrapper's `[[ -f … ]]` guard);
/// an unchanged file is left untouched (byte-identical to the wrapper's
/// rewrite whenever the input ends with a newline — see the module docs).
/// Returns whether the file was rewritten.
///
/// # Errors
///
/// When the `.tmp` rewrite or the rename fails.
fn scrub_file(path: &Path, raw: &[u8]) -> Result<bool> {
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(false);
    };
    if bytes.is_empty() {
        return Ok(false);
    }
    let kept: Vec<&[u8]> = store_lines(&bytes)
        .into_iter()
        .filter(|line| *line != raw)
        .collect();
    let mut rewritten: Vec<u8> = kept.join(&b'\n');
    if !kept.is_empty() {
        rewritten.push(b'\n');
    }
    if rewritten == bytes {
        return Ok(false);
    }
    let tmp = tmp_for(path);
    std::fs::write(&tmp, &rewritten)
        .with_context(|| format!("clip: cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("clip: cannot update {}", path.display()))?;
    Ok(true)
}

/// Append `raw` plus a trailing newline to `path` (the wrapper's
/// `printf '%s\n' "$raw" >> "$PINFILE"`), creating parent dirs and the file
/// first (the wrapper's `mkdir -p` + `touch`).
///
/// # Errors
///
/// When dirs cannot be created or the rewrite fails.
fn append_pin(path: &Path, raw: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("clip: cannot create {}", parent.display()))?;
        }
    }
    let mut bytes = std::fs::read(path).unwrap_or_default();
    bytes.extend_from_slice(raw);
    bytes.push(b'\n');
    let tmp = tmp_for(path);
    std::fs::write(&tmp, &bytes)
        .with_context(|| format!("clip: cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("clip: cannot update {}", path.display()))?;
    Ok(())
}

/// Is `raw` currently pinned (exact-line match in the pins file)?
///
/// A missing/unreadable pins file counts as unpinned (the wrapper's
/// `grep -q … 2>/dev/null` is false there too, so the entry gets appended).
#[must_use]
pub fn is_pinned(pins: &Path, raw: &str) -> bool {
    std::fs::read(pins).is_ok_and(|bytes| store_contains(&bytes, raw.as_bytes()))
}

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// The op that ran (`None` when `noop` short-circuited first — the
    /// wrapper exits before resolving, so this does too).
    pub op: Option<ClipOp>,
    /// Resolved stored line (`None` for `noop`).
    pub raw: Option<String>,
    /// Post-run pin state (`Some` only for [`ClipOp::Toggle`]).
    pub pinned: Option<bool>,
}

/// Copy/delete/toggle the selected entry: validate the id, short-circuit
/// `noop`, resolve the hash to its stored line in-process, and run the
/// [`plan`] steps.
/// `path_env` shadows the ambient `PATH` when `Some` (the stub seam tests
/// use); `None` inherits it.
///
/// # Errors
///
/// When the id is malformed (`bad id`, like the wrapper), the hash resolves
/// to nothing (`unknown id`), a store rewrite fails, or a tool is missing
/// or fails. Messages carry no `flex:` prefix; the runner reports them.
pub fn execute(op: ClipOp, action_id: &str, path_env: Option<&str>) -> Result<ExecuteReport> {
    let Some(action) = ClipAction::parse(action_id) else {
        anyhow::bail!("clip: bad id '{action_id}'");
    };
    let ClipAction::Id(id) = action else {
        return Ok(ExecuteReport {
            op: None,
            raw: None,
            pinned: None,
        });
    };
    let raw = clip::resolve(&id);
    let Some(raw) = raw else {
        anyhow::bail!("clip: unknown id '{id}'");
    };
    if raw.is_empty() {
        anyhow::bail!("clip: unknown id '{id}'");
    }
    execute_with(op, &raw, &clip::hist_path(), &clip::pins_path(), path_env)
}

/// Copy/delete/toggle half of [`execute`], with the resolved inputs given
/// (no env reads, no resolve): the shape integration tests drive with a
/// stub `PATH` and scratch store files.
///
/// # Errors
///
/// When a store rewrite fails, or a tool is missing or exits non-zero
/// (always loud — there is no quiet-cancel step). Messages carry no `flex:`
/// prefix.
pub fn execute_with(
    op: ClipOp,
    raw: &str,
    hist: &Path,
    pins: &Path,
    path_env: Option<&str>,
) -> Result<ExecuteReport> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let pinned = op == ClipOp::Toggle && is_pinned(pins, raw);
    for step in plan(op, raw, pinned) {
        match step {
            Step::WlCopy => {
                if !tool_piped(&path_env, "wl-copy", &[], &copy_stdin(raw))? {
                    anyhow::bail!("clip: wl-copy failed");
                }
            }
            Step::Scrub { raw, pins_only } => {
                if !pins_only {
                    scrub_file(hist, raw.as_bytes())?;
                }
                scrub_file(pins, raw.as_bytes())?;
            }
            Step::AppendPin { raw } => {
                append_pin(pins, raw.as_bytes())?;
            }
            Step::Notify { summary, preview } => {
                let args = vec![
                    String::from("-a"),
                    String::from("Cliphist"),
                    summary.clone(),
                    preview.clone(),
                ];
                if !tool(&path_env, "notify-send", &args)? {
                    anyhow::bail!("clip: notify-send failed");
                }
            }
        }
    }
    Ok(ExecuteReport {
        op: Some(op),
        raw: Some(raw.to_string()),
        pinned: if op == ClipOp::Toggle {
            Some(!pinned)
        } else {
            None
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_accept_lowercase_hex_of_any_length() {
        assert_eq!(
            ClipAction::parse("0123456789abcdef"),
            Some(ClipAction::Id(String::from("0123456789abcdef"))),
        );
        assert_eq!(
            ClipAction::parse("deadbeef"),
            Some(ClipAction::Id(String::from("deadbeef"))),
        );
        assert_eq!(ClipAction::parse("noop"), Some(ClipAction::Noop));
    }

    #[test]
    fn ids_reject_the_wrapper_bad_id_set() {
        for bad in [
            "",
            "a/b",
            "a\nb",
            "/lead",
            "trail/",
            "ABCDEF",
            "Noop",
            "deadbeef!",
        ] {
            assert_eq!(ClipAction::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn plan_snapshot_copy_is_wl_copy_plus_notify() {
        assert_eq!(
            describe_plan(&plan(ClipOp::Copy, "hello", false)),
            vec![
                "wl-copy",
                "notify-send -a Cliphist Copied to clipboard hello",
            ],
        );
    }

    #[test]
    fn plan_snapshot_delete_scrubs_both_files() {
        assert_eq!(
            describe_plan(&plan(ClipOp::Delete, "doomed", false)),
            vec![
                "remove doomed from history and pins",
                "notify-send -a Cliphist Deleted doomed",
            ],
        );
    }

    #[test]
    fn plan_snapshot_toggle_pins_or_unpins() {
        assert_eq!(
            describe_plan(&plan(ClipOp::Toggle, "flip", false)),
            vec![
                "append flip to pins",
                "notify-send -a Cliphist Pinned to history flip",
            ],
        );
        assert_eq!(
            describe_plan(&plan(ClipOp::Toggle, "flip", true)),
            vec![
                "remove flip from pins",
                "notify-send -a Cliphist Unpinned flip",
            ],
        );
    }

    #[test]
    fn preview_is_the_first_fifty_bytes() {
        assert_eq!(preview("short"), "short");
        let long = "L".repeat(200);
        assert_eq!(preview(&long), "L".repeat(50));
    }

    #[test]
    fn preview_never_panics_on_a_multibyte_split() {
        // `é` is 2 bytes: 49 ASCII bytes + `é` straddles byte 50, so the
        // lossy prefix ends in `U+FFFD` instead of raw split bytes (module
        // docs: deliberate departure from `head -c 50`).
        let raw = format!("{}{}", "a".repeat(49), "éclair");
        assert_eq!(preview(&raw), format!("{}{}", "a".repeat(49), "�"));
    }

    #[test]
    fn copy_stdin_decodes_the_placeholder_and_adds_a_newline() {
        assert_eq!(
            copy_stdin("alpha<NEWLINE>beta gamma"),
            b"alpha\nbeta gamma\n".to_vec(),
        );
        assert_eq!(copy_stdin("plain"), b"plain\n".to_vec());
    }

    #[test]
    fn store_lines_handles_a_missing_trailing_newline() {
        assert_eq!(
            store_lines(b"a\nb\n"),
            vec![b"a".as_slice(), b"b".as_slice()]
        );
        assert_eq!(store_lines(b"a\nb"), vec![b"a".as_slice(), b"b".as_slice()]);
        assert!(store_lines(b"").is_empty());
    }

    #[test]
    fn store_contains_matches_exact_lines_only() {
        assert!(store_contains(b"flip me\nother\n", b"flip me"));
        assert!(!store_contains(b"flip me extended\nother\n", b"flip me"));
        assert!(!store_contains(b"", b"flip me"));
    }
}
