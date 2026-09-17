//! Wi-Fi picker provider (M8): `flex wifi` — the network dialog.
//!
//! Replaces the rofi picker (`rofi/.config/rofi/scripts/wifi.sh`, which was
//! briefly a wrapper around `nmtui` — a package this host does not have).
//! The row set keeps that dialog's affordances, expressed as flex rows:
//!
//! - Radio row: `Turn Wi-Fi Off` when `nmcli radio wifi` says `enabled`,
//!   else `Turn Wi-Fi On` (ids `off`/`on`), each with its exact command as
//!   the meta (the `power` provider's convention).
//! - `Disconnect from {ssid}` (id `disconnect`) when a scan reports a
//!   network with `IN-USE` `*`. The SSID rides in the escaped label, never
//!   in the whitespace-split id token (SSIDs may contain spaces) — the same
//!   contract as the `center` `Networks` tab.
//! - One row per scanned network (id `wifi`, label = SSID, meta =
//!   [`center::wifi_meta_body`]: `{signal}% [{bar}] {🔓 Open|🔒 sec}` plus
//!   `  Connected`). Unlike the `center` tab these rows render in standard
//!   mode, because the signal/security meta is the whole point of the
//!   picker.
//!
//! Degradation follows the `center` `Networks` tab exactly: a failed
//! `nmcli` call, or success without a `*:wifi` device, gives one dim
//! `— offline` row (Q7); a successful scan that finds nothing gives the
//! bash-exact `(No Wi-Fi networks)` parenthetical.
//!
//! Testability: the pure [`rows`] takes captured stdout, and every live
//! snapshot honors a `$WIFI_*` file seam through the shared
//! [`center::snapshot`] contract (seam set → read that file; seam
//! set-but-empty → force failure; unset → run the real command once).
//! No test touches the network.
//!
//! Latency: a triggered `nmcli` scan blocks ~3 s, so the popup opens from
//! `NetworkManager`'s **cached** scan (`--rescan no`, ~10 ms) and the real scan
//! runs on a background thread; [`refresh_scan`] swaps its rows in on the
//! next tick. See [`menu`].
//!
//! The library never executes side effects; the wifi executor owns every
//! `nmcli`/`notify-send` call, including the password prompt.

use std::sync::Mutex;

use flex_core::{width, Menu, Row, RowId, Tab};

use super::center;

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "wifi";
/// Tab title (the picker shows the one surface it has).
pub const TAB_NAME: &str = "Networks";
/// Action id: turn the radio off (`nmcli radio wifi off`).
pub const OFF_ID: &str = "off";
/// Action id: turn the radio on (`nmcli radio wifi on`).
pub const ON_ID: &str = "on";
/// Action id: drop the active connection (`nmcli device disconnect`).
pub const DISCONNECT_ID: &str = "disconnect";
/// Action id for a network row (label = SSID; shared with the `center`
/// `Networks` tab via [`center::WIFI_ID`]).
pub const CONNECT_ID: &str = center::WIFI_ID;
/// Snapshot seam for `nmcli radio wifi` output (else the command runs).
pub const RADIO_FILE_ENV: &str = "WIFI_RADIO_FILE";
/// Snapshot seam for `nmcli … device` output (else the command runs).
pub const NMCLI_DEVICES_FILE_ENV: &str = "WIFI_NMCLI_DEVICES_FILE";
/// Snapshot seam for `nmcli … wifi list` output (else the command runs).
pub const NMCLI_WIFI_FILE_ENV: &str = "WIFI_NMCLI_WIFI_FILE";
/// Snapshot seam for `nmcli … connection show` output (else it runs).
pub const NMCLI_PROFILES_FILE_ENV: &str = "WIFI_NMCLI_PROFILES_FILE";
/// Marker for a network whose credentials `NetworkManager` already stores.
pub const SAVED_MARKER: &str = "Saved";
/// Marker for the network currently in use.
pub const CONNECTED_MARKER: &str = "Connected";
/// Placeholder label shown while the first background scan is in flight.
pub const SCANNING_LABEL: &str = "Scanning…";
/// Secured-network glyph (`nf-fa-lock`, single cell in a Nerd Font).
pub const LOCK_GLYPH: &str = "\u{f023}";
/// Open-network glyph (`nf-fa-unlock`; the deleted rofi picker's `icon=`).
pub const UNLOCK_GLYPH: &str = "\u{f09c}";

