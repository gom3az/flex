//! Center provider (M5): `control-center.sh` rows for `flex center`.
//!
//! Row-set parity with `scripts/.config/scripts/control-center.sh` (260
//! lines — tab order, labels, metas, and side effects below mirror it;
//! deviations are called out explicitly):
//!
//! - Tabs (in bash `flex_add_tab` order): `Launchers`, `Networks`,
//!   `Bluetooth`, `Power`, `Settings`. All standard spec rows; the
//!   Launchers tab keeps its `Terminal` meta, so unlike `flex launch` it
//!   is NOT bare-rows.
//! - `Launchers`: the same `.desktop` scan as [`launch`](super::launch),
//!   shared via [`launch::scan_dirs`] + [`launch::rows`] (no duplication;
//!   ids gain a `launch:` kind prefix for wrapper dispatch). An empty
//!   scan keeps the bash `(No applications found)` row.
//! - `Networks`: `nmcli` snapshot parse (tolerant). No Wi-Fi interface or
//!   `nmcli` failure → one dim `— offline` row (Q7); interface present but
//!   no networks → `(No Wi-Fi networks)` (bash-exact).
//! - `Bluetooth`: `bluetoothctl devices` snapshot plus a per-device `info`
//!   `Connected: yes` probe. Command missing/failing → dim `— offline`
//!   (bash printed `(bluetoothctl missing)`; the dim offline row is the
//!   Q7 replacement); no devices → `(No paired devices)` (bash-exact).
//! - `Power`: the 5 static bash rows with identical labels/metas.
//!   `Reboot`/`Power Off` are danger rows armed/confirmed by the shared
//!   [`keys`](flex_core::keys) flow — danger logic is never duplicated here.
//! - `Settings`: `Volume` + `Brightness` gauge rows (label-embedded
//!   `pct%` + [`bar`]-style `[████░░░░]` bars, bash-exact formulas) plus
//!   one `Theme: {name}` row per available theme (shared
//!   [`theme_`](super::theme_) scan; the bash `"$meta  $status"` join is
//!   folded into the meta, same as [`theme_`](super::theme_) does).
//!
//! Snapshot rule: every external command runs at most once per
//! load/tick; failures degrade to offline/empty rows, never errors (the
//! multi-command snapshot is the documented exception to the one-spawn
//! guideline — center aggregates five independent sources).
//!
//! Testability: tests never touch the network/`bluetoothd`. The pure
//! `parse_*` functions take captured stdout directly (fixtures live in
//! `tests/fixtures/center/`), and every snapshot honors a `$CENTER_*`
//! file seam (following the `CLIPHIST_FILE` precedent): seam set → read
//! that file; seam set-but-empty → force failure (offline path); seam
//! unset → run the real command once.
//!
//! Tick rule ([`refresh_gauges`], wired into [`Menu::tick`]): the 1 s
//! tick only rewrites the `Volume`/`Brightness` row labels/metas in
//! place — never the row-vec structure, never filter/focus/scroll (R7).
//!
//! The library never executes side effects; the center executor
//! owns the `nmcli`/`bluetoothctl`/`wpctl` calls.

use std::path::Path;

use flex_core::{Menu, Row, RowId, Tab};

