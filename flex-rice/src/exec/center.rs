//! `center` executor: the control-center flow (volume/bluetooth/wifi/power/theme).
//!
//! Port of the retired `flex-center.sh` wrapper: after the menu selects a
//! row, the executor handles the select/toggle outcomes (only the Settings
//! tab is deletable, so it can emit `Toggle`), validates the id (non-empty,
//! no `/`, no newline), and dispatches:
//!
//! - `select launch:<hash>` → resolve the hash to a desktop-id in-process
//!   (the same [`resolve_id`](crate::providers::launch::resolve_id) the
//!   library exposes), re-resolve to `(Exec, Terminal)` via
//!   [`find_exec`](crate::providers::launch::find_exec), strip field codes,
//!   detach with `setsid -f` (`<terminal> -e` prefixed for `Terminal=true`
//!   apps, following `$TERMINAL`);
//! - `select wifi` → fresh `nmcli` state (interface discovery, wifi-list
//!   scan matched on the unescaped SSID label): connected → disconnect +
//!   notify, open/unknown → connect (+ notify on success), secure → password
//!   (`FLEX_CENTER_PASSWORD`, else a `/dev/tty` prompt) + connect with
//!   `Connected`/`Failed` notify;
//! - `select`/`toggle bt:<mac>` → fresh `bluetoothctl info` state:
//!   `Connected: yes` → disconnect, else connect;
//! - `select`/`toggle vol` → `wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle`;
//! - `select pwlock|pwsuspend|pwreboot|pwoff|pwlogout` → `hyprlock` /
//!   `systemctl suspend|reboot|poweroff` / `pkill -SIGTERM Hyprland`;
//! - `select theme` → the ported native activator
//!   ([`theme::activate_theme_native`]) with the name stripped from the
//!   `Theme: {name}` label, unless `THEME_SWITCHER` names an override that
//!   runs `<switcher> activate <name>`;
//! - `select bright`, `select noop`, any `delete`, and non-volume/bt
//!   `toggle`s → exit `0` with no effect (brightness adjust is a v1 gap,
//!   exactly like the wrapper).
//!
//! Snapshot convention (the [`exec::shot`](super::shot) template): [`plan`]
//! builds the [`Step`]s from already-decided inputs, [`describe`] renders one
//! step as a single line — unit tests pin the lines, and integration tests
//! diff the stub-`PATH` call logs against the same shapes. Center-specific
//! describe lines:
//!
//! - `wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle`
//! - `bluetoothctl connect <mac>` / `bluetoothctl disconnect <mac>`
//! - `nmcli device disconnect <iface>`
//! - `nmcli device wifi connect <ssid> ifname <iface>`
//! - `nmcli device wifi connect <ssid> password <password> ifname <iface>`
//!   (the password renders verbatim, like clip's entry preview)
//! - `notify-send -a Control Center <Connected|Disconnected|Failed> <ssid>`
//! - `setsid -f [<terminal> -e ]<program> [args…]`
//! - `hyprlock`, `systemctl suspend|reboot|poweroff`, `pkill -SIGTERM Hyprland`
//! - `<switcher> activate <name>` (override) / `<native> activate <name>`
//!
//! Planning reads (`nmcli` discovery/list, `bluetoothctl info`) run inside
//! [`execute`] to decide the [`PlannedAction`] and are therefore not [`Step`]s
//! (unlike `shot`, whose worker resolves geometry mid-plan): `plan` covers the
//! effect + notify sequence, and the integration tests pin the full
//! read-then-act call sequences per action. The conditional notifies
//! (`if connect; then …; else …` verbatim) ride on
//! [`Step::Notify`] + [`NotifyWhen`]: the secure plan lists both the
//! `Connected` and the `Failed` notify, exactly one of which runs.
//!
//! No `ACTION:` unescape step: the wrapper's `center_unescape` undoes the wire
//! escaping of the `ACTION:` line, but the in-process [`Outcome`](flex_core::Outcome)
//! label is already raw, so the SSID/theme name compare identically with no
//! transform on either side.
//!
//! Deliberate departures from the wrapper (all tested):
//!
//! - No subprocess: the hash is resolved in-process with the same
//!   [`resolve_id`](crate::providers::launch::resolve_id) the library
//!   exposes (the launch port's departure, repeated).
//! - No shell word-splitting in the launch arm: the wrapper `eval`s the
//!   stripped `Exec` line; the port splits on whitespace and spawns directly
//!   (the launch port's departure, repeated). Plain `prog --flag` lines — the
//!   reference-data case — are byte-identical.
//! - Exit-code normalisation: the wrapper `exec`s the theme switcher and runs
//!   power tools bare under `set -e`, so their statuses propagate verbatim;
//!   the port maps every loud failure through the shared runner, so any tool
//!   failure exits `1` with the single `flex: error:` prefix. The quiet arms
//!   (`|| true` in the wrapper: mute, bt-toggle, every wifi path) stay quiet
//!   and return `Ok` even when a tool is missing or fails.
//! - The `Terminal=true` branch follows `$TERMINAL` through the shared
//!   [`terminal`](crate::terminal) helper ([`terminal::exec_argv`]) instead
//!   of the wrapper's hardcoded `kitty -e` (`flex-center.sh:87`), so
//!   terminal apps are hosted by the user's configured terminal. An unknown
//!   or empty `$TERMINAL` warns once on stderr and falls back to kitty. The
//!   variable is read only for `Terminal=true` entries, so a plain GUI
//!   launch never touches it (same behaviour as the launch port).
//! - The secure-wifi prompt uses `stty -echo`/`stty echo` subprocesses around
//!   a `/dev/tty` read (the wrapper's `read -rs` builtin disables echo
//!   internally; `unsafe_code = "deny"` forbids `termios`, so the subprocess
//!   is the sanctioned shape, like the planned wifi port). When `stty` is
//!   missing the prompt degrades to a visible read rather than failing; an
//!   abort mid-prompt can leave echo off (recovered with `reset`), because
//!   `panic = "abort"` means no guard runs.
//!
//! Env seams (empty values fall back, like the wrapper's `${VAR:-default}`):
//! `NMCLI`, `BLUETOOTHCTL`, `WPCTL` (tool overrides), `THEME_SWITCHER`
//! (override only; unset/empty runs the native activator), `FLEX_CENTER_PASSWORD`
//! (skips the `/dev/tty` prompt), `HOME` (desktop dirs + native theme dirs).