/// One live snapshot: every `nmcli` read the picker is built from.
///
/// `None` means the command failed or is missing (the offline path); the
/// `$WIFI_*` file seams fill the same fields for tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct Snapshot<'a> {
    /// `nmcli radio wifi` output.
    pub radio: Option<&'a str>,
    /// `nmcli -t -f DEVICE,TYPE device` output.
    pub devices: Option<&'a str>,
    /// `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list` output.
    pub wifi: Option<&'a str>,
    /// `nmcli -t -f NAME,TYPE connection show` output: the saved profiles.
    pub profiles: Option<&'a str>,
}

/// Finished scan handed from the background worker to [`refresh_scan`].
///
/// The binary runs exactly one menu per process, so a process-global slot is
/// the mailbox between the worker thread and the 1 s tick.
static PENDING_SCAN: Mutex<Option<crate::spawn::BgTask<Vec<Row>>>> = Mutex::new(None);

/// Whether `nmcli radio wifi` reports the radio on. Anything that is not
/// `enabled` (including `disabled` and unparseable output) is treated as
/// off: the picker then offers to switch it on, which is the only useful
/// action while the radio is down.
#[must_use]
pub fn radio_enabled(out: &str) -> bool {
    out.trim() == "enabled"
}

/// One dim `— offline` row (Q7, shared shape with `center`).
fn offline_row() -> Row {
    Row::offline_placeholder(RowId::new(center::NOOP_ID), center::OFFLINE_LABEL)
}

/// SSIDs of the saved Wi-Fi profiles in `nmcli -t -f NAME,TYPE connection show`
/// output.
///
/// `NetworkManager` names a profile after the SSID it was created for (that is
/// what `nmcli device wifi connect <ssid>` produces), so the name is the SSID
/// for every profile this picker can create. A profile renamed by hand is not
/// matched — the wrapper then falls back to asking, which is what used to
/// happen for *every* secured network.
///
/// Split against the **last** colon: `TYPE` never contains one, while a name
/// with a colon arrives escaped (`My\:Net`), and the escape only adds
/// characters before the separator.
#[must_use]
pub fn parse_saved_profiles(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (name_esc, kind) = line.rsplit_once(':')?;
            (kind == "802-11-wireless" && !name_esc.is_empty())
                .then(|| center::unescape_ssid(name_esc))
        })
        .collect()
}

/// Marker for a network's state column: [`CONNECTED_MARKER`],
/// [`SAVED_MARKER`], or nothing.
///
/// A saved profile means `NetworkManager` holds the credentials, so selecting
/// the row connects without asking for a password — the picker says so
/// instead of letting the prompt come as a surprise.
#[must_use]
pub fn state_marker(net: &center::WifiNet, saved: &[String]) -> &'static str {
    if net.connected {
        CONNECTED_MARKER
    } else if saved.iter().any(|ssid| ssid == &net.ssid) {
        SAVED_MARKER
    } else {
        ""
    }
}

/// Network rows for [`rows`]: one per net, or the bash parenthetical when
/// the scan found nothing. Standard-mode metas, so this is deliberately not
/// [`center::wifi_rows`], which is bash-exact for the `center` tab's bare
/// rows. `saved` is [`parse_saved_profiles`] output.
#[must_use]
pub fn network_rows(nets: &[center::WifiNet], saved: &[String]) -> Vec<Row> {
    if nets.is_empty() {
        return vec![Row::new(
            RowId::new(center::NOOP_ID),
            center::NO_NETWORKS_LABEL,
        )];
    }
    // One width for the whole list, so the security class and the state
    // column line up down the list.
    let security_width = nets
        .iter()
        .map(|net| width::str_width(&security_text(net)))
        .max()
        .unwrap_or_default();
    nets.iter()
        .map(|net| {
            Row::with_meta(
                RowId::new(CONNECT_ID),
                net.ssid.clone(),
                net_meta(net, security_width, state_marker(net, saved)),
            )
        })
        .collect()
}