use super::{launch, theme_};
use crate::spawn::RetryExec as _;

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "center";
/// Tab titles (bash `flex_add_tab` order).
pub const TAB_LAUNCHERS: &str = "Launchers";
/// Tab titles (bash `flex_add_tab` order).
pub const TAB_NETWORKS: &str = "Networks";
/// Tab titles (bash `flex_add_tab` order).
pub const TAB_BLUETOOTH: &str = "Bluetooth";
/// Tab titles (bash `flex_add_tab` order).
pub const TAB_POWER: &str = "Power";
/// Tab titles (bash `flex_add_tab` order).
pub const TAB_SETTINGS: &str = "Settings";
/// No-op row id: the shared [`crate::providers::NOOP_ID`] (bash `noop) :`
/// arm — the matching wrapper exits 0, no effect).
pub use super::NOOP_ID;
/// Offline placeholder label (Q7 dim offline; see `render::OFFLINE_STATE`).
pub const OFFLINE_LABEL: &str = "— offline";
/// Parenthetical shown when a Wi-Fi scan returns nothing (bash-exact;
/// shared with the [`wifi`](super::wifi) picker).
pub const NO_NETWORKS_LABEL: &str = "(No Wi-Fi networks)";
/// Volume gauge row id (stable across online/offline tick rewrites).
pub const VOL_ID: &str = "vol";
/// Brightness gauge row id (stable across online/offline tick rewrites).
pub const BRIGHT_ID: &str = "bright";
/// Wi-Fi row id (the SSID travels in the escaped label, like theme names
/// in the theme provider — SSIDs may contain spaces, so they cannot be the
/// whitespace-split id token).
pub const WIFI_ID: &str = "wifi";
/// Theme row id (the theme name travels in the `Theme: {name}` label).
pub const THEME_ID: &str = "theme";
/// Snapshot seam for `wpctl get-volume` output (else the command runs).
pub const WPCTL_FILE_ENV: &str = "CENTER_WPCTL_FILE";
/// Snapshot seam for `brightnessctl g` output (else the command runs).
pub const BRIGHT_CUR_FILE_ENV: &str = "CENTER_BRIGHTNESS_CUR_FILE";
/// Snapshot seam for `brightnessctl m` output (else the command runs).
pub const BRIGHT_MAX_FILE_ENV: &str = "CENTER_BRIGHTNESS_MAX_FILE";
/// Snapshot seam for `nmcli … device` output (else the command runs).
pub const NMCLI_DEVICES_FILE_ENV: &str = "CENTER_NMCLI_DEVICES_FILE";
/// Snapshot seam for `nmcli … wifi list` output (else the command runs).
pub const NMCLI_WIFI_FILE_ENV: &str = "CENTER_NMCLI_WIFI_FILE";
/// Snapshot seam for `bluetoothctl devices` output (else it runs).
pub const BT_DEVICES_FILE_ENV: &str = "CENTER_BT_DEVICES_FILE";
/// Snapshot seam directory: `$DIR/{mac}` holds `bluetoothctl info`
/// output per device (else `bluetoothctl info {mac}` runs).
pub const BT_INFO_DIR_ENV: &str = "CENTER_BT_INFO_DIR";

/// Launcher row id: `launch:{row-id}` (kind prefix for dispatch).
///
/// The row id is already the space-free [`launch::entry_id`] hash, so the
/// center executor resolves it back to a desktop-id through the launch
/// library resolver before launching.
#[must_use]
pub fn launch_id(row_id: &str) -> String {
    format!("launch:{row_id}")
}

/// Bluetooth row id: `bt:{mac}` (MACs never contain spaces).
#[must_use]
pub fn bt_id(mac: &str) -> String {
    format!("bt:{mac}")
}

/// Read one snapshot: fixture file when `$file_env` is set (tests), else
/// run `cmd` with `args` once. `None` on any failure — missing command,
/// failing exit, unreadable fixture — so callers degrade to offline or
/// empty rows. A set-but-empty seam forces `None` (offline-path tests).
/// Child stderr is discarded (bash `2>/dev/null` parity).
///
/// Shared with the [`wifi`](super::wifi) provider (same seam contract,
/// different `$WIFI_*` variables), so the rule lives in exactly one place.
pub(crate) fn snapshot(file_env: &str, cmd: &str, args: &[&str]) -> Option<String> {
    if let Ok(path) = std::env::var(file_env) {
        if path.is_empty() {
            return None;
        }
        return std::fs::read_to_string(path).ok();
    }
    let output = std::process::Command::new(cmd)
        .args(args)
        .output_retrying()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

/// `[████░░░░]` gauge primitive — exact port of bash `flex_bar`:
/// `max <= 0` becomes 1, `val` clamps to `[0, max]`, and
/// `filled = (val * width + max / 2) / max` (integer, rounds half up).
/// Saturating arithmetic: absurd magnitudes degrade to a full bar, never
/// panic.
#[must_use]
pub fn bar(val: i64, max: i64, width: usize) -> String {
    let max = max.max(1);
    let clamped = val.clamp(0, max);
    let slots = i64::try_from(width).unwrap_or(i64::MAX);
    let filled = usize::try_from(clamped.saturating_mul(slots).saturating_add(max / 2) / max)
        .unwrap_or(0)
        .min(width);
    let mut out = String::with_capacity(width + 2);
    out.push('[');
    for index in 0..width {
        out.push(if index < filled { '█' } else { '░' });
    }
    out.push(']');
    out
}

// --- Launchers -------------------------------------------------------------

/// Build the `Launchers` tab from pre-scanned entries (tests/replays).
///
/// Standard spec rows (NOT bare like `flex launch` — the bash tab showed
/// the `Terminal` meta). Row construction is shared with
/// [`launch::rows`]; only the id gains the `launch:` kind prefix (the
/// `debug_assert` pins the 1:1 mapping). Empty scans keep the bash
/// `(No applications found)` row.
#[must_use]
pub fn launchers_tab_from_entries(entries: &[launch::DesktopEntry]) -> Tab {
    let mut rows = launch::rows(entries);
    for (row, entry) in rows.iter_mut().zip(entries.iter()) {
        debug_assert_eq!(row.label, entry.name, "launch::rows maps 1:1");
        row.id = RowId::new(launch_id(row.id.as_str()));
    }
    if rows.is_empty() {
        rows.push(super::empty_row(launch::NO_APPS_LABEL));
    }
    let mut tab = Tab::with_rows(TAB_LAUNCHERS, rows);
    tab.bare_rows = false;
    tab
}

/// Build the `Launchers` tab from the real `.desktop` dirs.
#[must_use]
pub fn launchers_tab() -> Tab {
    launchers_tab_from_entries(&launch::scan_dirs(&launch::app_dirs()))
}

// --- Networks ---------------------------------------------------------------

/// One visible Wi-Fi network (`nmcli -t` fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiNet {
    /// SSID (nmcli escapes unescaped, colons included).
    pub ssid: String,
    /// Signal 0–100+ (non-numeric coerces to 0, bash-exact).
    pub signal: i64,
    /// Security string (`--`/empty = open).
    pub security: String,
    /// Whether `IN-USE` is `*`.
    pub connected: bool,
}

