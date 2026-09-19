//! `shot` executor: the screenshot/recording flow.
//!
//! Port of the retired `flex-shot.sh` wrapper: after the menu selects a row,
//! the parent stages `SCREENSHOT_DIR`, timestamps the
//! capture, detaches a worker (`setsid -f flex-shot --capture …`) so the
//! capture survives the popup kill, then closes the popup and exits `0`. The
//! worker runs the capture synchronously: a `slurp` region (area modes),
//! `hyprctl -j activewindow` geometry (window mode), `grim` + `wl-copy` +
//! `notify-send` (screenshots), or the `RECORDING_START` helper (recordings).
//!
//! Four deliberate departures from the wrapper (all tested):
//!
//! - No generated script: the wrapper writes a temp capture script and runs it
//!   under `setsid --fork bash` (plus a `trap` cleanup); the port builds a
//!   [`plan`] of [`Step`]s and runs them with direct `Command` spawns.
//! - No `jq` or `python3`: the window geometry is parsed in-process from
//!   `hyprctl -j activewindow` ([`active_geometry`]), and the popup-close wait
//!   polls `pgrep -f "flex-menu "` (20 × 50 ms, the wrapper's 1 s cap)
//!   instead of `hyprctl clients -j` piped to Python.
//! - Exact-class popup match: the wrapper's `pgrep -f 'kitty --class
//!   kitty-menu'` plus its `'kitty-menu' in class` substring also matched
//!   `kitty-menu-wide`; the port matches [`MENU_CLASS`] with the trailing
//!   space ([`popup::match_pattern`]), so the two variants never collide.
//! - `slurp` cancel exits 130 quietly: the wrapper's `set +e` fell through to
//!   `wl-copy`/`notify-send` on cancel; the port treats a failed `slurp` as
//!   user-cancelled and stops before `grim`.
//!
//! [`MENU_CLASS`]: crate::popup::MENU_CLASS
//! [`popup::match_pattern`]: crate::popup::match_pattern

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result};

use crate::popup;
use crate::spawn::RetryExec as _;

/// Capture rows in bash `case`-arm order (screenshots, then recordings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotId {
    /// `slurp` region → `grim` PNG.
    AreaShot,
    /// Full-screen `grim` PNG.
    FullShot,
    /// Active-window `grim` PNG.
    WinShot,
    /// `slurp` region → recording helper MP4.
    AreaRec,
    /// `slurp` region → recording helper MP4, with audio.
    AreaRecAudio,
    /// Full-screen recording helper MP4.
    FullRec,
    /// Full-screen recording helper MP4, with audio.
    FullRecAudio,
}

impl ShotId {
    /// Parse a menu action id (exact match, so the wrapper's bad-id set —
    /// empty, `/`-bearing, newline-bearing, unknown — all reject here).
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "area-shot" => Some(Self::AreaShot),
            "full-shot" => Some(Self::FullShot),
            "win-shot" => Some(Self::WinShot),
            "area-rec" => Some(Self::AreaRec),
            "area-rec-audio" => Some(Self::AreaRecAudio),
            "full-rec" => Some(Self::FullRec),
            "full-rec-audio" => Some(Self::FullRecAudio),
            _ => None,
        }
    }

    /// Bash `case`-arm spelling (also the detached worker's `ID` argument).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AreaShot => "area-shot",
            Self::FullShot => "full-shot",
            Self::WinShot => "win-shot",
            Self::AreaRec => "area-rec",
            Self::AreaRecAudio => "area-rec-audio",
            Self::FullRec => "full-rec",
            Self::FullRecAudio => "full-rec-audio",
        }
    }

    /// Whether this capture is a recording (MP4) rather than a shot (PNG).
    #[must_use]
    pub fn is_recording(self) -> bool {
        match self {
            Self::AreaShot | Self::FullShot | Self::WinShot => false,
            Self::AreaRec | Self::AreaRecAudio | Self::FullRec | Self::FullRecAudio => true,
        }
    }
}