/// Cell width of the trailing state column (`Connected` is the longest value).
const STATE_WIDTH: usize = 9;
/// Cell width of the bar column (`[` + 8 cells + `]`).
const BAR_WIDTH: usize = 10;

/// Security text for one network: `{lock} {security}`, or `{unlock} Open`.
///
/// The glyphs are Nerd Font private-use icons (`nf-fa-lock`, `nf-fa-unlock`
/// — the ones the deleted rofi picker used), **not** the `🔒`/`🔓` emoji that
/// [`center::wifi_meta_body`] carries: an emoji is drawn by a colour-emoji
/// fallback font, so inside a row of Noto Sans Nerd Font text it lands at
/// emoji metrics (different size and advance) and visibly breaks the meta
/// column. Private-use glyphs are single-cell by construction, like every
/// other icon in these dotfiles.
#[must_use]
pub fn security_text(net: &center::WifiNet) -> String {
    if net.security.is_empty() || net.security == "--" {
        format!("{UNLOCK_GLYPH} Open")
    } else {
        format!("{LOCK_GLYPH} {}", net.security)
    }
}

/// Meta for one network row, laid out as fixed sub-columns so the fields line
/// up down the list:
///
/// ```text
/// 100% [████████]  WPA1 WPA2  Connected
///  79% [██████░░]  WPA2       Saved
///  42% [███░░░░░]  Open
/// ```
///
/// The renderer right-aligns a row's meta as one string, so *equal widths* are
/// what make the columns straight: the signal is padded to three digits, the
/// bar is a fixed [`BAR_WIDTH`], the security class is padded to `security_width`
/// (the widest in the list, so `WPA2` and `WPA1 WPA2` share a column), and the
/// trailing [`STATE_WIDTH`] state column is padded too. Rows without a state
/// therefore end in blank cells rather than shifting every field to the right.
/// `security_width` comes from [`network_rows`].
#[must_use]
pub fn net_meta(net: &center::WifiNet, security_width: usize, state: &str) -> String {
    let meta = format!(
        "{signal:>3}% {bar:<bar_w$}  {security:<security_w$}  {state:>state_w$}",
        signal = net.signal,
        bar = center::bar(net.signal, 100, 8),
        security = security_text(net),
        bar_w = BAR_WIDTH,
        security_w = security_width,
        state_w = STATE_WIDTH,
    );
    debug_assert_eq!(
        width::str_width(&meta),
        4 + 1 + BAR_WIDTH + 2 + security_width + 2 + STATE_WIDTH,
        "every row's meta must be the same width for the columns to line up"
    );
    meta
}

/// Picker rows from a snapshot (`None` fields = command failed/missing).
///
/// Order: the radio action, then `Disconnect from {ssid}` when something is
/// connected, then the scanned networks. Radio off short-circuits to the
/// single `Turn Wi-Fi On` row: a down radio cannot scan, and offering
/// stale networks would be a lie.
#[must_use]
pub fn rows(snapshot: Snapshot<'_>) -> Vec<Row> {
    let Some(radio) = snapshot.radio else {
        return vec![offline_row()];
    };
    if !radio_enabled(radio) {
        return vec![Row::with_meta(
            RowId::new(ON_ID),
            "Turn Wi-Fi On",
            "nmcli radio wifi on",
        )];
    }
    let mut rows = vec![Row::with_meta(
        RowId::new(OFF_ID),
        "Turn Wi-Fi Off",
        "nmcli radio wifi off",
    )];
    let nets = match (snapshot.devices, snapshot.wifi) {
        (Some(devices), Some(list)) if center::parse_nmcli_devices(devices).is_some() => {
            center::parse_nmcli_wifi(list)
        }
        // No Wi-Fi interface, or the list scan failed: same offline row the
        // `center` tab shows, with the radio action still available.
        _ => {
            rows.push(offline_row());
            return rows;
        }
    };
    if let Some(connected) = nets.iter().find(|net| net.connected) {
        rows.push(Row::with_meta(
            RowId::new(DISCONNECT_ID),
            format!("Disconnect from {}", connected.ssid),
            "nmcli device disconnect",
        ));
    }
    let saved = snapshot
        .profiles
        .map_or_else(Vec::new, parse_saved_profiles);
    rows.extend(network_rows(&nets, &saved));
    rows
}