/// Parse `nmcli -t -f DEVICE,TYPE device` into the Wi-Fi interface name
/// (`None` = no `*:wifi` line, the offline case). Bash-exact
/// (`awk -F: '$2=="wifi"{print $1; exit}'`).
#[must_use]
pub fn parse_nmcli_devices(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut fields = line.split(':');
        let device = fields.next().filter(|name| !name.is_empty())?;
        (fields.next() == Some("wifi")).then(|| device.to_string())
    })
}

/// Undo nmcli `-t` escaping (`\\` → `\`, `\:` → `:`), bash-exact via the
/// `\x01` placeholder so an escaped backslash before a colon survives.
///
/// Shared with the [`wifi`](super::wifi) provider, which reads profile names
/// out of `nmcli … connection show` (same escaping rules).
pub(crate) fn unescape_ssid(escaped: &str) -> String {
    const PLACEHOLDER: char = '\x01';
    escaped
        .replace("\\\\", &PLACEHOLDER.to_string())
        .replace("\\:", ":")
        .replace(PLACEHOLDER, "\\")
}

/// Parse `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list`.
///
/// Edge-inward split (bash-exact): `IN-USE` is the first field and
/// `SECURITY`/`SIGNAL` the last two (never contain colons), so SSIDs
/// with colons survive. Non-numeric/negative signals become 0; empty
/// SSIDs are skipped.
#[must_use]
pub fn parse_nmcli_wifi(text: &str) -> Vec<WifiNet> {
    let mut nets = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let Some((inuse, after_first)) = line.split_once(':') else {
            continue;
        };
        let Some((middle, security)) = after_first.rsplit_once(':') else {
            continue;
        };
        let Some((ssid_esc, signal_raw)) = middle.rsplit_once(':') else {
            continue;
        };
        let signal: i64 = signal_raw.parse().ok().filter(|val| *val >= 0).unwrap_or(0);
        let ssid = unescape_ssid(ssid_esc);
        if ssid.is_empty() {
            continue;
        }
        nets.push(WifiNet {
            ssid,
            signal,
            security: security.to_string(),
            connected: inuse == "*",
        });
    }
    nets
}

/// Meta body for a Wi-Fi row — bash-exact minus the `select`-renderer
/// prefix: `{signal}% [{8-wide bar}]` `{🔓 Open|🔒 sec}` plus a
/// `  Connected` suffix when active.
///
/// Split out of [`wifi_meta`] because the standalone [`wifi`](super::wifi)
/// picker renders in standard mode (the meta column) where the leading
/// `◇ ` would be meaningless: both surfaces share this body.
#[must_use]
pub fn wifi_meta_body(net: &WifiNet) -> String {
    let lock = if net.security.is_empty() || net.security == "--" {
        "🔓 Open".to_string()
    } else {
        format!("🔒 {}", net.security)
    };
    let mut meta = format!("{}% {} {lock}", net.signal, bar(net.signal, 100, 8));
    if net.connected {
        meta.push_str("  Connected");
    }
    meta
}