use std::fs::File;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::exec::theme;
use crate::providers::{center, launch};
use crate::spawn::RetryExec as _;
use crate::terminal::{self, TerminalKind};

/// Which menu outcome is being executed: the wrapper has live `select` and
/// `toggle` arms (toggle only acts on `vol`/`bt:*`, everything else no-ops).
/// `delete` never reaches [`execute`] — the wrapper exits `0` before id
/// validation (`flex-center.sh:42-44`), so the binary returns `Ok` directly —
/// and `target` bails in the binary (no `ACTION:TARGET` arm, and no center
/// row carries dropdown targets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CenterOp {
    /// Row chosen (`select` arm).
    Select,
    /// Mute/pin toggle (`toggle` arm: `vol` mutes, `bt:*` flips).
    Toggle,
}

/// A control-center power row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerKind {
    /// `pwlock` → `hyprlock`.
    Lock,
    /// `pwsuspend` → `systemctl suspend`.
    Suspend,
    /// `pwreboot` → `systemctl reboot`.
    Reboot,
    /// `pwoff` → `systemctl poweroff`.
    Off,
    /// `pwlogout` → `pkill -SIGTERM Hyprland`.
    Logout,
}

impl PowerKind {
    /// Parse a menu action id (exact match, the wrapper's `do_power` arms).
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "pwlock" => Some(Self::Lock),
            "pwsuspend" => Some(Self::Suspend),
            "pwreboot" => Some(Self::Reboot),
            "pwoff" => Some(Self::Off),
            "pwlogout" => Some(Self::Logout),
            _ => None,
        }
    }

    /// Bash arm spelling (also the report detail).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lock => "pwlock",
            Self::Suspend => "pwsuspend",
            Self::Reboot => "pwreboot",
            Self::Off => "pwoff",
            Self::Logout => "pwlogout",
        }
    }
}

/// A validated center action id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CenterAction {
    /// The `noop` empty-scan placeholder: exit `0`, never resolve or act.
    Noop,
    /// A launcher row hash to resolve back to a desktop-id before launching.
    Launch(String),
    /// A Wi-Fi row (the SSID travels in the label, like theme names).
    Wifi,
    /// A bluetooth row (`bt:<mac>`, empty MAC errors like the wrapper).
    Bt(String),
    /// A power row.
    Power(PowerKind),
    /// The volume gauge row (mutes on both select and toggle).
    Vol,
    /// The brightness gauge row (v1 gap: no effect, like the wrapper).
    Bright,
    /// A theme row (the name travels in the `Theme: {name}` label).
    Theme,
    /// Well-formed but unsupported: `select` reports `unknown action` (like
    /// the wrapper's `*)` arm); `toggle` no-ops (the wrapper's `*)` arm).
    Unknown(String),
}

impl CenterAction {
    /// Validate a menu action id, mirroring the wrapper's id check
    /// (non-empty, no `/`, no newline). `None` is the wrapper's `bad id`
    /// exit; `noop` is the empty-scan placeholder.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        if id.is_empty() || id.contains('/') || id.contains('\n') {
            return None;
        }
        if id == "noop" {
            return Some(Self::Noop);
        }
        if let Some(hash) = id.strip_prefix("launch:") {
            return Some(Self::Launch(hash.to_string()));
        }
        if let Some(mac) = id.strip_prefix("bt:") {
            return Some(Self::Bt(mac.to_string()));
        }
        match id {
            "wifi" => Some(Self::Wifi),
            "vol" => Some(Self::Vol),
            "bright" => Some(Self::Bright),
            "theme" => Some(Self::Theme),
            _ => match PowerKind::parse(id) {
                Some(kind) => Some(Self::Power(kind)),
                None => Some(Self::Unknown(id.to_string())),
            },
        }
    }
}

/// When a [`Step::Notify`] runs (the wrapper's `if connect; then …; else …`
/// verbatim: the open-connect plan notifies only on success, the
/// disconnect plan always, and the secure plan lists both the `Connected`
/// (success) and the `Failed` (failure) notifies of which exactly one runs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyWhen {
    /// Always (disconnect + `|| true` arms).
    Always,
    /// Only when the previous tool step exited `0`.
    Success,
    /// Only when the previous tool step failed.
    Failure,
}

/// One center step: a tool spawn (pure data, no process started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle` (quiet, `|| true`).
    WpctlMute {
        /// `WPCTL` program.
        wpctl: String,
    },
    /// `bluetoothctl connect <mac>` (quiet, `|| true`).
    BtConnect {
        /// `BLUETOOTHCTL` program.
        bt: String,
        /// Device MAC.
        mac: String,
    },
    /// `bluetoothctl disconnect <mac>` (quiet, `|| true`).
    BtDisconnect {
        /// `BLUETOOTHCTL` program.
        bt: String,
        /// Device MAC.
        mac: String,
    },
    /// `nmcli device disconnect <iface>` (quiet, `|| true`).
    NmcliDisconnect {
        /// `NMCLI` program.
        nmcli: String,
        /// Wi-Fi interface.
        iface: String,
    },
    /// `nmcli device wifi connect <ssid> ifname <iface>` (gates its notify).
    NmcliConnect {
        /// `NMCLI` program.
        nmcli: String,
        /// Target SSID (one argument, so spaces/colons survive).
        ssid: String,
        /// Wi-Fi interface.
        iface: String,
    },
    /// `nmcli device wifi connect <ssid> password <password> ifname <iface>`
    /// (gates its notifies).
    NmcliConnectSecure {
        /// `NMCLI` program.
        nmcli: String,
        /// Target SSID (one argument).
        ssid: String,
        /// Wi-Fi password (one argument; renders verbatim in [`describe`]).
        password: String,
        /// Wi-Fi interface.
        iface: String,
    },
    /// `notify-send -a Control Center <summary> <body>` (quiet, `|| true`).
    Notify {
        /// `Connected` / `Disconnected` / `Failed` (the wrapper's summaries).
        summary: String,
        /// SSID named in the notification.
        body: String,
        /// Which outcome runs it (see [`NotifyWhen`]).
        when: NotifyWhen,
    },
    /// `setsid -f [<terminal> -e ]<program> [args…]` (loud; the terminal
    /// prefix comes from [`terminal::exec_argv`] for `Terminal=true`).
    Launch {
        /// Terminal decision: `None` spawns directly, `Some` runs the
        /// program inside that terminal.
        terminal: Option<TerminalKind>,
        /// Launched program (first token of the stripped `Exec` line).
        program: String,
        /// Remaining tokens of the stripped `Exec` line.
        args: Vec<String>,
    },
    /// A power tool spawn (loud): `hyprlock`, `systemctl …`, or
    /// `pkill -SIGTERM Hyprland`.
    Power {
        /// Power row.
        kind: PowerKind,
    },
    /// `<switcher> activate <name>` (loud, the wrapper's `exec` line) or the
    /// native activator when no override is set.
    ThemeActivate {
        /// Theme-switcher override program (`None` = native activator).
        switcher: Option<String>,
        /// Resolved theme name (one argument, so spaces survive — B-021).
        name: String,
    },
}