/// Build the picker tab from a snapshot (tests/fixtures feed captured `nmcli`
/// stdout here), exactly as the picker opens it: an empty cache that is being
/// scanned renders the [`SCANNING_LABEL`] placeholder rather than the raw
/// `(No Wi-Fi networks)` parenthetical (see [`initial_rows`]).
///
/// Standard rows (the signal/security meta column is the point), filterable
/// (type-to-filter on an SSID list, like the rofi `-dmenu` it replaces),
/// never deletable (`Delete` is dead here).
#[must_use]
pub fn tab_from(snapshot: Snapshot<'_>) -> Tab {
    picker_tab(initial_rows(snapshot))
}

/// Apply the picker's tab flags to a row set.
fn picker_tab(rows: Vec<Row>) -> Tab {
    let mut tab = Tab::with_rows(TAB_NAME, rows);
    tab.bare_rows = false;
    tab.filterable = true;
    tab.deletable = false;
    tab
}

/// `nmcli … device wifi list` arguments.
///
/// `rescan = false` asks `NetworkManager` for its **cached** scan (measured
/// ~10 ms against ~3 s for a triggered scan on the reference host) — that is
/// what the first frame is built from. `rescan = true` triggers a real scan;
/// it only ever runs on the background worker thread.
fn list_args(iface: &str, rescan: bool) -> Vec<&str> {
    let mut args = vec![
        "-t",
        "-f",
        "IN-USE,SSID,SIGNAL,SECURITY",
        "device",
        "wifi",
        "list",
        "ifname",
        iface,
    ];
    if !rescan {
        args.extend(["--rescan", "no"]);
    }
    args
}

/// Owned form of [`Snapshot`]: what the live path captures before borrowing
/// into the pure row builders.
#[derive(Debug, Default)]
struct OwnedSnapshot {
    radio: Option<String>,
    devices: Option<String>,
    cached: Option<String>,
    profiles: Option<String>,
}

impl OwnedSnapshot {
    /// Borrow as a [`Snapshot`] for [`rows`]/[`initial_rows`].
    fn view(&self) -> Snapshot<'_> {
        Snapshot {
            radio: self.radio.as_deref(),
            devices: self.devices.as_deref(),
            wifi: self.cached.as_deref(),
            profiles: self.profiles.as_deref(),
        }
    }
}

/// The *fast* reads behind the first frame: radio state, devices, the cached
/// scan, and the saved profiles. Never triggers a scan, so it cannot block
/// the UI.
fn live_snapshot() -> OwnedSnapshot {
    let radio = center::snapshot(RADIO_FILE_ENV, "nmcli", &["radio", "wifi"]);
    let devices = center::snapshot(
        NMCLI_DEVICES_FILE_ENV,
        "nmcli",
        &["-t", "-f", "DEVICE,TYPE", "device"],
    );
    // Skip the cached read when the radio is down or no interface exists;
    // both paths already ended in a placeholder row.
    let cached = radio
        .as_deref()
        .filter(|out| radio_enabled(out))
        .and(devices.as_deref())
        .and_then(center::parse_nmcli_devices)
        .and_then(|iface| {
            center::snapshot(NMCLI_WIFI_FILE_ENV, "nmcli", &list_args(&iface, false))
        });
    let profiles = center::snapshot(
        NMCLI_PROFILES_FILE_ENV,
        "nmcli",
        &["-t", "-f", "NAME,TYPE", "connection", "show"],
    );
    OwnedSnapshot {
        radio,
        devices,
        cached,
        profiles,
    }
}

/// Build the picker tab live (`nmcli`, or the `$WIFI_*` fixture seams).
///
/// Synchronous and instant: the same cached-first snapshot [`menu`] opens
/// with. The interactive binary uses [`menu`] so it also gets the
/// background rescan.
#[must_use]
pub fn tab() -> Tab {
    tab_from(live_snapshot().view())
}