/// Meta for a Wi-Fi row — bash-exact: [`wifi_meta_body`] with the
/// `select`-renderer `◇ ` prefix (empty opt leaves the double space).
#[must_use]
pub fn wifi_meta(net: &WifiNet) -> String {
    format!("◇  {}", wifi_meta_body(net))
}

/// Wi-Fi rows from parsed networks (empty scan → bash parenthetical).
#[must_use]
pub fn wifi_rows(nets: &[WifiNet]) -> Vec<Row> {
    if nets.is_empty() {
        return vec![Row::new(RowId::new(NOOP_ID), NO_NETWORKS_LABEL)];
    }
    nets.iter()
        .map(|net| Row::with_meta(RowId::new(WIFI_ID), net.ssid.clone(), wifi_meta(net)))
        .collect()
}

/// One dim `— offline` row (Q7; shared by the offline tabs).
fn offline_row() -> Row {
    Row::offline_placeholder(RowId::new(NOOP_ID), OFFLINE_LABEL)
}

/// `Networks` rows from raw snapshots (`None` = command failed).
fn networks_rows(devices_out: Option<&str>, wifi_out: Option<&str>) -> Vec<Row> {
    let Some(text) = devices_out else {
        return vec![offline_row()];
    };
    if parse_nmcli_devices(text).is_none() {
        return vec![offline_row()];
    }
    wifi_rows(&wifi_out.map_or_else(Vec::new, parse_nmcli_wifi))
}

/// Build the `Networks` tab from raw outputs (`None` = command failed;
/// tests/fixtures feed captured `nmcli` stdout here).
#[must_use]
pub fn networks_tab_from(devices_out: Option<&str>, wifi_out: Option<&str>) -> Tab {
    let mut tab = Tab::with_rows(TAB_NETWORKS, networks_rows(devices_out, wifi_out));
    tab.bare_rows = true;
    tab
}

/// Build the `Networks` tab live (`nmcli`, or fixture seams).
#[must_use]
pub fn networks_tab() -> Tab {
    let devices = snapshot(
        NMCLI_DEVICES_FILE_ENV,
        "nmcli",
        &["-t", "-f", "DEVICE,TYPE", "device"],
    );
    // Skip the list scan when no interface exists (bash `build_networks`
    // returns early, avoiding a pointless failing spawn).
    let wifi = devices
        .as_deref()
        .and_then(parse_nmcli_devices)
        .and_then(|iface| {
            snapshot(
                NMCLI_WIFI_FILE_ENV,
                "nmcli",
                &[
                    "-t",
                    "-f",
                    "IN-USE,SSID,SIGNAL,SECURITY",
                    "device",
                    "wifi",
                    "list",
                    "ifname",
                    &iface,
                ],
            )
        });
    let mut tab = Tab::with_rows(
        TAB_NETWORKS,
        networks_rows(devices.as_deref(), wifi.as_deref()),
    );
    tab.bare_rows = true;
    tab
}

// --- Bluetooth ---------------------------------------------------------------

/// `(mac, name)` pairs from `bluetoothctl devices` output (only
/// `Device …` lines; the name is everything after `Device {mac} `).
#[must_use]
pub fn parse_bt_devices(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("Device ")?;
            let (mac, name) = rest.split_once(' ')?;
            if mac.is_empty() || name.is_empty() {
                return None;
            }
            Some((mac.to_string(), name.to_string()))
        })
        .collect()
}

/// Whether `bluetoothctl info {mac}` output reports `Connected: yes`
/// (bash `grep -q` parity: substring match).
#[must_use]
pub fn bt_connected(info_out: &str) -> bool {
    info_out.contains("Connected: yes")
}