/// A fully-decided center action: [`execute`] performs the planning reads
/// (`nmcli` discovery/list, `bluetoothctl info`, id resolution, password
/// prompt) and [`execute_with`] runs the [`plan`] steps with no further
/// input (the `execute`/`execute_with` split from the pilot template).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    /// Nothing to do (`noop`, `bright`, vanished interface, empty password,
    /// or an inert `toggle`): no steps, exit `0`.
    None,
    /// Volume mute toggle.
    Mute {
        /// `WPCTL` program.
        wpctl: String,
    },
    /// Bluetooth connect/disconnect on fresh `info` state.
    BtToggle {
        /// `BLUETOOTHCTL` program.
        bt: String,
        /// Device MAC.
        mac: String,
        /// Whether `info` reported `Connected: yes` (probe failure counts
        /// as disconnected, like the wrapper's failed `grep`).
        connected: bool,
    },
    /// Disconnect from the current network, then always notify.
    WifiDisconnect {
        /// `NMCLI` program.
        nmcli: String,
        /// Wi-Fi interface.
        iface: String,
        /// SSID named in the notify.
        ssid: String,
    },
    /// Connect to an open (or stale-label) network, notify on success only.
    WifiConnectOpen {
        /// `NMCLI` program.
        nmcli: String,
        /// Target SSID.
        ssid: String,
        /// Wi-Fi interface.
        iface: String,
    },
    /// Connect with a password: `Connected` notify on success, `Failed` on
    /// failure (the wrapper's `if/else` verbatim).
    WifiConnectSecure {
        /// `NMCLI` program.
        nmcli: String,
        /// Target SSID.
        ssid: String,
        /// Wi-Fi password.
        password: String,
        /// Wi-Fi interface.
        iface: String,
    },
    /// Launch a desktop entry (resolved hash → desktop-id → `Exec`).
    Launch {
        /// Resolved desktop-id (report detail + failure messages).
        desktop_id: String,
        /// Terminal decision (`Some` for a `Terminal=true` entry).
        terminal: Option<TerminalKind>,
        /// Launched program.
        program: String,
        /// Remaining `Exec` tokens.
        args: Vec<String>,
    },
    /// A power operation.
    Power(PowerKind),
    /// Theme activation.
    Theme {
        /// Theme-switcher override program (`None` = native activator).
        switcher: Option<String>,
        /// Resolved theme name.
        name: String,
    },
}

/// Build the [`Step`]s for a decided [`PlannedAction`] (no process started).
#[must_use]
pub fn plan(planned: &PlannedAction) -> Vec<Step> {
    match planned {
        PlannedAction::None => Vec::new(),
        PlannedAction::Mute { wpctl } => vec![Step::WpctlMute {
            wpctl: wpctl.clone(),
        }],
        PlannedAction::BtToggle { bt, mac, connected } => {
            if *connected {
                vec![Step::BtDisconnect {
                    bt: bt.clone(),
                    mac: mac.clone(),
                }]
            } else {
                vec![Step::BtConnect {
                    bt: bt.clone(),
                    mac: mac.clone(),
                }]
            }
        }
        PlannedAction::WifiDisconnect { nmcli, iface, ssid } => vec![
            Step::NmcliDisconnect {
                nmcli: nmcli.clone(),
                iface: iface.clone(),
            },
            Step::Notify {
                summary: String::from("Disconnected"),
                body: ssid.clone(),
                when: NotifyWhen::Always,
            },
        ],
        PlannedAction::WifiConnectOpen { nmcli, ssid, iface } => vec![
            Step::NmcliConnect {
                nmcli: nmcli.clone(),
                ssid: ssid.clone(),
                iface: iface.clone(),
            },
            Step::Notify {
                summary: String::from("Connected"),
                body: ssid.clone(),
                when: NotifyWhen::Success,
            },
        ],
        PlannedAction::WifiConnectSecure {
            nmcli,
            ssid,
            password,
            iface,
        } => vec![
            Step::NmcliConnectSecure {
                nmcli: nmcli.clone(),
                ssid: ssid.clone(),
                password: password.clone(),
                iface: iface.clone(),
            },
            Step::Notify {
                summary: String::from("Connected"),
                body: ssid.clone(),
                when: NotifyWhen::Success,
            },
            Step::Notify {
                summary: String::from("Failed"),
                body: ssid.clone(),
                when: NotifyWhen::Failure,
            },
        ],
        PlannedAction::Launch {
            terminal,
            program,
            args,
            ..
        } => vec![Step::Launch {
            terminal: *terminal,
            program: program.clone(),
            args: args.clone(),
        }],
        PlannedAction::Power(kind) => vec![Step::Power { kind: *kind }],
        PlannedAction::Theme { switcher, name } => vec![Step::ThemeActivate {
            switcher: switcher.clone(),
            name: name.clone(),
        }],
    }
}

/// The full command run inside the `setsid` spawn for one [`Step::Launch`]:
/// the terminal argv from [`terminal::exec_argv`] wrapping `program`/`args`
/// when a terminal was selected, else the bare program argv.
fn launch_argv(terminal: Option<TerminalKind>, program: &str, args: &[String]) -> Vec<String> {
    let mut program_argv = Vec::with_capacity(1 + args.len());
    program_argv.push(program.to_string());
    program_argv.extend(args.iter().cloned());
    match terminal {
        Some(kind) => terminal::exec_argv(kind, &program_argv),
        None => program_argv,
    }
}