/// Build the whole `wifi` menu (one tab) for the interactive binary.
///
/// Opening is **cached-first** because a triggered `nmcli` scan blocks for
/// seconds (measured ~3 s on the reference host) and the popup would sit on a
/// blank terminal until it returned: the radio/device reads plus the cached
/// scan are ~10 ms each, so the first frame is up almost immediately. A
/// background thread then runs the real scan and [`refresh_scan`] swaps its
/// rows in on the next 1 s tick — the list is instant, not stale.
///
/// On a cold cache (no scan in this `NetworkManager` session) the list area
/// says [`SCANNING_LABEL`] instead of claiming there are no networks.
///
/// The background rescan is skipped when a `$WIFI_NMCLI_WIFI_FILE` seam is
/// set: the seam *is* the scan, which keeps fixture runs deterministic and
/// test processes off the radio.
#[must_use]
pub fn menu() -> Menu {
    let snapshot = live_snapshot();
    let tab = tab_from(snapshot.view());
    spawn_rescan(&snapshot);
    crate::menu(PROVIDER, vec![tab])
}

/// First-frame rows: the cached row set, except that an empty/unreadable
/// cache renders as a dim [`SCANNING_LABEL`] placeholder while the background
/// scan is in flight — `(No Wi-Fi networks)` would be a lie at that point.
#[must_use]
pub fn initial_rows(snapshot: Snapshot<'_>) -> Vec<Row> {
    let mut rows = rows(snapshot);
    if !scan_pending(snapshot) {
        return rows;
    }
    // `rows` ends in the empty-cache placeholder (parenthetical or offline
    // row) exactly when the scan has nothing to show yet.
    if let Some(last) = rows.last_mut() {
        *last = scanning_row();
    }
    rows
}

/// Whether the first frame is waiting on the background scan: the radio is
/// on, a Wi-Fi device exists, and the cache holds no networks.
fn scan_pending(snapshot: Snapshot<'_>) -> bool {
    let radio_on = snapshot.radio.is_some_and(radio_enabled);
    let has_device = snapshot
        .devices
        .and_then(center::parse_nmcli_devices)
        .is_some();
    let cached_empty = snapshot
        .wifi
        .is_none_or(|list| center::parse_nmcli_wifi(list).is_empty());
    radio_on && has_device && cached_empty
}

/// Dim `Scanning…` placeholder (noop id: selecting it does nothing).
fn scanning_row() -> Row {
    Row::offline_placeholder(RowId::new(center::NOOP_ID), SCANNING_LABEL)
}

/// Hand a finished scan to the running picker (worker thread → tick).
///
/// The binary runs exactly one menu per process, so a process-global slot is
/// the mailbox; tests fill it directly (no thread, no radio) to exercise
/// [`refresh_scan`].
pub fn store_scan(rows: Vec<Row>) {
    if let Ok(mut slot) = PENDING_SCAN.lock() {
        *slot = Some(crate::spawn::BgTask::ready(rows));
    }
}

/// Tick hook ([`Menu::tick`], `wifi` menus only): swap in a finished scan.
///
/// Focus follows the row the user was on (matched by id + label, so the
/// cursor stays on the same SSID even if the scan reordered the list); when
/// the placeholder is being replaced it lands on the first network row.
/// Filter, marks and scroll are untouched — ticks must never steal state,
/// and a half-typed SSID survives the swap.
pub fn refresh_scan(menu: &mut Menu) {
    let Some(fresh) = PENDING_SCAN.lock().ok().and_then(|mut slot| {
        let task = slot.as_ref()?;
        let res = task.take()?;
        *slot = None;
        Some(res)
    }) else {
        return;
    };
    // Identity of the row under the cursor, read *through the filter*
    // (`TabState::focus` indexes the filtered view, not `Tab::rows`).
    let previous = menu
        .app
        .focused_row()
        .map(|row| (row.id.clone(), row.label.clone()));
    let was_scanning = previous
        .as_ref()
        .is_some_and(|(_, label)| label == SCANNING_LABEL);
    let Some(tab) = menu.app.tabs.iter_mut().find(|tab| tab.name == TAB_NAME) else {
        return;
    };
    tab.rows = fresh;
    let restored = previous
        .as_ref()
        .and_then(|(id, label)| visible_position(menu, |row| row.id == *id && row.label == *label));
    let first_network = was_scanning
        .then(|| visible_position(menu, |row| row.id.as_str() == CONNECT_ID))
        .flatten();
    if let Some(position) = restored.or(first_network) {
        if let Some(state) = menu.app.active_tab_mut().map(|tab| &mut tab.state) {
            state.focus = position;
        }
    }
    menu.app.clamp_focus();
}