/// Where a capture's region geometry comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Geom {
    /// `slurp` stdout (`x,y WxH`), resolved at runtime.
    Slurp,
    /// `hyprctl -j activewindow`, resolved at runtime.
    Window,
    /// Already-resolved geometry.
    Fixed(String),
}

impl Geom {
    /// Snapshot spelling: `<slurp>`/`<window>` placeholders for runtime
    /// values, the value itself when fixed.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Slurp => String::from("<slurp>"),
            Self::Window => String::from("<window>"),
            Self::Fixed(value) => value.clone(),
        }
    }
}

/// One capture step: a tool spawn (pure data, no process started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `slurp`; stdout is the `x,y WxH` geometry.
    Slurp,
    /// `hyprctl -j activewindow`, parsed in-process to `x,y WxH`.
    ActiveWindow,
    /// `grim [-g GEOM] FILE`.
    Grim {
        /// Region (`None` = fullscreen, no `-g` flag).
        geometry: Option<Geom>,
        /// Output file.
        file: String,
    },
    /// `wl-copy --type image/png < FILE` (typed offer so image-aware
    /// targets paste pixels, not text).
    WlCopy {
        /// File piped to `wl-copy`'s stdin.
        file: String,
    },
    /// `notify-send -h string:image-path:FILE "Screenshot saved" FILE` (the
    /// hint gives the drawer a graphic preview plus the Copy Image target).
    Notify {
        /// Saved file named in the notification.
        file: String,
    },
    /// `REC [-a] [-g GEOM] FILE` (the `RECORDING_START` helper).
    Record {
        /// Recording helper program.
        rec: String,
        /// Whether `-a` (audio) is passed.
        audio: bool,
        /// Region (`None` = fullscreen, no `-g` flag).
        geometry: Option<Geom>,
        /// Output file.
        file: String,
    },
}

/// Build the capture [`Step`]s for `id` (no process started).
///
/// `file` is the staged output path, `rec` the `RECORDING_START` helper.
/// Region steps (`Slurp`/`ActiveWindow`) always precede their consumer, so
/// the worker resolves each [`Geom::Slurp`]/[`Geom::Window`] in order.
#[must_use]
pub fn plan(id: ShotId, file: &str, rec: &str) -> Vec<Step> {
    let file = file.to_string();
    let rec = rec.to_string();
    match id {
        ShotId::AreaShot => vec![
            Step::Slurp,
            Step::Grim {
                geometry: Some(Geom::Slurp),
                file: file.clone(),
            },
            Step::WlCopy { file: file.clone() },
            Step::Notify { file },
        ],
        ShotId::FullShot => vec![
            Step::Grim {
                geometry: None,
                file: file.clone(),
            },
            Step::WlCopy { file: file.clone() },
            Step::Notify { file },
        ],
        ShotId::WinShot => vec![
            Step::ActiveWindow,
            Step::Grim {
                geometry: Some(Geom::Window),
                file: file.clone(),
            },
            Step::WlCopy { file: file.clone() },
            Step::Notify { file },
        ],
        ShotId::AreaRec => vec![
            Step::Slurp,
            Step::Record {
                rec,
                audio: false,
                geometry: Some(Geom::Slurp),
                file,
            },
        ],
        ShotId::AreaRecAudio => vec![
            Step::Slurp,
            Step::Record {
                rec,
                audio: true,
                geometry: Some(Geom::Slurp),
                file,
            },
        ],
        ShotId::FullRec => vec![Step::Record {
            rec,
            audio: false,
            geometry: None,
            file,
        }],
        ShotId::FullRecAudio => vec![Step::Record {
            rec,
            audio: true,
            geometry: None,
            file,
        }],
    }
}