/// Render one [`Step`] as a single snapshot line: `argv` joined by spaces.
///
/// This is the template snapshot convention: unit tests pin these lines per
/// id (pure, no spawn), and the integration tests diff the stub-`PATH` call
/// logs against the same shapes.
#[must_use]
pub fn describe(step: &Step) -> String {
    match step {
        Step::WpctlMute { wpctl } => format!("{wpctl} set-mute @DEFAULT_AUDIO_SINK@ toggle"),
        Step::BtConnect { bt, mac } => format!("{bt} connect {mac}"),
        Step::BtDisconnect { bt, mac } => format!("{bt} disconnect {mac}"),
        Step::NmcliDisconnect { nmcli, iface } => format!("{nmcli} device disconnect {iface}"),
        Step::NmcliConnect { nmcli, ssid, iface } => {
            format!("{nmcli} device wifi connect {ssid} ifname {iface}")
        }
        Step::NmcliConnectSecure {
            nmcli,
            ssid,
            password,
            iface,
        } => format!("{nmcli} device wifi connect {ssid} password {password} ifname {iface}"),
        Step::Notify { summary, body, .. } => {
            format!("notify-send -a Control Center {summary} {body}")
        }
        Step::Launch {
            terminal,
            program,
            args,
        } => {
            let mut parts = vec![String::from("setsid"), String::from("-f")];
            parts.extend(launch_argv(*terminal, program, args));
            parts.join(" ")
        }
        Step::Power { kind } => match kind {
            PowerKind::Lock => String::from("hyprlock"),
            PowerKind::Suspend => String::from("systemctl suspend"),
            PowerKind::Reboot => String::from("systemctl reboot"),
            PowerKind::Off => String::from("systemctl poweroff"),
            PowerKind::Logout => String::from("pkill -SIGTERM Hyprland"),
        },
        Step::ThemeActivate { switcher, name } => match switcher {
            Some(switcher) => format!("{switcher} activate {name}"),
            None => format!("<native> activate {name}"),
        },
    }
}

/// Render a whole [`plan`] as snapshot lines (see [`describe`]).
#[must_use]
pub fn describe_plan(steps: &[Step]) -> Vec<String> {
    steps.iter().map(describe).collect()
}

/// `NMCLI` override, else `nmcli` (empty values fall back, like the
/// wrapper's `${NMCLI:-nmcli}`).
#[must_use]
pub fn nmcli_cmd() -> String {
    if let Ok(cmd) = std::env::var("NMCLI") {
        if !cmd.is_empty() {
            return cmd;
        }
    }
    String::from("nmcli")
}

/// `BLUETOOTHCTL` override, else `bluetoothctl` (empty values fall back).
#[must_use]
pub fn bluetoothctl_cmd() -> String {
    if let Ok(cmd) = std::env::var("BLUETOOTHCTL") {
        if !cmd.is_empty() {
            return cmd;
        }
    }
    String::from("bluetoothctl")
}