/// Build the `Bluetooth` tab: `devices_out` is the captured `devices`
/// output (`None` = command missing/failed → dim `— offline`), and
/// `info` maps a MAC to its `info` output (`None` = probe failed, shown
/// disconnected — bash `grep -q … && …` treats probe failure the same).
#[must_use]
pub fn bluetooth_tab_from(devices_out: Option<&str>, info: &dyn Fn(&str) -> Option<String>) -> Tab {
    let Some(text) = devices_out else {
        let mut tab = Tab::with_rows(TAB_BLUETOOTH, vec![offline_row()]);
        tab.bare_rows = true;
        return tab;
    };
    let devices = parse_bt_devices(text);
    if devices.is_empty() {
        let mut tab = Tab::with_rows(
            TAB_BLUETOOTH,
            vec![Row::new(RowId::new(NOOP_ID), "(No paired devices)")],
        );
        tab.bare_rows = true;
        return tab;
    }
    let rows = devices
        .iter()
        .map(|(mac, name)| {
            let connected = info(mac).is_some_and(|out| bt_connected(&out));
            let mut meta = mac.clone();
            if connected {
                meta.push_str("  Connected");
            }
            Row::with_meta(RowId::new(bt_id(mac)), name.clone(), meta)
        })
        .collect();
    let mut tab = Tab::with_rows(TAB_BLUETOOTH, rows);
    tab.bare_rows = true;
    tab
}

/// `bluetoothctl info {mac}` snapshot (seam dir or live command).
fn bt_info_snapshot(mac: &str) -> Option<String> {
    if let Ok(dir) = std::env::var(BT_INFO_DIR_ENV) {
        if dir.is_empty() {
            return None;
        }
        return std::fs::read_to_string(Path::new(&dir).join(mac)).ok();
    }
    snapshot(BT_INFO_DIR_ENV, "bluetoothctl", &["info", mac])
}

/// Build the `Bluetooth` tab live (`bluetoothctl`, or fixture seams).
#[must_use]
pub fn bluetooth_tab() -> Tab {
    // A failing `devices` call (missing binary, dead daemon) is the
    // offline case; an empty-but-successful call is `(No paired …)`.
    let devices = snapshot(BT_DEVICES_FILE_ENV, "bluetoothctl", &["devices"]);
    bluetooth_tab_from(devices.as_deref(), &bt_info_snapshot)
}

// --- Power --------------------------------------------------------------------

/// Build the `Power` tab: the 5 static bash rows with identical
/// labels/metas. `Reboot`/`Power Off` are danger rows (armed/confirmed
/// by `keys`; see the danger-timing lock in `tests/keys.rs`). Rows are
/// non-deletable — `Delete` never fires here.
pub const TAB_PROFILES: &str = "Profiles";
pub const TAB_THEMES: &str = "Themes";

#[must_use]
pub fn power_tab() -> Tab {
    let plain = |id: &str, label: &str, meta: &str| Row {
        meta: Some(meta.to_string()),
        ..Row::new(RowId::new(id), label)
    };
    let confirmable = |id: &str, label: &str, meta: &str| Row {
        confirmable: true,
        meta: Some(meta.to_string()),
        ..Row::new(RowId::new(id), label)
    };
    Tab::with_rows(
        TAB_POWER,
        vec![
            plain("pwlock", "Lock Screen", "hyprlock"),
            plain("pwsuspend", "Suspend", "systemctl suspend"),
            confirmable("pwreboot", "Reboot", "systemctl reboot"),
            confirmable("pwoff", "Power Off", "systemctl poweroff"),
            plain("pwlogout", "Logout", "pkill -SIGTERM Hyprland"),
        ],
    )
}

#[must_use]
pub fn profiles_tab_from(active: Option<&str>) -> Tab {
    let plain = |id: &str, label: &str, meta: &str, is_active: bool| {
        let meta_str = if is_active {
            format!("{meta}  Active")
        } else {
            meta.to_string()
        };
        let mut r = Row::new(RowId::new(id), label);
        r.meta = Some(meta_str);
        r.is_default = is_active;
        r
    };
    let mut tab = Tab::with_rows(
        TAB_PROFILES,
        vec![
            plain(
                "pwprofile:performance",
                "Performance Profile",
                "powerprofilesctl set performance",
                active == Some("performance"),
            ),
            plain(
                "pwprofile:balanced",
                "Balanced Profile",
                "powerprofilesctl set balanced",
                active == Some("balanced"),
            ),
            plain(
                "pwprofile:power-saver",
                "Power Saver Profile",
                "powerprofilesctl set power-saver",
                active == Some("power-saver"),
            ),
        ],
    );
    if let Some(act) = active {
        let idx = match act {
            "performance" => Some(0),
            "balanced" => Some(1),
            "power-saver" => Some(2),
            _ => None,
        };
        if let Some(i) = idx {
            tab.state.focus = i;
        }
    }
    tab
}

#[must_use]
pub fn profiles_tab() -> Tab {
    profiles_tab_from(super::profile::active_profile().as_deref())
}