/// Render one [`Step`] as a single snapshot line: `argv` joined by spaces,
/// stdin-file steps as `wl-copy --type image/png < FILE`.
///
/// This is the template snapshot convention: unit tests pin these lines per
/// id (pure, no spawn), and the integration tests diff the stub-`PATH` call
/// logs against the same shapes with concrete geometries.
#[must_use]
pub fn describe(step: &Step) -> String {
    match step {
        Step::Slurp => String::from("slurp"),
        Step::ActiveWindow => String::from("hyprctl -j activewindow"),
        Step::Grim { geometry, file } => match geometry {
            None => format!("grim {file}"),
            Some(geom) => format!("grim -g {} {file}", geom.describe()),
        },
        Step::WlCopy { file } => format!("wl-copy --type image/png < {file}"),
        Step::Notify { file } => {
            format!("notify-send -h string:image-path:{file} Screenshot saved {file}")
        }
        Step::Record {
            rec,
            audio,
            geometry,
            file,
        } => {
            let mut line = rec.clone();
            if *audio {
                line.push_str(" -a");
            }
            if let Some(geom) = geometry {
                line.push_str(" -g ");
                line.push_str(&geom.describe());
            }
            line.push(' ');
            line.push_str(file);
            line
        }
    }
}

/// Render a whole [`plan`] as snapshot lines (see [`describe`]).
#[must_use]
pub fn describe_plan(steps: &[Step]) -> Vec<String> {
    steps.iter().map(describe).collect()
}

/// Staged output path: `Screenshot-<ts>.png` for shots,
/// `Recording-<ts>.mp4` for recordings (the wrapper's two arms).
#[must_use]
pub fn filepath_for(id: ShotId, dir: &Path, timestamp: &str) -> PathBuf {
    let (stem, ext) = if id.is_recording() {
        ("Recording", "mp4")
    } else {
        ("Screenshot", "png")
    };
    dir.join(format!("{stem}-{timestamp}.{ext}"))
}

/// Screenshot staging dir: `SCREENSHOT_DIR`, else `$HOME/Pictures/Screenshots`
/// (empty values fall back, like the wrapper's `${VAR:-default}`).
///
/// # Errors
///
/// When neither variable yields a dir (`HOME` unset and no override).
pub fn save_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("SCREENSHOT_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let home = std::env::var("HOME").context("shot: HOME is not set")?;
    Ok(Path::new(&home).join("Pictures/Screenshots"))
}

/// Recording helper: `RECORDING_START`, else `$HOME/.local/bin/flex-record`
/// (empty values fall back). The helper takes `[-a] [-g GEOM] FILE`, which
/// `flex-record` accepts directly (the drop-in start shape).
///
/// # Errors
///
/// When `HOME` is unset and no override is set.
pub fn rec_start() -> Result<PathBuf> {
    if let Ok(rec) = std::env::var("RECORDING_START") {
        if !rec.is_empty() {
            return Ok(PathBuf::from(rec));
        }
    }
    let home = std::env::var("HOME").context("shot: HOME is not set")?;
    Ok(Path::new(&home).join(".local/bin/flex-record"))
}

/// How a detached worker run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureEnd {
    /// Capture ran to completion.
    Done,
    /// The user cancelled the `slurp` region pick (exit 130, quietly).
    Cancelled,
}