/// `WPCTL` override, else `wpctl` (empty values fall back).
#[must_use]
pub fn wpctl_cmd() -> String {
    if let Ok(cmd) = std::env::var("WPCTL") {
        if !cmd.is_empty() {
            return cmd;
        }
    }
    String::from("wpctl")
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
/// `name` (the usual override shape) resolves to itself, exactly like the
/// wrapper's `"$nmcli_cmd" …` direct invocation.
fn resolve_tool(name: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

/// Run one tool with `args` (loud): stdin nulled, stdout/stderr inherited
/// like the wrapper's bare power/theme invocations (feedback reaches the
/// popup). Returns whether it exited `0`. Messages carry no `flex:` prefix;
/// the runner reports them.
///
/// # Errors
///
/// When the tool is missing from `path_env` or the spawn itself fails.
fn tool(path_env: &str, name: &str, args: &[String]) -> Result<bool> {
    let Some(bin) = resolve_tool(name, path_env) else {
        anyhow::bail!("center: {name} not found on PATH");
    };
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .status_retrying()
        .with_context(|| format!("center: failed to run {name}"))?;
    Ok(status.success())
}

/// Run one tool with `args` (quiet): stdio nulled, failures swallowed —
/// the wrapper's `… 2>/dev/null || true`. Returns whether it exited `0`; a
/// missing tool or failed spawn counts as failure, never an error.
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

/// Capture one tool's stdout (query): stderr nulled, stdin nulled, like the
/// wrapper's `$(… 2>/dev/null || true)` — stdout is captured even on a
/// non-zero exit. `None` only when the tool is missing or the spawn fails.
fn tool_captured(path_env: &str, name: &str, args: &[String]) -> Option<String> {
    let bin = resolve_tool(name, path_env)?;
    let output = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output_retrying()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// First Wi-Fi interface from `nmcli -t -f DEVICE,TYPE device` (the
/// wrapper's `awk -F: '$2=="wifi"{print $1; exit}'`, shared with the
/// provider scan). `None` when the interface vanished since the snapshot.
fn wifi_iface(nmcli: &str, path_env: &str) -> Option<String> {
    let args = vec![
        String::from("-t"),
        String::from("-f"),
        String::from("DEVICE,TYPE"),
        String::from("device"),
    ];
    let out = tool_captured(path_env, nmcli, &args)?;
    center::parse_nmcli_devices(&out)
}

/// `nmcli … wifi list` output for `iface` (`None` on any failure — the
/// caller treats a missing scan as a stale label, i.e. the open arm).
fn wifi_list(nmcli: &str, path_env: &str, iface: &str) -> Option<String> {
    let args = vec![
        String::from("-t"),
        String::from("-f"),
        String::from("IN-USE,SSID,SIGNAL,SECURITY"),
        String::from("device"),
        String::from("wifi"),
        String::from("list"),
        String::from("ifname"),
        iface.to_string(),
    ];
    tool_captured(path_env, nmcli, &args)
}

/// Whether `bluetoothctl info <mac>` reports `Connected: yes` (the
/// wrapper's `… | grep -q 'Connected: yes'`: substring match, and any probe
/// failure counts as disconnected).
fn bt_connected(bt: &str, path_env: &str, mac: &str) -> bool {
    let args = vec![String::from("info"), mac.to_string()];
    tool_captured(path_env, bt, &args).is_some_and(|out| out.contains("Connected: yes"))
}

/// Secure-wifi password: `FLEX_CENTER_PASSWORD` when set and non-empty,
/// else a `/dev/tty` prompt (the wrapper's `read -rsp` verbatim: the prompt
/// goes to stderr, `/dev/tty` missing means an empty answer — there is no
/// stdin fallback in `flex-center.sh`).
///
/// Echo is disabled with `stty -echo` around the read and restored right
/// after (best-effort: a missing `stty` degrades to a visible read rather
/// than failing). A missing tty, failed read, or empty answer yields `""`
/// (the caller exits `0`, like the wrapper).
#[must_use]
pub fn read_password(ssid: &str, path_env: &str) -> String {
    if let Ok(stored) = std::env::var("FLEX_CENTER_PASSWORD") {
        if !stored.is_empty() {
            return stored;
        }
    }
    let Ok(tty) = File::open("/dev/tty") else {
        flex_core::diag::warn("");
        return String::new();
    };
    flex_core::diag::warn_inline(&format!("Password for {ssid}: "));
    set_echo(path_env, false);
    let mut line = String::new();
    let read = BufReader::new(tty).read_line(&mut line);
    set_echo(path_env, true);
    flex_core::diag::warn("");
    if read.is_err() {
        return String::new();
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    line
}

/// `stty -echo` / `stty echo` with stdin on `/dev/tty` (best-effort: every
/// failure is swallowed — the prompt still works, just visibly).
fn set_echo(path_env: &str, on: bool) {
    let Ok(tty) = File::open("/dev/tty") else {
        return;
    };
    let Some(stty) = resolve_tool("stty", path_env) else {
        return;
    };
    let arg = if on { "echo" } else { "-echo" };
    let _ = Command::new(&stty)
        .arg(arg)
        .stdin(tty)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status_retrying();
}

/// Whether any application directory currently holds `desk_id` (tells the
/// wrapper's `not found` apart from its `empty Exec` when the provider
/// parse yields nothing — same helper shape as the launch port).
fn desktop_file_present(desk_id: &str) -> bool {
    launch::app_dirs()
        .iter()
        .any(|dir| dir.join(desk_id).is_file())
}

/// What [`execute`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    /// The outcome that ran (`delete` never reaches [`execute`], so every
    /// report carries its op).
    pub op: CenterOp,
    /// The action id that ran.
    pub action_id: String,
    /// Resolved detail (desktop-id / SSID / MAC / power arm / theme name;
    /// `None` for effect-free rows like `noop`/`bright`).
    pub detail: Option<String>,
}

/// Run the selected center action: validate the id, short-circuit `noop`,
/// resolve hashes in-process, take fresh
/// `nmcli`/`bluetoothctl` state where the wrapper does, and run the
/// [`plan`] steps. `label` is the in-process row label (already raw — no
/// wire unescape, see the module docs). `path_env` shadows the ambient
/// `PATH` when `Some` (the stub seam tests use); `None` inherits it.
///
/// # Errors
///
/// When the id is malformed (`bad id`, like the wrapper), a hash resolves
/// to nothing (`unknown launch id`), the desktop file is gone (`not
/// found`), its `Exec` is empty, the theme name is empty, a loud tool is
/// missing or fails, or the password prompt cannot write. Messages carry no
/// `flex:` prefix; the runner reports them. Quiet arms (mute, bt-toggle,
/// every wifi path, `bright`, `noop`, inert toggles) always succeed.
pub fn execute(
    op: CenterOp,
    action_id: &str,
    label: &str,
    path_env: Option<&str>,
) -> Result<ExecuteReport> {
    let Some(action) = CenterAction::parse(action_id) else {
        anyhow::bail!("center: bad id '{action_id}'");
    };
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    let planned = decide(op, &action, label, &path_env)?;
    execute_with(op, action_id, &planned, Some(&path_env))
}

/// Decide half of [`execute`]: validate, resolve, and take fresh tool state
/// (no effect started). Pure planning reads only.
fn decide(
    op: CenterOp,
    action: &CenterAction,
    label: &str,
    path_env: &str,
) -> Result<PlannedAction> {
    if matches!(action, CenterAction::Noop) {
        return Ok(PlannedAction::None);
    }
    match op {
        CenterOp::Select => match action {
            CenterAction::Launch(hash) => decide_launch(hash),
            CenterAction::Wifi => Ok(decide_wifi(label, path_env)),
            CenterAction::Bt(mac) => decide_bt(mac, path_env),
            CenterAction::Power(kind) => Ok(PlannedAction::Power(*kind)),
            CenterAction::Vol => Ok(PlannedAction::Mute { wpctl: wpctl_cmd() }),
            // No bash gauge-activate; the ±5% adjust is a v1 gap.
            CenterAction::Bright | CenterAction::Noop => Ok(PlannedAction::None),
            CenterAction::Theme => decide_theme(label),
            CenterAction::Unknown(id) => {
                anyhow::bail!("center: unknown action: {id}");
            }
        },
        CenterOp::Toggle => match action {
            CenterAction::Vol => Ok(PlannedAction::Mute { wpctl: wpctl_cmd() }),
            CenterAction::Bt(mac) => decide_bt(mac, path_env),
            // Brightness/theme/launch/wifi/power TOGGLEs: no bash equivalent.
            _ => Ok(PlannedAction::None),
        },
    }
}

/// Resolve a `launch:<hash>` id to its detached-spawn inputs (in-process
/// via the launch library resolver).
fn decide_launch(hash: &str) -> Result<PlannedAction> {
    let Some(desk_id) = launch::resolve_id(hash) else {
        anyhow::bail!("center: unknown launch id: {hash}");
    };
    if desk_id.is_empty() || desk_id.contains('/') || desk_id.contains('\n') {
        anyhow::bail!("center: bad desktop id");
    }
    let Some((exec_raw, terminal)) = launch::find_exec(&desk_id) else {
        if desktop_file_present(&desk_id) {
            anyhow::bail!("center: empty Exec in {desk_id}");
        }
        anyhow::bail!("center: {desk_id} not found");
    };
    let stripped = launch::strip_field_codes(&exec_raw);
    if stripped.is_empty() {
        anyhow::bail!("center: empty Exec in {desk_id}");
    }
    let mut tokens = stripped.split_whitespace().map(str::to_string);
    let Some(program) = tokens.next() else {
        anyhow::bail!("center: empty Exec in {desk_id}");
    };
    // Read `$TERMINAL` only for `Terminal=true` entries: a plain GUI launch
    // must never consult it (and so never warns about a bogus value).
    let terminal = if terminal {
        Some(terminal::detect())
    } else {
        None
    };
    Ok(PlannedAction::Launch {
        desktop_id: desk_id,
        terminal,
        program,
        args: tokens.collect(),
    })
}

/// Decide the wifi flow on fresh `nmcli` state (the wrapper's `do_wifi`
/// verbatim: vanished interface → nothing; connected → disconnect; open or
/// stale label → connect; secure → password then connect).
fn decide_wifi(ssid: &str, path_env: &str) -> PlannedAction {
    let nmcli = nmcli_cmd();
    let Some(iface) = wifi_iface(&nmcli, path_env) else {
        return PlannedAction::None;
    };
    let list = wifi_list(&nmcli, path_env, &iface).unwrap_or_default();
    let found = center::parse_nmcli_wifi(&list)
        .into_iter()
        .find(|net| net.ssid == ssid);
    match found {
        Some(net) if net.connected => PlannedAction::WifiDisconnect {
            nmcli,
            iface,
            ssid: ssid.to_string(),
        },
        Some(net) if net.security.is_empty() || net.security == "--" => {
            PlannedAction::WifiConnectOpen {
                nmcli,
                ssid: ssid.to_string(),
                iface,
            }
        }
        Some(_) => {
            let password = read_password(ssid, path_env);
            if password.is_empty() {
                return PlannedAction::None;
            }
            PlannedAction::WifiConnectSecure {
                nmcli,
                ssid: ssid.to_string(),
                password,
                iface,
            }
        }
        // Unknown (stale label) lands here: treated as open, like an empty
        // `$sec` in bash.
        None => PlannedAction::WifiConnectOpen {
            nmcli,
            ssid: ssid.to_string(),
            iface,
        },
    }
}

/// Decide the bluetooth flow on fresh `info` state (empty MAC errors, like
/// the wrapper's `do_bt_toggle` guard).
fn decide_bt(mac: &str, path_env: &str) -> Result<PlannedAction> {
    if mac.is_empty() {
        anyhow::bail!("center: empty MAC");
    }
    let bt = bluetoothctl_cmd();
    let connected = bt_connected(&bt, path_env, mac);
    Ok(PlannedAction::BtToggle {
        bt,
        mac: mac.to_string(),
        connected,
    })
}

/// Resolve a `theme` row to its activation inputs (the name is stripped
/// from the `Theme: {name}` label; empty errors, like the wrapper). The
/// `THEME_SWITCHER` override is captured as `Some`, else `None` (native).
fn decide_theme(label: &str) -> Result<PlannedAction> {
    let name = label.strip_prefix("Theme: ").unwrap_or(label);
    if name.is_empty() {
        anyhow::bail!("center: empty theme name");
    }
    Ok(PlannedAction::Theme {
        switcher: theme::theme_switcher_override()
            .map(|switcher| switcher.to_string_lossy().into_owned()),
        name: name.to_string(),
    })
}

/// Effect half of [`execute`], with the decided [`PlannedAction`] given (no
/// env reads, no resolve, no prompt): the shape integration tests drive
/// with a stub `PATH` and scratch `HOME`.
///
/// # Errors
///
/// When a loud tool (launch `setsid`, power tools, theme switcher) is
/// missing or exits non-zero (always loud — the quiet arms never fail).
/// Messages carry no `flex:` prefix.
pub fn execute_with(
    op: CenterOp,
    action_id: &str,
    planned: &PlannedAction,
    path_env: Option<&str>,
) -> Result<ExecuteReport> {
    let path_env = path_env.map_or_else(ambient_path, str::to_string);
    // The previous gated-tool outcome: only the `nmcli connect` steps set
    // it from a real exit status (quiet steps never fail, like `|| true`).
    let mut last_ok = true;
    for step in plan(planned) {
        run_step(&path_env, step, &mut last_ok, planned, action_id)?;
    }
    Ok(ExecuteReport {
        op,
        action_id: action_id.to_string(),
        detail: report_detail(planned),
    })
}

/// Run one [`Step`] (the [`execute_with`] interpreter body, split out for
/// the line-count lint). `last_ok` carries the previous gated-tool outcome
/// for [`NotifyWhen`]; `planned`/`action_id` feed the launch failure line.
///
/// # Errors
///
/// When a loud tool (launch `setsid`, power tools, theme switcher) is
/// missing or exits non-zero. Messages carry no `flex:` prefix.
fn run_step(
    path_env: &str,
    step: Step,
    last_ok: &mut bool,
    planned: &PlannedAction,
    action_id: &str,
) -> Result<()> {
    match step {
        Step::WpctlMute { wpctl } => {
            let args = vec![
                String::from("set-mute"),
                String::from("@DEFAULT_AUDIO_SINK@"),
                String::from("toggle"),
            ];
            tool_quiet(path_env, &wpctl, &args);
        }
        Step::BtConnect { bt, mac } => {
            let args = vec![String::from("connect"), mac];
            tool_quiet(path_env, &bt, &args);
        }
        Step::BtDisconnect { bt, mac } => {
            let args = vec![String::from("disconnect"), mac];
            tool_quiet(path_env, &bt, &args);
        }
        Step::NmcliDisconnect { nmcli, iface } => {
            let args = vec![String::from("device"), String::from("disconnect"), iface];
            tool_quiet(path_env, &nmcli, &args);
        }
        Step::NmcliConnect { nmcli, ssid, iface } => {
            let args = vec![
                String::from("device"),
                String::from("wifi"),
                String::from("connect"),
                ssid,
                String::from("ifname"),
                iface,
            ];
            *last_ok = tool_quiet(path_env, &nmcli, &args);
        }
        Step::NmcliConnectSecure {
            nmcli,
            ssid,
            password,
            iface,
        } => {
            let args = vec![
                String::from("device"),
                String::from("wifi"),
                String::from("connect"),
                ssid,
                String::from("password"),
                password,
                String::from("ifname"),
                iface,
            ];
            *last_ok = tool_quiet(path_env, &nmcli, &args);
        }
        Step::Notify {
            summary,
            body,
            when,
        } => {
            let run = match when {
                NotifyWhen::Always => true,
                NotifyWhen::Success => *last_ok,
                NotifyWhen::Failure => !*last_ok,
            };
            if run {
                let args = vec![
                    String::from("-a"),
                    String::from("Control Center"),
                    summary,
                    body,
                ];
                tool_quiet(path_env, "notify-send", &args);
            }
        }
        Step::Launch {
            terminal,
            program,
            args,
        } => {
            run_launch_step(path_env, planned, action_id, terminal, &program, &args)?;
        }
        Step::Power { kind } => {
            let (name, args) = power_argv(kind);
            if !tool(path_env, name, &args)? {
                anyhow::bail!("center: {} failed", describe(&Step::Power { kind }));
            }
        }
        Step::ThemeActivate { switcher, name } => match switcher {
            Some(switcher) => {
                let args = vec![String::from("activate"), name.clone()];
                if !tool(path_env, &switcher, &args)? {
                    anyhow::bail!("center: activate {name} failed");
                }
            }
            None => {
                crate::exec::theme::activate_theme_native(&name, path_env)
                    .map_err(|err| anyhow::anyhow!("center: {err}"))?;
            }
        },
    }
    Ok(())
}

/// Run one [`Step::Launch`]: `setsid -f [<terminal> -e ]…` detached.
///
/// # Errors
///
/// When `setsid` is missing or the launch exits non-zero (always loud).
/// Messages carry no `flex:` prefix.
fn run_launch_step(
    path_env: &str,
    planned: &PlannedAction,
    action_id: &str,
    terminal: Option<TerminalKind>,
    program: &str,
    args: &[String],
) -> Result<()> {
    let mut cmd = vec![String::from("-f")];
    cmd.extend(launch_argv(terminal, program, args));
    if run_detached(path_env, &cmd)? {
        return Ok(());
    }
    let detail = match planned {
        PlannedAction::Launch { desktop_id, .. } => desktop_id.clone(),
        _ => action_id.to_string(),
    };
    anyhow::bail!("center: failed to launch {detail}");
}

/// Report detail for a decided action (the resolved value the row carried).
fn report_detail(planned: &PlannedAction) -> Option<String> {
    match planned {
        PlannedAction::None | PlannedAction::Mute { .. } => None,
        PlannedAction::BtToggle { mac, .. } => Some(mac.clone()),
        PlannedAction::WifiDisconnect { ssid, .. }
        | PlannedAction::WifiConnectOpen { ssid, .. }
        | PlannedAction::WifiConnectSecure { ssid, .. } => Some(ssid.clone()),
        PlannedAction::Launch { desktop_id, .. } => Some(desktop_id.clone()),
        PlannedAction::Power(kind) => Some(kind.as_str().to_string()),
        PlannedAction::Theme { name, .. } => Some(name.clone()),
    }
}

/// `(tool, args)` for a power row (the wrapper's `do_power` arms verbatim).
fn power_argv(kind: PowerKind) -> (&'static str, Vec<String>) {
    match kind {
        PowerKind::Lock => ("hyprlock", Vec::new()),
        PowerKind::Suspend => ("systemctl", vec![String::from("suspend")]),
        PowerKind::Reboot => ("systemctl", vec![String::from("reboot")]),
        PowerKind::Off => ("systemctl", vec![String::from("poweroff")]),
        PowerKind::Logout => (
            "pkill",
            vec![String::from("-SIGTERM"), String::from("Hyprland")],
        ),
    }
}

/// Run `setsid -f …` detached: stdio nulled, like the wrapper's
/// `</dev/null >/dev/null 2>&1` redirections (same helper shape as the
/// launch port; the terminal prefix and the launched program stay arguments,
/// exactly like the wrapper's `eval 'setsid -f [<terminal> -e ]…'` line).
///
/// # Errors
///
/// When `setsid` is missing from `path_env` or the spawn itself fails.
fn run_detached(path_env: &str, args: &[String]) -> Result<bool> {
    let Some(bin) = resolve_tool("setsid", path_env) else {
        anyhow::bail!("center: setsid not found on PATH");
    };
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status_retrying()
        .with_context(|| String::from("center: failed to run setsid"))?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_validate_like_the_wrapper_check() {
        assert_eq!(CenterAction::parse("noop"), Some(CenterAction::Noop));
        assert_eq!(
            CenterAction::parse("launch:0123456789abcdef"),
            Some(CenterAction::Launch(String::from("0123456789abcdef"))),
        );
        assert_eq!(
            CenterAction::parse("bt:AA:BB:CC:DD:EE:FF"),
            Some(CenterAction::Bt(String::from("AA:BB:CC:DD:EE:FF"))),
        );
        assert_eq!(CenterAction::parse("wifi"), Some(CenterAction::Wifi));
        assert_eq!(CenterAction::parse("vol"), Some(CenterAction::Vol));
        assert_eq!(CenterAction::parse("bright"), Some(CenterAction::Bright));
        assert_eq!(CenterAction::parse("theme"), Some(CenterAction::Theme));
        assert_eq!(
            CenterAction::parse("pwreboot"),
            Some(CenterAction::Power(PowerKind::Reboot)),
        );
        // Well-formed but unsupported: parses, `select` rejects later.
        assert_eq!(
            CenterAction::parse("frobnicate"),
            Some(CenterAction::Unknown(String::from("frobnicate"))),
        );
    }

    #[test]
    fn ids_reject_the_wrapper_bad_id_set() {
        for bad in ["", "a/b", "a\nb", "/lead", "trail/", "mid\ndle"] {
            assert_eq!(CenterAction::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn power_ids_parse_by_bash_arm() {
        for (text, kind) in [
            ("pwlock", PowerKind::Lock),
            ("pwsuspend", PowerKind::Suspend),
            ("pwreboot", PowerKind::Reboot),
            ("pwoff", PowerKind::Off),
            ("pwlogout", PowerKind::Logout),
        ] {
            assert_eq!(PowerKind::parse(text), Some(kind));
            assert_eq!(kind.as_str(), text, "round-trips through as_str");
        }
        assert_eq!(PowerKind::parse("pwhalt"), None);
        assert_eq!(PowerKind::parse(""), None);
    }

    #[test]
    fn plan_snapshot_mute_is_one_wpctl_line() {
        assert_eq!(
            describe_plan(&plan(&PlannedAction::Mute {
                wpctl: String::from("wpctl"),
            })),
            vec!["wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"],
        );
    }

    #[test]
    fn plan_snapshot_bt_connects_or_disconnects_on_fresh_state() {
        let mac = String::from("AA:BB:CC:DD:EE:FF");
        assert_eq!(
            describe_plan(&plan(&PlannedAction::BtToggle {
                bt: String::from("bluetoothctl"),
                mac: mac.clone(),
                connected: true,
            })),
            vec!["bluetoothctl disconnect AA:BB:CC:DD:EE:FF"],
        );
        assert_eq!(
            describe_plan(&plan(&PlannedAction::BtToggle {
                bt: String::from("bluetoothctl"),
                mac,
                connected: false,
            })),
            vec!["bluetoothctl connect AA:BB:CC:DD:EE:FF"],
        );
    }

    #[test]
    fn plan_snapshot_wifi_disconnect_always_notifies() {
        assert_eq!(
            describe_plan(&plan(&PlannedAction::WifiDisconnect {
                nmcli: String::from("nmcli"),
                iface: String::from("wlan0"),
                ssid: String::from("HomeNet"),
            })),
            vec![
                "nmcli device disconnect wlan0",
                "notify-send -a Control Center Disconnected HomeNet",
            ],
        );
    }

    #[test]
    fn plan_snapshot_wifi_open_lists_connect_plus_success_notify() {
        assert_eq!(
            describe_plan(&plan(&PlannedAction::WifiConnectOpen {
                nmcli: String::from("nmcli"),
                ssid: String::from("Coffee Shop"),
                iface: String::from("wlan0"),
            })),
            vec![
                "nmcli device wifi connect Coffee Shop ifname wlan0",
                "notify-send -a Control Center Connected Coffee Shop",
            ],
        );
    }

    #[test]
    fn plan_snapshot_wifi_secure_lists_both_notifies() {
        // Exactly one runs (the wrapper's `if/else` via `NotifyWhen`).
        assert_eq!(
            describe_plan(&plan(&PlannedAction::WifiConnectSecure {
                nmcli: String::from("nmcli"),
                ssid: String::from("MyNet"),
                password: String::from("s3cret"),
                iface: String::from("wlan0"),
            })),
            vec![
                "nmcli device wifi connect MyNet password s3cret ifname wlan0",
                "notify-send -a Control Center Connected MyNet",
                "notify-send -a Control Center Failed MyNet",
            ],
        );
    }

    #[test]
    fn plan_snapshot_launch_uses_the_terminal_decision() {
        assert_eq!(
            describe_plan(&plan(&PlannedAction::Launch {
                desktop_id: String::from("firefox.desktop"),
                terminal: None,
                program: String::from("firefox"),
                args: Vec::new(),
            })),
            vec!["setsid -f firefox"],
        );
        assert_eq!(
            describe_plan(&plan(&PlannedAction::Launch {
                desktop_id: String::from("termapp.desktop"),
                terminal: Some(TerminalKind::Kitty),
                program: String::from("htop"),
                args: Vec::new(),
            })),
            vec!["setsid -f kitty -e htop"],
        );
    }

    #[test]
    fn plan_snapshot_power_arms() {
        for (kind, line) in [
            (PowerKind::Lock, "hyprlock"),
            (PowerKind::Suspend, "systemctl suspend"),
            (PowerKind::Reboot, "systemctl reboot"),
            (PowerKind::Off, "systemctl poweroff"),
            (PowerKind::Logout, "pkill -SIGTERM Hyprland"),
        ] {
            assert_eq!(
                describe_plan(&plan(&PlannedAction::Power(kind))),
                vec![line.to_string()],
                "{kind:?}",
            );
        }
    }

    #[test]
    fn plan_snapshot_theme_is_the_switcher_activate_line() {
        assert_eq!(
            describe_plan(&plan(&PlannedAction::Theme {
                switcher: Some(String::from("/sw/theme-switcher.sh")),
                name: String::from("tokyo-night"),
            })),
            vec!["/sw/theme-switcher.sh activate tokyo-night"],
        );
        assert_eq!(
            describe_plan(&plan(&PlannedAction::Theme {
                switcher: None,
                name: String::from("tokyo-night"),
            })),
            vec!["<native> activate tokyo-night"],
        );
    }

    #[test]
    fn plan_snapshot_none_is_empty() {
        assert!(plan(&PlannedAction::None).is_empty());
    }

    #[test]
    fn tool_cmds_prefer_overrides_and_fall_back_empty() {
        let saved_nmcli = std::env::var("NMCLI").ok();
        let saved_bt = std::env::var("BLUETOOTHCTL").ok();
        let saved_wpctl = std::env::var("WPCTL").ok();
        std::env::set_var("NMCLI", "/tmp/stub-nmcli");
        std::env::set_var("BLUETOOTHCTL", "");
        std::env::remove_var("WPCTL");
        assert_eq!(nmcli_cmd(), "/tmp/stub-nmcli");
        assert_eq!(bluetoothctl_cmd(), "bluetoothctl");
        assert_eq!(wpctl_cmd(), "wpctl");
        match saved_nmcli {
            Some(value) => std::env::set_var("NMCLI", value),
            None => std::env::remove_var("NMCLI"),
        }
        match saved_bt {
            Some(value) => std::env::set_var("BLUETOOTHCTL", value),
            None => std::env::remove_var("BLUETOOTHCTL"),
        }
        match saved_wpctl {
            Some(value) => std::env::set_var("WPCTL", value),
            None => std::env::remove_var("WPCTL"),
        }
    }

    #[test]
    fn theme_switcher_override_empty_means_native() {
        let saved_switcher = std::env::var("THEME_SWITCHER").ok();
        let saved_home = std::env::var("HOME").ok();
        std::env::set_var("THEME_SWITCHER", "/tmp/stub-switcher.sh");
        assert_eq!(
            theme::theme_switcher_override().as_deref(),
            Some(Path::new("/tmp/stub-switcher.sh"))
        );
        std::env::set_var("THEME_SWITCHER", "");
        std::env::set_var("HOME", "/tmp/fake-home");
        assert_eq!(theme::theme_switcher_override(), None);
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