#[must_use]
pub fn themes_rows(themes: &[theme_::ThemeEntry]) -> Vec<Row> {
    themes
        .iter()
        .map(|theme| {
            let meta = if theme.active {
                "Theme  Active"
            } else {
                "Theme"
            };
            Row::with_meta(RowId::new(THEME_ID), format!("Theme: {}", theme.name), meta)
        })
        .collect()
}

#[must_use]
pub fn themes_tab_from(themes: &[theme_::ThemeEntry]) -> Tab {
    let mut tab = Tab::with_rows(TAB_THEMES, themes_rows(themes));
    tab.bare_rows = true;
    tab
}

#[must_use]
pub fn themes_tab() -> Tab {
    let current = theme_::current_name();
    let themes = theme_::scan_available(&theme_::available_dir(), &current);
    themes_tab_from(&themes)
}

// --- Settings (gauges) --------------------------------------------------------

/// `(pct, muted)` from `wpctl get-volume @DEFAULT_AUDIO_SINK@` output
/// (`Volume: 0.55`, muted adds `[MUTED]`). Bash-exact: volume is field 2,
/// `pct` truncates `v * 100` (`awk 'printf %d'`), muted iff the output
/// contains `MUTED`. `None` when unparseable (the row goes `— offline`;
/// bash defaulted to `0%`, the Q7 replacement).
#[must_use]
pub fn parse_wpctl_volume(text: &str) -> Option<(i64, bool)> {
    let vol: f64 = text.split_whitespace().nth(1)?.parse().ok()?;
    if !vol.is_finite() || !(0.0..=1_000_000.0).contains(&vol) {
        return None;
    }
    // Bounded + finite, so the truncating cast is exact.
    #[allow(clippy::cast_possible_truncation)]
    let pct = (vol * 100.0) as i64;
    Some((pct, text.contains("MUTED")))
}

/// `(cur * 100 + max / 2) / max` from `brightnessctl g`/`m` outputs
/// (bash-exact; `max <= 0` becomes 1). `None` on unparseable input.
#[must_use]
pub fn parse_brightness(cur_out: &str, max_out: &str) -> Option<i64> {
    let cur: i64 = cur_out.trim().parse().ok()?;
    let max: i64 = max_out.trim().parse().ok()?;
    let max = max.max(1);
    Some((cur.saturating_mul(100) + max / 2) / max)
}

/// Gauge-row label — bash-exact `gauge`-renderer `left`:
/// `Volume  {pct}%  [{10-wide bar}]` plus `  Muted` (the bash `$status`).
#[must_use]
pub fn volume_label(pct: i64, muted: bool) -> String {
    let mut label = format!("Volume  {pct}%  {}", bar(pct, 100, 10));
    if muted {
        label.push_str("  Muted");
    }
    label
}

/// Gauge-row label — bash-exact: `Brightness  {pct}%  [{10-wide bar}]`
/// (the bash `$status` is always empty here).
#[must_use]
pub fn brightness_label(pct: i64) -> String {
    format!("Brightness  {pct}%  {}", bar(pct, 100, 10))
}

/// Live volume snapshot (`wpctl`, or the fixture seam).
fn volume_snapshot() -> Option<(i64, bool)> {
    snapshot(
        WPCTL_FILE_ENV,
        "wpctl",
        &["get-volume", "@DEFAULT_AUDIO_SINK@"],
    )
    .as_deref()
    .and_then(parse_wpctl_volume)
}

/// Live brightness snapshot (`brightnessctl g` + `m`, or fixture seams).
fn brightness_snapshot() -> Option<i64> {
    let cur = snapshot(BRIGHT_CUR_FILE_ENV, "brightnessctl", &["g"])?;
    let max = snapshot(BRIGHT_MAX_FILE_ENV, "brightnessctl", &["m"])?;
    parse_brightness(&cur, &max)
}

/// Offline gauge placeholder that keeps its slot id so [`refresh_gauges`]
/// can reclaim it in place (row count/order never change on tick).
fn offline_gauge_row(id: &str, label: &str) -> Row {
    Row::offline_placeholder(RowId::new(id), label)
}