/// Position of the first row matching `pred` in the **filtered** view (what
/// `TabState::focus` indexes), or `None`.
fn visible_position(menu: &Menu, pred: impl Fn(&Row) -> bool) -> Option<usize> {
    let rows = &menu.app.active_tab()?.rows;
    menu.app
        .visible_rows()
        .into_iter()
        .position(|index| rows.get(index).is_some_and(&pred))
}

/// Run the real (multi-second) scan off the UI thread and publish its rows.
///
/// Skipped when the list seam is set (the fixture is the scan) or when there
/// is nothing scanable — a down radio or no Wi-Fi device: those paths have no
/// placeholder waiting.
fn spawn_rescan(snapshot: &OwnedSnapshot) {
    if std::env::var(NMCLI_WIFI_FILE_ENV).is_ok() {
        return;
    }
    if !snapshot.radio.as_deref().is_some_and(radio_enabled) {
        return;
    }
    let Some(iface) = snapshot
        .devices
        .as_deref()
        .and_then(center::parse_nmcli_devices)
    else {
        return;
    };
    let radio = snapshot.radio.clone();
    let devices = snapshot.devices.clone();
    let profiles = snapshot.profiles.clone();
    let task = crate::spawn::BgTask::spawn(move || {
        let fresh = center::snapshot(NMCLI_WIFI_FILE_ENV, "nmcli", &list_args(&iface, true));
        rows(Snapshot {
            radio: radio.as_deref(),
            devices: devices.as_deref(),
            wifi: fresh.as_deref(),
            profiles: profiles.as_deref(),
        })
    });
    if let Ok(mut slot) = PENDING_SCAN.lock() {
        *slot = Some(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES: &str = "wlan0:wifi\neth0:ethernet\n";
    const LIST: &str = "*:HomeNet:87:WPA2\n:Coffee Shop:41:--\n";

    /// Snapshot from the three scan inputs (no saved profiles).
    fn snap<'a>(
        radio: Option<&'a str>,
        devices: Option<&'a str>,
        wifi: Option<&'a str>,
    ) -> Snapshot<'a> {
        Snapshot {
            radio,
            devices,
            wifi,
            profiles: None,
        }
    }

    #[test]
    fn radio_off_yields_the_single_turn_on_row() {
        let rows = rows(snap(Some("disabled\n"), Some(DEVICES), Some(LIST)));
        assert_eq!(rows.len(), 1, "a down radio cannot scan: {rows:?}");
        assert_eq!(rows[0].id.as_str(), ON_ID);
        assert_eq!(rows[0].label, "Turn Wi-Fi On");
        assert_eq!(rows[0].meta.as_deref(), Some("nmcli radio wifi on"));
    }

    #[test]
    fn unknown_radio_output_reads_as_off() {
        let rows = rows(snap(Some("garbage\n"), Some(DEVICES), Some(LIST)));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id.as_str(), ON_ID);
    }

    #[test]
    fn enabled_radio_orders_actions_before_networks() {
        let rows = rows(snap(Some("enabled\n"), Some(DEVICES), Some(LIST)));
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec![OFF_ID, DISCONNECT_ID, CONNECT_ID, CONNECT_ID]);
        assert_eq!(rows[0].label, "Turn Wi-Fi Off");
        assert_eq!(rows[1].label, "Disconnect from HomeNet");
        assert_eq!(rows[2].label, "HomeNet");
        assert_eq!(
            rows[2].meta.as_deref().map(str::trim_end),
            Some(" 87% [███████░]  \u{f023} WPA2  Connected")
        );
        assert_eq!(
            rows[3].meta.as_deref().map(str::trim_end),
            Some(" 41% [███░░░░░]  \u{f09c} Open")
        );
        assert_eq!(
            width::str_width(rows[2].meta.as_deref().expect("meta")),
            width::str_width(rows[3].meta.as_deref().expect("meta")),
            "both network rows reserve the same columns"
        );
    }

    #[test]
    fn meta_columns_share_one_geometry() {
        // `WPA1 WPA2` (9 cells with its lock glyph) sets the security column
        // for the whole list; `Open` and the `Connected` marker pad to it.
        let nets = center::parse_nmcli_wifi("*:A:100:WPA2\n:B:49:WPA1 WPA2\n:C:7:--\n");
        let rows = network_rows(&nets, &[]);
        let metas: Vec<String> = rows
            .iter()
            .map(|row| row.meta.clone().expect("meta"))
            .collect();
        let widths: Vec<usize> = metas.iter().map(|meta| width::str_width(meta)).collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] == pair[1]),
            "{widths:?}"
        );
        for (index, meta) in metas.iter().enumerate() {
            let chars: Vec<char> = meta.chars().collect();
            assert_eq!(
                chars.iter().take(4).collect::<String>(),
                match index {
                    0 => "100%",
                    1 => " 49%",
                    _ => "  7%",
                },
                "signal column {index}"
            );
            assert_eq!(chars.get(5), Some(&'['), "bar column {index}");
        }
        // Only the connected row carries the marker, in the reserved column.
        assert!(metas[0].ends_with("Connected"), "{:?}", metas[0]);
        assert!(metas[1].ends_with("         "), "{:?}", metas[1]);
    }

    #[test]
    fn saved_profiles_parse_from_connection_show() {
        let text = "gom3a-5g:802-11-wireless\nlo:loopback\nMy\\:Net:802-11-wireless\n\
Wired:802-3-ethernet\n:802-11-wireless\n";
        assert_eq!(
            parse_saved_profiles(text),
            vec!["gom3a-5g".to_string(), "My:Net".to_string()],
            "wifi profiles only, names unescaped, empty names skipped"
        );
        assert!(parse_saved_profiles("").is_empty());
    }

    #[test]
    fn state_marker_prefers_connected_then_saved() {
        let saved = vec!["HomeNet".to_string(), "Lobby".to_string()];
        let nets = center::parse_nmcli_wifi("*:HomeNet:87:WPA2\n:Lobby:41:WPA2\n:Guest:20:--\n");
        let markers: Vec<&str> = nets.iter().map(|net| state_marker(net, &saved)).collect();
        assert_eq!(markers, vec!["Connected", "Saved", ""]);
    }

    #[test]
    fn saved_networks_are_marked_in_their_own_column() {
        // The marker column keeps its width: `Saved` pads, so the security
        // column does not shift between rows.
        let nets = center::parse_nmcli_wifi(":HomeNet:87:WPA2\n:Lobby:41:WPA3\n");
        let saved = vec!["HomeNet".to_string()];
        let rows = network_rows(&nets, &saved);
        let metas: Vec<&str> = rows
            .iter()
            .map(|row| row.meta.as_deref().expect("meta"))
            .collect();
        assert!(metas[0].ends_with("      Saved"), "{:?}", metas[0]);
        assert_eq!(
            width::str_width(metas[0]),
            width::str_width(metas[1]),
            "the marker must not shift the columns"
        );
    }

    #[test]
    fn profiles_reach_the_rows_through_the_snapshot() {
        let rows = rows(Snapshot {
            radio: Some("enabled\n"),
            devices: Some(DEVICES),
            wifi: Some(LIST),
            profiles: Some("HomeNet:802-11-wireless\nlo:loopback\n"),
        });
        let home = rows
            .iter()
            .find(|row| row.label == "HomeNet")
            .expect("HomeNet row");
        let coffee = rows
            .iter()
            .find(|row| row.label == "Coffee Shop")
            .expect("Coffee Shop row");
        assert!(
            home.meta.as_deref().expect("meta").contains("Connected"),
            "connected wins over saved"
        );
        assert!(
            !coffee.meta.as_deref().expect("meta").contains(SAVED_MARKER),
            "unsaved network carries no marker: {:?}",
            coffee.meta
        );
    }

    #[test]
    fn failed_snapshots_degrade_to_offline() {
        for (radio, devices, wifi) in [
            (None, Some(DEVICES), Some(LIST)),
            (Some("enabled\n"), None, Some(LIST)),
            (Some("enabled\n"), Some(DEVICES), None),
            (Some("enabled\n"), Some("eth0:ethernet\n"), Some(LIST)),
        ] {
            let rows = rows(snap(radio, devices, wifi));
            assert!(
                rows.iter().any(|row| row.offline),
                "offline placeholder for {radio:?}/{devices:?}/{wifi:?}: {rows:?}"
            );
            assert_eq!(rows.last().expect("row").id.as_str(), center::NOOP_ID);
        }
    }

    #[test]
    fn empty_scan_keeps_the_bash_parenthetical() {
        let rows = rows(snap(Some("enabled\n"), Some(DEVICES), Some("")));
        assert_eq!(rows.len(), 2, "radio row + parenthetical: {rows:?}");
        assert_eq!(rows[1].label, center::NO_NETWORKS_LABEL);
        assert_eq!(rows[1].id.as_str(), center::NOOP_ID);
        assert!(rows[1].meta.is_none(), "parenthetical carries no meta");
    }

    #[test]
    fn tab_flags_suit_a_picker() {
        let tab = tab_from(snap(Some("enabled\n"), Some(DEVICES), Some(LIST)));
        assert_eq!(tab.name, TAB_NAME);
        assert!(!tab.bare_rows, "the signal/security meta must render");
        assert!(tab.filterable, "type-to-filter SSIDs");
        assert!(!tab.deletable, "wifi rows are never deletable");
    }

    #[test]
    fn cold_cache_renders_the_scanning_placeholder() {
        // Radio on + device present + nothing cached (unreadable or empty):
        // the first frame must not claim there are no networks.
        for cached in [None, Some("")] {
            let rows = initial_rows(snap(Some("enabled\n"), Some(DEVICES), cached));
            assert_eq!(rows.len(), 2, "radio row + placeholder: {rows:?}");
            assert_eq!(rows[1].label, SCANNING_LABEL);
            assert_eq!(rows[1].id.as_str(), center::NOOP_ID);
            assert!(rows[1].offline, "placeholder is dim");
        }
    }

    #[test]
    fn cached_scan_needs_no_placeholder() {
        let initial = initial_rows(snap(Some("enabled\n"), Some(DEVICES), Some(LIST)));
        assert_eq!(
            initial,
            rows(snap(Some("enabled\n"), Some(DEVICES), Some(LIST))),
            "a populated cache renders the real rows unchanged"
        );
        assert!(initial.iter().all(|row| row.label != SCANNING_LABEL));
    }

    #[test]
    fn placeholder_only_when_a_scan_is_actually_possible() {
        // Radio down: the single `Turn Wi-Fi On` row, no placeholder.
        let down = initial_rows(snap(Some("disabled\n"), Some(DEVICES), None));
        assert_eq!(down.len(), 1);
        assert_eq!(down[0].label, "Turn Wi-Fi On");
        // No Wi-Fi device: the offline row stays (nothing will be scanned).
        let nodev = initial_rows(snap(Some("enabled\n"), Some("eth0:ethernet\n"), None));
        assert!(nodev.last().expect("row").offline);
        assert_eq!(nodev.last().expect("row").label, center::OFFLINE_LABEL);
    }

    #[test]
    fn list_args_only_rescan_when_asked() {
        let cached = list_args("wlan0", false);
        assert_eq!(cached.last(), Some(&"no"));
        assert!(cached.contains(&"--rescan"), "cached read: {cached:?}");
        let fresh = list_args("wlan0", true);
        assert!(
            !fresh.contains(&"--rescan"),
            "fresh read triggers the default scan: {fresh:?}"
        );
        assert_eq!(fresh.last(), Some(&"wlan0"));
    }
}