/// Worker outcome of one tool spawn.
struct ToolOut {
    /// Whether the tool exited `0`.
    ok: bool,
    /// Raw stdout (geometry/`date` text).
    stdout: String,
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

/// Run one tool with `args` (and `stdin_file` piped to stdin when set).
///
/// # Errors
///
/// When the tool is missing from `path_env`, the stdin file cannot be read,
/// or the spawn itself fails. A non-zero exit is `Ok` with `ok: false`, not
/// an error — callers map it (cancel vs failure). Messages carry no `flex:`
/// prefix; the runner reports them.
fn tool(path_env: &str, name: &str, args: &[String], stdin_file: Option<&Path>) -> Result<ToolOut> {
    let Some(bin) = resolve_tool(name, path_env) else {
        anyhow::bail!("shot: {name} not found on PATH");
    };
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    if let Some(input) = stdin_file {
        let file =
            File::open(input).with_context(|| format!("shot: cannot read {}", input.display()))?;
        cmd.stdin(file);
    } else {
        cmd.stdin(Stdio::null());
    }
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output_retrying()
        .with_context(|| format!("shot: failed to run {name}"))?;
    Ok(ToolOut {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

/// `notify-send` argv for a saved screenshot: the `image-path` hint (drawer
/// graphic preview + Copy Image target) plus the human-readable line.
fn notify_args(file: String) -> Vec<String> {
    vec![
        String::from("-h"),
        format!("string:image-path:{file}"),
        String::from("Screenshot saved"),
        file,
    ]
}

/// First line of `text`, trimmed (geometry/`date` output convention).
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// Find `needle` in `haystack` (byte-wise, panic-free: `None` when absent).
fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let mut start = 0_usize;
    loop {
        let window = haystack.get(start..)?;
        let pos = window
            .iter()
            .position(|byte| Some(byte) == needle.first())?;
        let candidate = start.checked_add(pos)?;
        let slice = haystack.get(candidate..candidate.checked_add(needle.len())?)?;
        if slice.iter().zip(needle.iter()).all(|(a, b)| a == b) {
            return Some(candidate);
        }
        start = candidate.checked_add(1)?;
    }
}

/// Parse one ASCII integer at `cursor` (leading whitespace skipped).
///
/// Returns the digits plus the cursor past them; `None` when no digits
/// follow (all panic-free `get`/`checked_add`, never indexing or slicing).
fn int_at(haystack: &[u8], mut cursor: usize) -> Option<(String, usize)> {
    while let b' ' | b'\t' | b'\n' | b'\r' = haystack.get(cursor)? {
        cursor = cursor.checked_add(1)?;
    }
    let mut text = String::new();
    if haystack.get(cursor) == Some(&b'-') {
        text.push('-');
        cursor = cursor.checked_add(1)?;
    }
    let mut any = false;
    while let Some(byte) = haystack.get(cursor) {
        if !byte.is_ascii_digit() {
            break;
        }
        text.push(char::from(*byte));
        cursor = cursor.checked_add(1)?;
        any = true;
    }
    if any {
        Some((text, cursor))
    } else {
        None
    }
}

/// Parse `[int, int]` after the key at `key_pos` (key length `key_len`).
///
/// Returns both numbers plus the cursor past `]`; surrounding whitespace is
/// allowed, anything else ends the parse with `None`.
fn pair_after(haystack: &[u8], key_pos: usize, key_len: usize) -> Option<(String, String, usize)> {
    let mut cursor = key_pos.checked_add(key_len)?;
    loop {
        let byte = haystack.get(cursor)?;
        cursor = cursor.checked_add(1)?;
        if *byte == b'[' {
            break;
        }
    }
    let (first, next) = int_at(haystack, cursor)?;
    let mut cursor = next;
    loop {
        let byte = haystack.get(cursor)?;
        cursor = cursor.checked_add(1)?;
        if *byte == b',' {
            break;
        }
        if !matches!(*byte, b' ' | b'\t' | b'\n' | b'\r') {
            return None;
        }
    }
    let (second, next) = int_at(haystack, cursor)?;
    let mut cursor = next;
    loop {
        let byte = haystack.get(cursor)?;
        cursor = cursor.checked_add(1)?;
        if *byte == b']' {
            break;
        }
        if !matches!(*byte, b' ' | b'\t' | b'\n' | b'\r') {
            return None;
        }
    }
    Some((first, second, cursor))
}

/// Parse `hyprctl -j activewindow` JSON to the `grim -g` geometry (`x,y WxH`).
///
/// Reads the `"at":[x,y]` and `"size":[w,h]` pairs with a panic-free byte
/// scanner (no `serde_json`: this stays std-only, and the two pairs are the
/// only fields the capture needs). Returns `None` when either pair is
/// missing or malformed — the worker then errors instead of capturing the
/// wrong region.
#[must_use]
pub fn active_geometry(json: &str) -> Option<String> {
    let bytes = json.as_bytes();
    let at = find_sub(bytes, b"\"at\"")?;
    let (x, y, end) = pair_after(bytes, at, "\"at\"".len())?;
    let after = bytes.get(end..)?;
    let rel = find_sub(after, b"\"size\"")?;
    let abs = end.checked_add(rel)?;
    let (width, height, _) = pair_after(bytes, abs, "\"size\"".len())?;
    Some(format!("{x},{y} {width}x{height}"))
}

/// Run one capture synchronously (the detached `--capture` worker body).
///
/// `path_env` shadows the ambient `PATH` when `Some` (the stub seam tests
/// use); `None` inherits it. A failed `slurp` is [`CaptureEnd::Cancelled`]
/// (exit 130, quietly); every other tool failure is an error.
///
/// # Errors
///
/// When a required tool is missing, `hyprctl` output is unparsable, the
/// region is missing, or a capture step fails. Messages carry no `flex:`
/// prefix; the runner reports them.
pub fn run_worker(
    id: ShotId,
    file: &Path,
    rec: &Path,
    path_env: Option<&str>,
) -> Result<CaptureEnd> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let file_str = file.to_string_lossy().into_owned();
    let rec_str = rec.to_string_lossy().into_owned();
    let mut geometry: Option<String> = None;
    for step in plan(id, &file_str, &rec_str) {
        match step {
            Step::Slurp => {
                let out = tool(&path_env, "slurp", &[], None)?;
                if !out.ok {
                    return Ok(CaptureEnd::Cancelled);
                }
                let geom = first_line(&out.stdout);
                if geom.is_empty() {
                    anyhow::bail!("shot: slurp produced no geometry");
                }
                geometry = Some(geom);
            }
            Step::ActiveWindow => {
                let args = vec![String::from("-j"), String::from("activewindow")];
                let out = tool(&path_env, "hyprctl", &args, None)?;
                if !out.ok {
                    anyhow::bail!("shot: hyprctl activewindow failed");
                }
                let Some(geom) = active_geometry(&out.stdout) else {
                    anyhow::bail!("shot: cannot parse hyprctl activewindow geometry");
                };
                geometry = Some(geom);
            }
            Step::Grim {
                geometry: want,
                file,
            } => {
                let resolved = match &want {
                    None => None,
                    Some(Geom::Fixed(value)) => Some(value.clone()),
                    Some(Geom::Slurp | Geom::Window) => geometry.clone(),
                };
                if want.is_some() && resolved.is_none() {
                    anyhow::bail!("shot: capture region missing before grim");
                }
                let mut args = Vec::new();
                if let Some(geom) = resolved {
                    args.push(String::from("-g"));
                    args.push(geom);
                }
                args.push(file);
                let out = tool(&path_env, "grim", &args, None)?;
                if !out.ok {
                    anyhow::bail!("shot: grim failed");
                }
            }
            Step::WlCopy { file } => {
                let args = vec![String::from("--type"), String::from("image/png")];
                let out = tool(&path_env, "wl-copy", &args, Some(Path::new(&file)))?;
                if !out.ok {
                    anyhow::bail!("shot: wl-copy failed");
                }
            }
            Step::Notify { file } => {
                let args = notify_args(file);
                let out = tool(&path_env, "notify-send", &args, None)?;
                if !out.ok {
                    anyhow::bail!("shot: notify-send failed");
                }
            }
            Step::Record {
                rec,
                audio,
                geometry: want,
                file,
            } => {
                let resolved = match &want {
                    None => None,
                    Some(Geom::Fixed(value)) => Some(value.clone()),
                    Some(Geom::Slurp | Geom::Window) => geometry.clone(),
                };
                if want.is_some() && resolved.is_none() {
                    anyhow::bail!("shot: capture region missing before recording");
                }
                let mut args = Vec::new();
                if audio {
                    args.push(String::from("-a"));
                }
                if let Some(geom) = resolved {
                    args.push(String::from("-g"));
                    args.push(geom);
                }
                args.push(file);
                let out = tool(&path_env, &rec, &args, None)?;
                if !out.ok {
                    anyhow::bail!("shot: recording failed");
                }
            }
        }
    }
    Ok(CaptureEnd::Done)
}

/// `date +%Y-%m-%d_%H-%M-%S` (the wrapper's timestamp, byte-identical).
///
/// # Errors
///
/// When `date` is missing, fails, or prints nothing.
fn timestamp(path_env: &str) -> Result<String> {
    let args = vec![String::from("+%Y-%m-%d_%H-%M-%S")];
    let out = tool(path_env, "date", &args, None)?;
    if !out.ok {
        anyhow::bail!("shot: date failed");
    }
    let stamp = first_line(&out.stdout);
    if stamp.is_empty() {
        anyhow::bail!("shot: date produced no output");
    }
    Ok(stamp)
}

/// Detach the capture worker: `setsid -f <self> --capture ID FILE REC` with
/// stdio nulled (the `setsid --fork` half of the wrapper, minus the bash).
///
/// # Errors
///
/// When the current executable path cannot be read, `setsid` is missing, or
/// the spawn fails. Messages carry no `flex:` prefix.
fn spawn_detached_worker(id: ShotId, filepath: &Path, rec: &Path, path_env: &str) -> Result<()> {
    let exe = std::env::current_exe().context("shot: cannot read the current executable path")?;
    let Some(setsid) = resolve_tool("setsid", path_env) else {
        anyhow::bail!("shot: setsid not found on PATH");
    };
    let filepath_str = filepath.to_string_lossy();
    let rec_str = rec.to_string_lossy();
    let status = crate::spawn::spawn_detached(
        &setsid,
        &exe,
        &["--capture", id.as_str(), &filepath_str, &rec_str],
    )
    .context("shot: failed to detach the capture worker")?;
    if !status.success() {
        anyhow::bail!("shot: failed to detach the capture worker");
    }
    Ok(())
}

/// Close the popup and wait for its window to disappear (best-effort, never
/// fails: like the wrapper's `kill … || true` plus its `2>/dev/null … else
/// break`, a missing tool or an already-dead match just ends the wait).
///
/// The `pkill` pattern is the exact-class [`popup::match_pattern`] for
/// [`MENU_CLASS`](popup::MENU_CLASS); the poll keeps the wrapper's 1 s cap
/// (20 × 50 ms).
fn close_popup(path_env: &str) {
    let pattern = popup::match_pattern(popup::MENU_CLASS);
    if let Some(pkill) = resolve_tool("pkill", path_env) {
        let _ = Command::new(pkill)
            .arg("-f")
            .arg(&pattern)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status_retrying();
    }
    let Some(pgrep) = resolve_tool("pgrep", path_env) else {
        return;
    };
    for _ in 0..20_u32 {
        let open = Command::new(&pgrep)
            .arg("-f")
            .arg(&pattern)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output_retrying()
            .is_ok_and(|output| output.status.success());
        if !open {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What [`execute`] staged for the detached worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// Staged output path handed to the worker.
    pub filepath: PathBuf,
}

/// Parent half of the flow: stage the output, detach the worker, close the
/// popup (see the module docs). `path_env` shadows the ambient `PATH` when
/// `Some` (the stub seam); `None` inherits it.
///
/// # Errors
///
/// When the id is unknown, the staging dir cannot be created, the timestamp
/// fails, or the detach fails. Messages carry no `flex:` prefix; the runner
/// reports them.
pub fn execute(action_id: &str, path_env: Option<&str>) -> Result<ExecuteReport> {
    let Some(id) = ShotId::parse(action_id) else {
        anyhow::bail!("shot: unknown id '{action_id}'");
    };
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let dir = save_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("shot: cannot create {}", dir.display()))?;
    let stamp = timestamp(&path_env)?;
    let filepath = filepath_for(id, &dir, &stamp);
    let rec = rec_start()?;
    execute_with(id, &filepath, &rec, Some(&path_env))
}

/// Detach/spawn plus popup-close half of [`execute`], with the staging inputs
/// given (no env reads, no clock): the shape integration tests drive with a
/// stub `PATH`.
///
/// # Errors
///
/// When the detach fails. Messages carry no `flex:` prefix.
pub fn execute_with(
    id: ShotId,
    filepath: &Path,
    rec: &Path,
    path_env: Option<&str>,
) -> Result<ExecuteReport> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    spawn_detached_worker(id, filepath, rec, &path_env)?;
    close_popup(&path_env);
    Ok(ExecuteReport {
        filepath: filepath.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_and_rec() -> (String, String) {
        (String::from("/shots/f.png"), String::from("/rec/start.sh"))
    }

    #[test]
    fn ids_parse_by_bash_case_arm() {
        let cases = [
            ("area-shot", ShotId::AreaShot),
            ("full-shot", ShotId::FullShot),
            ("win-shot", ShotId::WinShot),
            ("area-rec", ShotId::AreaRec),
            ("area-rec-audio", ShotId::AreaRecAudio),
            ("full-rec", ShotId::FullRec),
            ("full-rec-audio", ShotId::FullRecAudio),
        ];
        for (text, expected) in cases {
            assert_eq!(ShotId::parse(text), Some(expected));
            assert_eq!(expected.as_str(), text, "round-trips through as_str");
        }
    }

    #[test]
    fn ids_reject_the_wrapper_bad_id_set() {
        for bad in [
            "",
            "nope",
            "AREA-SHOT",
            "area-shot ",
            "area-shot/",
            "area-shot\n",
            "area-shot../x",
            "noop",
        ] {
            assert_eq!(ShotId::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn recordings_use_mp4_and_shots_png() {
        let dir = Path::new("/shots");
        for id in [
            ShotId::AreaShot,
            ShotId::FullShot,
            ShotId::WinShot,
            ShotId::AreaRec,
            ShotId::AreaRecAudio,
            ShotId::FullRec,
            ShotId::FullRecAudio,
        ] {
            let path = filepath_for(id, dir, "ts");
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let expected = if id.is_recording() {
                "Recording-ts.mp4"
            } else {
                "Screenshot-ts.png"
            };
            assert_eq!(name, expected, "{id:?}");
        }
    }

    #[test]
    fn plan_snapshot_area_shot() {
        let (file, rec) = file_and_rec();
        assert_eq!(
            describe_plan(&plan(ShotId::AreaShot, &file, &rec)),
            vec![
                "slurp",
                "grim -g <slurp> /shots/f.png",
                "wl-copy --type image/png < /shots/f.png",
                "notify-send -h string:image-path:/shots/f.png Screenshot saved /shots/f.png",
            ],
        );
    }

    #[test]
    fn plan_snapshot_full_shot() {
        let (file, rec) = file_and_rec();
        assert_eq!(
            describe_plan(&plan(ShotId::FullShot, &file, &rec)),
            vec![
                "grim /shots/f.png",
                "wl-copy --type image/png < /shots/f.png",
                "notify-send -h string:image-path:/shots/f.png Screenshot saved /shots/f.png",
            ],
        );
    }

    #[test]
    fn plan_snapshot_win_shot() {
        let (file, rec) = file_and_rec();
        assert_eq!(
            describe_plan(&plan(ShotId::WinShot, &file, &rec)),
            vec![
                "hyprctl -j activewindow",
                "grim -g <window> /shots/f.png",
                "wl-copy --type image/png < /shots/f.png",
                "notify-send -h string:image-path:/shots/f.png Screenshot saved /shots/f.png",
            ],
        );
    }

    #[test]
    fn plan_snapshot_recordings() {
        let (file, rec) = file_and_rec();
        assert_eq!(
            describe_plan(&plan(ShotId::AreaRec, &file, &rec)),
            vec!["slurp", "/rec/start.sh -g <slurp> /shots/f.png",],
        );
        assert_eq!(
            describe_plan(&plan(ShotId::AreaRecAudio, &file, &rec)),
            vec!["slurp", "/rec/start.sh -a -g <slurp> /shots/f.png",],
        );
        assert_eq!(
            describe_plan(&plan(ShotId::FullRec, &file, &rec)),
            vec!["/rec/start.sh /shots/f.png"],
        );
        assert_eq!(
            describe_plan(&plan(ShotId::FullRecAudio, &file, &rec)),
            vec!["/rec/start.sh -a /shots/f.png"],
        );
    }

    #[test]
    fn plan_orders_the_region_step_first() {
        let (file, rec) = file_and_rec();
        for id in [ShotId::AreaShot, ShotId::AreaRec, ShotId::AreaRecAudio] {
            let steps = plan(id, &file, &rec);
            assert_eq!(steps.first(), Some(&Step::Slurp), "{id:?}");
        }
        let steps = plan(ShotId::WinShot, &file, &rec);
        assert_eq!(steps.first(), Some(&Step::ActiveWindow));
    }

    #[test]
    fn active_geometry_parses_hyprctl_json() {
        let json = "{\"at\":[11,22],\"size\":[33,44],\"class\":\"kitty\"}";
        assert_eq!(active_geometry(json).as_deref(), Some("11,22 33x44"));
    }

    #[test]
    fn active_geometry_tolerates_whitespace() {
        let json = "{ \"at\" : [ 11 , 22 ] , \"size\" : [ 33 , 44 ] }";
        assert_eq!(active_geometry(json).as_deref(), Some("11,22 33x44"));
    }

    #[test]
    fn active_geometry_rejects_missing_or_broken_pairs() {
        assert_eq!(active_geometry("{}"), None);
        assert_eq!(active_geometry("{\"at\":[11,22]}"), None, "no size");
        assert_eq!(active_geometry("{\"size\":[33,44]}"), None, "no at");
        assert_eq!(active_geometry("{\"at\":[11],\"size\":[33,44]}"), None);
        assert_eq!(active_geometry("not json at all"), None);
        assert_eq!(active_geometry(""), None);
    }

    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn save_dir_prefers_the_override() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved = std::env::var("SCREENSHOT_DIR").ok();
        std::env::set_var("SCREENSHOT_DIR", "/tmp/shots");
        let dir = save_dir().expect("override dir");
        assert_eq!(dir, Path::new("/tmp/shots"));
        match saved {
            Some(value) => std::env::set_var("SCREENSHOT_DIR", value),
            None => std::env::remove_var("SCREENSHOT_DIR"),
        }
    }

    #[test]
    fn rec_start_prefers_the_override() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved = std::env::var("RECORDING_START").ok();
        std::env::set_var("RECORDING_START", "/tmp/rec.sh");
        let rec = rec_start().expect("override rec");
        assert_eq!(rec, Path::new("/tmp/rec.sh"));
        match saved {
            Some(value) => std::env::set_var("RECORDING_START", value),
            None => std::env::remove_var("RECORDING_START"),
        }
    }

    #[test]
    fn rec_start_defaults_to_the_flex_record_binary() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved_rec = std::env::var("RECORDING_START").ok();
        let saved_home = std::env::var("HOME").ok();
        std::env::remove_var("RECORDING_START");
        std::env::set_var("HOME", "/tmp/flex-rec-home");
        let rec = rec_start().expect("default rec");
        assert_eq!(rec, Path::new("/tmp/flex-rec-home/.local/bin/flex-record"));
        match saved_rec {
            Some(value) => std::env::set_var("RECORDING_START", value),
            None => std::env::remove_var("RECORDING_START"),
        }
        match saved_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}