/// `Settings` rows: `Volume` + `Brightness` gauge rows (bash labels,
/// `sink`/`backlight` metas) plus any optional `Theme: {name}` rows.
#[must_use]
pub fn settings_rows(
    volume: Option<(i64, bool)>,
    brightness: Option<i64>,
    themes: &[theme_::ThemeEntry],
) -> Vec<Row> {
    let mut rows = Vec::with_capacity(2 + themes.len());
    if let Some((pct, muted)) = volume {
        rows.push(Row::with_meta(
            RowId::new(VOL_ID),
            volume_label(pct, muted),
            "sink",
        ));
    } else {
        rows.push(offline_gauge_row(VOL_ID, "Volume — offline"));
    }
    if let Some(pct) = brightness {
        rows.push(Row::with_meta(
            RowId::new(BRIGHT_ID),
            brightness_label(pct),
            "backlight",
        ));
    } else {
        rows.push(offline_gauge_row(BRIGHT_ID, "Brightness — offline"));
    }
    for theme in themes {
        let meta = if theme.active {
            "Theme  Active"
        } else {
            "Theme"
        };
        rows.push(Row::with_meta(
            RowId::new(THEME_ID),
            format!("Theme: {}", theme.name),
            meta,
        ));
    }
    rows
}

/// Build the `Settings` tab from raw snapshots (`None` = failed command)
/// plus pre-scanned themes (tests/fixtures feed everything here).
///
/// The tab is `deletable` to opt into NAVIGATE `m` → `Toggle` for the
/// volume mute (clip precedent): the wrapper no-ops `DELETE` (center
/// rows are never data) and non-volume `TOGGLE`s.
#[must_use]
pub fn settings_tab_from(
    volume_out: Option<&str>,
    bright_cur: Option<&str>,
    bright_max: Option<&str>,
    themes: &[theme_::ThemeEntry],
) -> Tab {
    let volume = volume_out.and_then(parse_wpctl_volume);
    let brightness = match (bright_cur, bright_max) {
        (Some(cur), Some(max)) => parse_brightness(cur, max),
        _ => None,
    };
    let mut tab = Tab::with_rows(TAB_SETTINGS, settings_rows(volume, brightness, themes));
    tab.deletable = true;
    tab.bare_rows = true;
    tab
}

/// Build the `Settings` tab live (commands/seams).
#[must_use]
pub fn settings_tab() -> Tab {
    let volume = volume_snapshot();
    let brightness = brightness_snapshot();
    let mut tab = Tab::with_rows(TAB_SETTINGS, settings_rows(volume, brightness, &[]));
    tab.deletable = true;
    tab.bare_rows = true;
    tab
}

// --- Menu + tick ---------------------------------------------------------------

/// Build the full 7-tab `center` menu (bash `flex_add_tab` order).
#[must_use]
pub fn center_menu() -> Menu {
    crate::menu(
        PROVIDER,
        vec![
            launchers_tab(),
            networks_tab(),
            bluetooth_tab(),
            power_tab(),
            profiles_tab(),
            themes_tab(),
            settings_tab(),
        ],
    )
}

/// 1 s tick refresh (Q7/R7): re-read volume + brightness and rewrite the
/// `vol`/`bright` row labels/metas/offline flags in place — including
/// offline↔online transitions. Never rebuilds rows (count/order/ids are
/// stable), never touches filter/focus/scroll/marks/armed state, so
/// ticks cannot steal focus. Called from [`Menu::tick`] for `center`
/// menus only.
///
/// Skips the snapshots unless the `Settings` tab is active (B-025): the
/// three spawns cost ~11 ms per idle second and are only visible there,
/// at the price of up to 1 s of staleness when switching to `Settings`.
pub fn refresh_gauges(menu: &mut Menu) {
    let is_settings = menu
        .app
        .active_tab()
        .is_some_and(|tab| tab.name == TAB_SETTINGS);
    if !is_settings {
        return;
    }
    let volume = volume_snapshot();
    let brightness = brightness_snapshot();
    for tab in &mut menu.app.tabs {
        if tab.name != TAB_SETTINGS {
            continue;
        }
        for row in &mut tab.rows {
            if row.id.as_str() == VOL_ID {
                if let Some((pct, muted)) = volume {
                    row.label = volume_label(pct, muted);
                    row.meta = Some("sink".to_string());
                    row.offline = false;
                } else {
                    row.label = "Volume — offline".to_string();
                    row.meta = None;
                    row.offline = true;
                }
            } else if row.id.as_str() == BRIGHT_ID {
                if let Some(pct) = brightness {
                    row.label = brightness_label(pct);
                    row.meta = Some("backlight".to_string());
                    row.offline = false;
                } else {
                    row.label = "Brightness — offline".to_string();
                    row.meta = None;
                    row.offline = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_matches_bash_flex_bar_vectors() {
        // Hand-computed from `flex_bar`: filled=(v*w+max/2)/max.
        assert_eq!(bar(87, 100, 8), "[███████░]");
        assert_eq!(bar(55, 100, 10), "[██████░░░░]");
        assert_eq!(bar(0, 100, 10), "[░░░░░░░░░░]");
        assert_eq!(bar(100, 100, 10), "[██████████]");
        assert_eq!(bar(150, 100, 8), "[████████]", "clamps over-max");
        assert_eq!(bar(-3, 100, 8), "[░░░░░░░░]", "clamps negative");
        assert_eq!(bar(50, 0, 4), "[████]", "max<=0 becomes 1, clamps");
    }

    #[test]
    fn nmcli_colon_ssids_survive_and_signal_coerces() {
        let nets = parse_nmcli_wifi(
            "*:Corp\\:Net:87:WPA2\n :Open Net:12:--\n*:Bad Sig:xx:WPA2\n*::50:WPA2\n*:Skip::WPA2\n",
        );
        assert_eq!(nets.len(), 4, "empty SSID skipped: {nets:?}");
        assert_eq!(nets[0].ssid, "Corp:Net");
        assert_eq!(nets[0].signal, 87);
        assert!(nets[0].connected);
        assert_eq!(nets[1].security, "--");
        assert!(!nets[1].connected);
        assert_eq!(nets[2].ssid, "Bad Sig");
        assert_eq!(nets[2].signal, 0, "non-numeric signal coerces");
        assert_eq!(nets[3].ssid, "Skip");
        assert_eq!(nets[3].signal, 0, "empty signal coerces");
    }

    #[test]
    fn nmcli_escaped_backslash_survives_before_colon() {
        // Wire `C\\X` (two chars) is one literal backslash.
        let nets = parse_nmcli_wifi("*:C\\\\X:50:WPA2\n");
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].ssid, "C\\X");
    }

    #[test]
    fn wpctl_parses_volume_and_mute_flag() {
        assert_eq!(parse_wpctl_volume("Volume: 0.55\n"), Some((55, false)));
        assert_eq!(
            parse_wpctl_volume("Volume: 0.55 [MUTED]\n"),
            Some((55, true))
        );
        assert_eq!(parse_wpctl_volume("Volume: 0"), Some((0, false)));
        assert_eq!(parse_wpctl_volume("Volume: bogus"), None);
        assert_eq!(parse_wpctl_volume(""), None);
    }

    #[test]
    fn brightness_rounds_half_up_and_guards_max() {
        assert_eq!(parse_brightness("1200\n", "2400\n"), Some(50));
        assert_eq!(parse_brightness("1\n", "200\n"), Some(1), "(100+100)/200");
        assert_eq!(parse_brightness("5\n", "0\n"), Some(500), "max 0 → 1");
        assert_eq!(parse_brightness("bogus\n", "100\n"), None);
    }

    #[test]
    fn bt_devices_parse_and_connected_probe() {
        let devices = parse_bt_devices(
            "Device AA:BB:CC:DD:EE:FF Headphones\nnoise\nDevice 11:22:33:44:55:66\n",
        );
        assert_eq!(devices.len(), 1, "nameless line skipped");
        assert_eq!(devices[0].0, "AA:BB:CC:DD:EE:FF");
        assert!(bt_connected("Device AA\n\tConnected: yes\n"));
        assert!(!bt_connected("Device AA\n\tConnected: no\n"));
    }

    #[test]
    fn power_tab_has_two_confirmable_rows() {
        let tab = power_tab();
        assert_eq!(tab.name, TAB_POWER);
        assert!(!tab.deletable);
        let ids: Vec<&str> = tab.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["pwlock", "pwsuspend", "pwreboot", "pwoff", "pwlogout",]
        );
        assert_eq!(tab.rows[2].meta.as_deref(), Some("systemctl reboot"));
        assert!(tab.rows[2].confirmable && tab.rows[3].confirmable);
        assert!(!tab.rows[0].confirmable && !tab.rows[4].confirmable);

        let prof_tab = profiles_tab();
        assert_eq!(prof_tab.name, TAB_PROFILES);
        let prof_ids: Vec<&str> = prof_tab.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            prof_ids,
            vec![
                "pwprofile:performance",
                "pwprofile:balanced",
                "pwprofile:power-saver"
            ]
        );
    }
}
