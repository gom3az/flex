//! `flex wifi` cutover tests (M8): provider fixtures, key-seq replays
//! (filter/navigate/cancel), the 80x24 golden, the live `$WIFI_*` seam
//! path, and stubbed-pipeline wrapper dispatch.
//!
//! Reference-data fixtures live in `tests/fixtures/wifi/` (captured
//! `nmcli` stdout); expected labels/metas are hand-computed from the
//! shared `center` row builders. No test touches the network.

use std::io::Cursor;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::{run, Menu};
use flex_rice::exec::wifi as exec_wifi;
use flex_rice::providers::{center, wifi};

/// Serializes every test that touches process env or spawns the wrapper:
/// the `WIFI_*` seam readers, the wrapper-dispatch tests, and the executor
/// tests below. The executor tests set process-wide tool seams (`NMCLI`,
/// `NOTIFY_SEND`, `FLEX_WIFI_PASSWORD`, `STUB_LOG`, …) that wrapper
/// children would otherwise inherit through the ambient environment, so
/// both sides must hold this lock — per-child `Command::env` alone no
/// longer suffices once process env is mutated.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Serializes the tests that use the process-global scan mailbox
/// (`wifi::store_scan` / `wifi::refresh_scan`): cargo runs tests in threads,
/// and one picker per process is the production assumption.
static SCAN_LOCK: Mutex<()> = Mutex::new(());

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wifi")
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(fixtures_dir().join(name)).expect("read fixture")
}

/// Snapshot from the scan fixtures (`profiles` = saved-profile output).
fn snap<'a>(
    radio: Option<&'a str>,
    devices: Option<&'a str>,
    wifi: Option<&'a str>,
) -> wifi::Snapshot<'a> {
    wifi::Snapshot {
        radio,
        devices,
        wifi,
        profiles: None,
    }
}

/// Snapshot with saved profiles attached.
fn snap_saved<'a>(
    radio: Option<&'a str>,
    devices: Option<&'a str>,
    wifi: Option<&'a str>,
    profiles: &'a str,
) -> wifi::Snapshot<'a> {
    wifi::Snapshot {
        profiles: Some(profiles),
        ..snap(radio, devices, wifi)
    }
}

fn fixture_tab(list: &str) -> flex_core::Tab {
    wifi::tab_from(snap(
        Some(&fixture("radio-enabled.txt")),
        Some(&fixture("nmcli-devices.txt")),
        Some(list),
    ))
}

/// Same, with the saved-profile fixture attached (marker column).
fn fixture_tab_saved(list: &str) -> flex_core::Tab {
    wifi::tab_from(snap_saved(
        Some(&fixture("radio-enabled.txt")),
        Some(&fixture("nmcli-devices.txt")),
        Some(list),
        &fixture("nmcli-profiles.txt"),
    ))
}

/// Menu built from `nmcli-wifi.txt` (`HomeNet` connected, Coffee Shop open).
fn fixture_menu() -> Menu {
    flex_rice::menu(
        wifi::PROVIDER,
        vec![fixture_tab(&fixture("nmcli-wifi.txt"))],
    )
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn rune(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

// --- Provider rows ----------------------------------------------------------

#[test]
fn row_set_matches_the_rofi_picker_affordances() {
    let tab = fixture_tab(&fixture("nmcli-wifi.txt"));
    assert_eq!(tab.name, wifi::TAB_NAME);
    assert!(!tab.bare_rows, "signal/security metas must render");
    assert!(tab.filterable);
    assert!(!tab.deletable);
    // Network metas are column-aligned and padded to a common width (the
    // `Connected` column reserves its cells), so compare them trimmed.
    let rows: Vec<(&str, &str, Option<&str>)> = tab
        .rows
        .iter()
        .map(|row| {
            (
                row.id.as_str(),
                row.label.as_str(),
                row.meta.as_deref().map(str::trim_end),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("off", "Turn Wi-Fi Off", Some("nmcli radio wifi off")),
            (
                "disconnect",
                "Disconnect from HomeNet",
                Some("nmcli device disconnect")
            ),
            (
                "wifi",
                "HomeNet",
                Some(" 87% [███████░]  \u{f023} WPA2  Connected")
            ),
            ("wifi", "Corp:Net", Some(" 63% [█████░░░]  \u{f023} WPA2")),
            (
                "wifi",
                "Coffee Shop",
                Some(" 41% [███░░░░░]  \u{f09c} Open")
            ),
            ("wifi", "Lobby", Some("  5% [░░░░░░░░]  \u{f023} WPA3")),
        ]
    );
}

/// Every network meta is the same width, so the signal/bar/security/
/// `Connected` fields line up down the list (the renderer right-aligns one
/// string per row, so equal widths are what make the columns straight).
#[test]
fn network_metas_are_padded_to_one_column_geometry() {
    let tab = fixture_tab(&fixture("nmcli-wifi.txt"));
    let widths: Vec<usize> = tab
        .rows
        .iter()
        .filter(|row| row.id.as_str() == "wifi")
        .map(|row| flex_core::width::str_width(row.meta.as_deref().expect("meta")))
        .collect();
    assert!(widths.len() >= 3, "fixture has several networks");
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "all network metas share one width: {widths:?}"
    );
    // The `%` of the signal sits at one column on every row: `NNN%` = 4 cells.
    let signals: Vec<String> = tab
        .rows
        .iter()
        .filter(|row| row.id.as_str() == "wifi")
        .filter_map(|row| row.meta.as_deref())
        .map(|meta| meta.chars().take(4).collect())
        .collect();
    assert_eq!(
        signals,
        vec![
            " 87%".to_string(),
            " 63%".to_string(),
            " 41%".to_string(),
            "  5%".to_string()
        ],
        "the signal column is right-aligned to three digits"
    );
    // `Connected` occupies its own column: same position on the connected row,
    // blank cells elsewhere.
    let connected_row = tab
        .rows
        .iter()
        .find(|row| row.label == "HomeNet")
        .expect("connected row");
    let meta = connected_row.meta.as_deref().expect("meta");
    assert!(meta.trim_end().ends_with("Connected"));
    assert!(
        !meta.trim_end().ends_with("WPA2"),
        "the marker is a separate column, not glued to the security class"
    );
}

#[test]
fn disconnected_scan_has_no_disconnect_row() {
    let tab = fixture_tab(&fixture("nmcli-wifi-disconnected.txt"));
    let ids: Vec<&str> = tab.rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, vec!["off", "wifi", "wifi", "wifi"]);
    assert!(
        tab.rows.iter().all(|row| !row
            .meta
            .as_deref()
            .unwrap_or_default()
            .contains("Connected")),
        "no Connected suffix without an IN-USE row"
    );
}

#[test]
fn radio_disabled_offers_only_turn_on() {
    let tab = wifi::tab_from(snap(
        Some(&fixture("radio-disabled.txt")),
        Some(&fixture("nmcli-devices.txt")),
        Some(&fixture("nmcli-wifi.txt")),
    ));
    assert_eq!(
        tab.rows.len(),
        1,
        "a down radio cannot scan: {:?}",
        tab.rows
    );
    assert_eq!(tab.rows[0].id.as_str(), "on");
    assert_eq!(tab.rows[0].label, "Turn Wi-Fi On");
}

#[test]
fn missing_command_degrades_to_one_offline_row() {
    let tab = wifi::tab_from(snap(None, None, None));
    assert_eq!(tab.rows.len(), 1);
    assert!(tab.rows[0].offline, "dim offline placeholder");
    assert_eq!(tab.rows[0].id.as_str(), center::NOOP_ID);
}

#[test]
fn enabled_radio_without_wifi_device_keeps_the_off_action() {
    let tab = wifi::tab_from(snap(
        Some(&fixture("radio-enabled.txt")),
        Some(&fixture("nmcli-devices-nowifi.txt")),
        Some(&fixture("nmcli-wifi.txt")),
    ));
    let ids: Vec<&str> = tab.rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, vec!["off", center::NOOP_ID]);
    assert!(tab.rows[1].offline);
}

#[test]
fn empty_cache_shows_scanning_not_the_parenthetical() {
    // The raw row set keeps the bash parenthetical (the `center` contract)…
    let raw = wifi::rows(snap(
        Some(&fixture("radio-enabled.txt")),
        Some(&fixture("nmcli-devices.txt")),
        Some(""),
    ));
    assert_eq!(raw[1].label, center::NO_NETWORKS_LABEL);
    // …but a picker that is still scanning must not claim there are none.
    let tab = fixture_tab("");
    let ids: Vec<&str> = tab.rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, vec!["off", center::NOOP_ID]);
    assert_eq!(tab.rows[1].label, wifi::SCANNING_LABEL);
    assert!(tab.rows[1].offline, "placeholder is dim");
}

#[test]
fn menu_is_the_wifi_provider_with_one_tab() {
    let menu = fixture_menu();
    assert_eq!(menu.provider, "wifi");
    assert_eq!(menu.app.tabs.len(), 1);
    assert_eq!(
        menu.app.active_tab().expect("tab").name,
        wifi::TAB_NAME,
        "single-tab menu still names its surface"
    );
}

// --- Live seam path ---------------------------------------------------------

/// Process-env guard: restores overwritten vars on drop so parallel suites
/// never observe leaked `WIFI_*` seams.
struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn set(pairs: &[(&str, &str)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect();
        for (key, value) in pairs {
            std::env::set_var(key, value);
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn live_tab_reads_every_seam() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = fixtures_dir();
    let devices = dir.join("nmcli-devices.txt");
    let list = dir.join("nmcli-wifi.txt");
    let radio = dir.join("radio-enabled.txt");
    let profiles = dir.join("nmcli-profiles.txt");
    let _guard = EnvGuard::set(&[
        (
            wifi::NMCLI_DEVICES_FILE_ENV,
            devices.to_str().expect("utf-8 path"),
        ),
        (
            wifi::NMCLI_WIFI_FILE_ENV,
            list.to_str().expect("utf-8 path"),
        ),
        (wifi::RADIO_FILE_ENV, radio.to_str().expect("utf-8 path")),
        (
            wifi::NMCLI_PROFILES_FILE_ENV,
            profiles.to_str().expect("utf-8 path"),
        ),
    ]);
    let live = wifi::tab();
    let expected = fixture_tab_saved(&fixture("nmcli-wifi.txt"));
    let live_rows: Vec<(&str, &str, Option<&str>)> = live
        .rows
        .iter()
        .map(|row| {
            (
                row.id.as_str(),
                row.label.as_str(),
                row.meta.as_deref().map(str::trim_end),
            )
        })
        .collect();
    let expected_rows: Vec<(&str, &str, Option<&str>)> = expected
        .rows
        .iter()
        .map(|row| {
            (
                row.id.as_str(),
                row.label.as_str(),
                row.meta.as_deref().map(str::trim_end),
            )
        })
        .collect();
    assert_eq!(live_rows, expected_rows);
}

/// The saved profiles reach the rows as their own column: a network the scan
/// matches shows `Saved`, the connected one keeps `Connected`, and the rest
/// stay blank.
#[test]
fn saved_profiles_mark_the_rows() {
    let tab = fixture_tab_saved(&fixture("nmcli-wifi.txt"));
    let state_of = |label: &str| -> String {
        let meta = tab
            .rows
            .iter()
            .find(|row| row.label == label)
            .and_then(|row| row.meta.as_deref())
            .unwrap_or_else(|| panic!("row {label}"));
        // The state column is the last 9 cells of the padded meta.
        let chars: Vec<char> = meta.chars().collect();
        chars[chars.len() - 9..].iter().collect::<String>()
    };
    assert_eq!(state_of("HomeNet").trim(), "Connected");
    assert_eq!(state_of("Coffee Shop").trim(), "Saved");
    assert_eq!(state_of("Corp:Net").trim(), "", "unsaved stays blank");
    assert_eq!(state_of("Lobby").trim(), "", "unsaved stays blank");
}

#[test]
fn live_tab_skips_the_scan_when_the_radio_is_down() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = fixtures_dir();
    // A stale list seam must not be read while the radio is off.
    let _guard = EnvGuard::set(&[
        (
            wifi::RADIO_FILE_ENV,
            dir.join("radio-disabled.txt").to_str().expect("utf-8 path"),
        ),
        (
            wifi::NMCLI_DEVICES_FILE_ENV,
            dir.join("nmcli-devices.txt").to_str().expect("utf-8 path"),
        ),
        (
            wifi::NMCLI_WIFI_FILE_ENV,
            dir.join("nmcli-wifi.txt").to_str().expect("utf-8 path"),
        ),
    ]);
    let tab = wifi::tab();
    assert_eq!(tab.rows.len(), 1);
    assert_eq!(tab.rows[0].id.as_str(), "on");
}

// --- Instant open + background rescan ---------------------------------------

const DEVICES: &str = "wlan0:wifi\neth0:ethernet\n";

#[test]
fn menu_opens_from_the_cached_scan_and_shows_no_placeholder() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = fixtures_dir();
    // Seams set ⇒ no worker thread (the fixture *is* the scan), so this is
    // deterministic and never touches the radio.
    let _guard = EnvGuard::set(&[
        (
            wifi::RADIO_FILE_ENV,
            dir.join("radio-enabled.txt").to_str().expect("utf-8 path"),
        ),
        (
            wifi::NMCLI_DEVICES_FILE_ENV,
            dir.join("nmcli-devices.txt").to_str().expect("utf-8 path"),
        ),
        (
            wifi::NMCLI_WIFI_FILE_ENV,
            dir.join("nmcli-wifi.txt").to_str().expect("utf-8 path"),
        ),
    ]);
    let menu = wifi::menu();
    assert_eq!(menu.provider, wifi::PROVIDER);
    assert_eq!(menu.app.tabs.len(), 1);
    let rows: Vec<(&str, &str)> = menu.app.tabs[0]
        .rows
        .iter()
        .map(|row| (row.id.as_str(), row.label.as_str()))
        .collect();
    let expected_tab = fixture_tab(&fixture("nmcli-wifi.txt"));
    let expected: Vec<(&str, &str)> = expected_tab
        .rows
        .iter()
        .map(|row| (row.id.as_str(), row.label.as_str()))
        .collect();
    assert_eq!(rows, expected, "the cached scan fills the first frame");
    assert!(
        !rows.iter().any(|(_, label)| *label == wifi::SCANNING_LABEL),
        "a populated cache shows results, not a placeholder"
    );
}

#[test]
fn cold_cache_opens_on_the_scanning_placeholder() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = fixtures_dir();
    let _guard = EnvGuard::set(&[
        (
            wifi::RADIO_FILE_ENV,
            dir.join("radio-enabled.txt").to_str().expect("utf-8 path"),
        ),
        (
            wifi::NMCLI_DEVICES_FILE_ENV,
            dir.join("nmcli-devices.txt").to_str().expect("utf-8 path"),
        ),
        // Set-but-empty seam = "the cache read failed" (cold cache).
        (wifi::NMCLI_WIFI_FILE_ENV, ""),
    ]);
    let menu = wifi::menu();
    let rows = &menu.app.tabs[0].rows;
    assert_eq!(rows.len(), 2, "radio row + placeholder: {rows:?}");
    assert_eq!(rows[1].label, wifi::SCANNING_LABEL);
    assert!(rows[1].offline, "placeholder renders dim");
    assert_eq!(rows[1].id.as_str(), center::NOOP_ID);
}

#[test]
fn finished_scan_replaces_the_placeholder_and_focuses_a_network() {
    let _lock = SCAN_LOCK.lock().expect("scan lock");
    let mut menu = flex_rice::menu(
        wifi::PROVIDER,
        vec![wifi::tab_from(snap(
            Some("enabled\n"),
            Some(DEVICES),
            Some(""),
        ))],
    );
    let _ = handle_key(&mut menu, press(KeyCode::Down), run::test_base());
    assert_eq!(
        menu.app.focused_row().expect("row").label,
        wifi::SCANNING_LABEL
    );

    wifi::store_scan(wifi::rows(snap(
        Some("enabled\n"),
        Some(DEVICES),
        Some(secure_list().as_str()),
    )));
    wifi::refresh_scan(&mut menu);

    assert_eq!(
        menu.app.focused_row().expect("row").label,
        "HomeNet",
        "the cursor lands on the first network, not the dead placeholder"
    );
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "wifi");
    assert!(
        !menu.app.tabs[0]
            .rows
            .iter()
            .any(|row| row.label == wifi::SCANNING_LABEL),
        "placeholder is gone: {:?}",
        menu.app.tabs[0].rows
    );
}

#[test]
fn refresh_scan_keeps_the_filter_and_the_focused_ssid() {
    let _lock = SCAN_LOCK.lock().expect("scan lock");
    let mut menu = fixture_menu();
    let base = run::test_base();
    // `co` matches Corp:Net and Coffee Shop; the ranking decides which is
    // first, so read the focused row instead of assuming one.
    let _ = run::replay_keys(&mut menu, &[rune('c'), rune('o')], base);
    let focused = menu.app.focused_row().expect("row").label.clone();
    assert!(
        focused == "Corp:Net" || focused == "Coffee Shop",
        "filtered focus: {focused:?}"
    );

    // A fresh scan: the disconnect row is gone and the ranking reorders.
    wifi::store_scan(wifi::rows(snap(
        Some("enabled\n"),
        Some(DEVICES),
        Some(fixture("nmcli-wifi-disconnected.txt").as_str()),
    )));
    wifi::refresh_scan(&mut menu);

    assert_eq!(
        menu.app.active_state().expect("state").filter,
        "co",
        "the half-typed filter survives the swap"
    );
    assert_eq!(
        menu.app.focused_row().expect("row").label,
        focused,
        "focus follows the SSID, not the row index"
    );
}

#[test]
fn refresh_scan_follows_the_focused_network_through_a_reorder() {
    let _lock = SCAN_LOCK.lock().expect("scan lock");
    let mut menu = fixture_menu();
    let base = run::test_base();
    // No filter: focus the last row (Lobby) — off, disconnect, 4 networks.
    let down = press(KeyCode::Down);
    let _ = run::replay_keys(&mut menu, &[down; 5], base);
    assert_eq!(menu.app.focused_row().expect("row").label, "Lobby");
    assert_eq!(
        menu.app.focused_original_index(),
        Some(5),
        "Lobby starts as raw row 5"
    );

    // The fresh scan drops the disconnect row and moves Lobby to raw row 2:
    // keeping the index would land on Corp:Net, following the SSID must not.
    wifi::store_scan(wifi::rows(snap(
        Some("enabled\n"),
        Some(DEVICES),
        Some(":HomeNet:87:WPA2\n:Lobby:5:WPA3\n:Corp\\:Net:63:WPA2\n"),
    )));
    wifi::refresh_scan(&mut menu);

    assert_eq!(
        menu.app.focused_row().expect("row").label,
        "Lobby",
        "the cursor stays on the same network after a reorder"
    );
    assert_eq!(
        menu.app.focused_original_index(),
        Some(2),
        "…at its new raw index"
    );
}

#[test]
fn refresh_scan_clamps_when_the_focused_network_vanishes() {
    let _lock = SCAN_LOCK.lock().expect("scan lock");
    let mut menu = fixture_menu();
    let base = run::test_base();
    // Focus the last network (Lobby: off, disconnect, 4 networks → 5 Downs),
    // which the fresh scan does not have. No filter, so the whole list stays
    // visible and the clamp is the only thing keeping focus in range.
    let down = press(KeyCode::Down);
    let _ = run::replay_keys(&mut menu, &[down; 5], base);
    assert_eq!(menu.app.focused_row().expect("row").label, "Lobby");

    wifi::store_scan(wifi::rows(snap(
        Some("enabled\n"),
        Some(DEVICES),
        Some(fixture("nmcli-wifi-disconnected.txt").as_str()),
    )));
    wifi::refresh_scan(&mut menu);

    let rows = &menu.app.tabs[0].rows;
    let visible = menu.app.visible_rows().len();
    assert!(visible > 0, "the fresh rows are visible");
    assert!(
        menu.app.active_state().expect("state").focus < visible,
        "focus stays inside the filtered view"
    );
    assert!(
        !rows.iter().any(|row| row.label == "Lobby"),
        "the vanished row is gone"
    );
}

#[test]
fn refresh_scan_without_a_finished_scan_changes_nothing() {
    let _lock = SCAN_LOCK.lock().expect("scan lock");
    let mut menu = fixture_menu();
    // Drain anything a sibling test may have left in the process-global
    // mailbox before taking the snapshot.
    wifi::refresh_scan(&mut menu);
    let before: Vec<(String, String)> = menu.app.tabs[0]
        .rows
        .iter()
        .map(|row| (row.id.0.clone(), row.label.clone()))
        .collect();

    wifi::refresh_scan(&mut menu);

    let after: Vec<(String, String)> = menu.app.tabs[0]
        .rows
        .iter()
        .map(|row| (row.id.0.clone(), row.label.clone()))
        .collect();
    assert_eq!(before, after, "an idle tick must not churn the list");
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "off");
}

// --- Key-seq replays --------------------------------------------------------

#[test]
fn keyseq_esc_cancels_with_no_action() {
    let mut menu = fixture_menu();
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], run::test_base());
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
}

#[test]
fn typing_filters_then_enter_selects_the_network() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[rune('c'), rune('o'), rune('f'), press(KeyCode::Enter)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select, "filtered Enter selects");
    assert_eq!(menu.app.focused_row().expect("row").label, "Coffee Shop");
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "wifi");
}

#[test]
fn enter_on_the_first_row_turns_the_radio_off() {
    let mut menu = fixture_menu();
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Enter)], run::test_base());
    assert_eq!(outcome, KeyOutcome::Select);
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "off");
}

#[test]
fn down_then_enter_selects_disconnect() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Down), press(KeyCode::Enter)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select);
    assert_eq!(
        menu.app.focused_row().expect("row").label,
        "Disconnect from HomeNet"
    );
}

#[test]
fn delete_never_fires_on_wifi_rows() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Delete), press(KeyCode::Delete)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Consumed, "Delete is dead on wifi");
    assert!(
        !menu.app.active_state().expect("state").confirm_pending,
        "no confirm arms on non-deletable tabs"
    );
}

#[test]
fn flex_test_replays_are_deterministic_across_bases() {
    let script = [rune('o'), rune('p'), press(KeyCode::Enter)];
    let run_script = |menu: &mut Menu| {
        let base = run::test_base();
        let mut outcomes = Vec::new();
        for (index, key) in script.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let step = index as u32;
            outcomes.push(handle_key(menu, *key, base + run::FLEX_TEST_STEP * step));
        }
        outcomes
    };
    let mut first = fixture_menu();
    let mut second = fixture_menu();
    assert_eq!(run_script(&mut first), run_script(&mut second));
    assert_eq!(
        first.app.focused_row().expect("row").id,
        second.app.focused_row().expect("row").id
    );
}

// --- Golden -----------------------------------------------------------------

/// Concatenate non-skip symbols of row `y` (exact cell content, no ANSI).
fn row_text(buf: &ratatui::buffer::Buffer, y: u16, w: u16) -> String {
    let mut out = String::new();
    for x in 0..w {
        let cell = buf.cell((x, y)).expect("cell in frame");
        if !cell.skip {
            out.push_str(cell.symbol());
        }
    }
    out
}

fn draw(menu: &mut Menu, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    terminal
        .draw(|frame| flex_core::render::render(frame, menu))
        .expect("render frame");
    terminal.backend().buffer().clone()
}

fn assert_full_width(buf: &ratatui::buffer::Buffer, w: u16, h: u16) {
    for y in 0..h {
        let mut x = 0_u16;
        let mut total = 0_usize;
        while x < w {
            let cell = buf.cell((x, y)).expect("cell in frame");
            assert!(!cell.skip, "dangling skip cell at ({x}, {y})");
            let symbol_w = flex_core::width::str_width(cell.symbol());
            total += symbol_w;
            x += u16::try_from(symbol_w.max(1)).expect("row width fits u16");
        }
        assert_eq!(total, usize::from(w), "row {y} must be exactly {w} cells");
    }
}

#[test]
fn wifi_default_view_golden_at_80x24() {
    let mut menu = fixture_menu();
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf, 80, 24);
    let tab_bar = row_text(&buf, 23, 80);
    assert!(
        tab_bar.contains(wifi::TAB_NAME),
        "tab bar names the surface: {tab_bar:?}"
    );
    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    for token in [
        "Turn Wi-Fi Off",
        "Disconnect from HomeNet",
        "Coffee Shop",
        "87%",
        "WPA2",
        "filter",
        "navigate",
    ] {
        assert!(all.contains(token), "80x24 frame contains {token:?}");
    }
    // Standard mode: the selection bar sits on col 0 of the first node row,
    // below the reserved `•••` indicator line.
    let first_y = flex_core::render::LIST_INDICATOR_ROWS / 2;
    let selector = buf.cell((0, first_y)).expect("first list row selector");
    assert_eq!(selector.symbol(), "░");
    assert_eq!(
        selector.fg,
        flex_core::theme::Theme::DEFAULT
            .selector
            .fg
            .expect("selector sets a fg")
    );
}

// --- Wrapper tests ----------------------------------------------------------

fn wrapper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-wifi.sh")
}

fn stub_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("flex-wifi-test-{}-{name}", std::process::id()))
}

fn write_stub(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).expect("write stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// One wrapper run: `flex` echoes `action_line` (or exits 130 when it is
/// `None`), while `nmcli` and `notify-send` stubs log every call.
struct StubRun {
    dir: PathBuf,
    output: std::process::Output,
}

impl StubRun {
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("calls.log")).unwrap_or_default()
    }
}

impl Drop for StubRun {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn run_wrapper(
    name: &str,
    action_line: Option<&str>,
    password: Option<&str>,
    list: &str,
) -> StubRun {
    run_wrapper_with_profiles(name, action_line, password, list, "")
}

/// Same, with saved `NetworkManager` profiles (`nmcli … connection show` output).
fn run_wrapper_with_profiles(
    name: &str,
    action_line: Option<&str>,
    password: Option<&str>,
    list: &str,
    profiles: &str,
) -> StubRun {
    let dir = stub_dir(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    let log = dir.join("calls.log");
    let list_file = dir.join("wifi-list.txt");
    let profiles_file = dir.join("profiles.txt");
    std::fs::write(&list_file, list).expect("list fixture");
    std::fs::write(&profiles_file, profiles).expect("profiles fixture");

    let flex = dir.join("flex");
    match action_line {
        Some(line) => write_stub(&flex, &format!("#!/usr/bin/env bash\necho '{line}'\n")),
        None => write_stub(&flex, "#!/usr/bin/env bash\nexit 130\n"),
    }

    // Argument-shaped `nmcli` stub: logs every call, answers the two
    // queries the wrapper makes (interface discovery + fresh state probe).
    let nmcli = dir.join("nmcli");
    write_stub(
        &nmcli,
        &format!(
            r#"#!/usr/bin/env bash
echo "nmcli $*" >> "{log}"
case "$*" in
    "-t -f DEVICE,TYPE device") echo "wlan0:wifi" ;;
    "-t -f NAME,TYPE connection show") cat "{profiles}" ;;
    "-t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname wlan0 --rescan no") cat "{list}" ;;
    # `StaleNet` stands for a saved network whose stored key no longer works:
    # only the explicit password path succeeds.
    *"device wifi connect StaleNet"*)
        case "$*" in
            *"password s3cret"*) echo "Device 'wlan0' successfully activated" ;;
            *) echo "Error: Connection activation failed: (7) Secrets were required, but not provided." >&2; exit 1 ;;
        esac
        ;;
esac
"#,
            log = log.display(),
            list = list_file.display(),
            profiles = profiles_file.display()
        ),
    );
    let notify = dir.join("notify-send");
    write_stub(
        &notify,
        &format!(
            "#!/usr/bin/env bash\necho \"notify-send $*\" >> \"{}\"\n",
            log.display()
        ),
    );

    let path_env = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = std::process::Command::new("bash");
    cmd.arg(wrapper_path()).env("PATH", path_env);
    // The popup re-exec is bind-path behavior; wrapper tests emulate the
    // in-popup half (stubbed HOME has no popup.sh).
    cmd.env("POPUP_KITTY", "1");
    if let Some(password) = password {
        cmd.env("FLEX_WIFI_PASSWORD", password);
    }
    let output = cmd.output().expect("run wrapper");
    StubRun { dir, output }
}

fn secure_list() -> String {
    fixture("nmcli-wifi.txt")
}

#[test]
fn wrapper_turns_the_radio_on_and_off() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    for (name, action, expected) in [
        ("on", "ACTION: wifi on Turn Wi-Fi On", "nmcli radio wifi on"),
        (
            "off",
            "ACTION: wifi off Turn Wi-Fi Off",
            "nmcli radio wifi off",
        ),
    ] {
        let run = run_wrapper(name, Some(action), None, &secure_list());
        assert!(
            run.output.status.success(),
            "{name} stderr: {:?}",
            String::from_utf8_lossy(&run.output.stderr)
        );
        assert_eq!(run.log().trim_end(), expected);
    }
}

#[test]
fn wrapper_disconnects_the_interface() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper(
        "disconnect",
        Some("ACTION: wifi disconnect Disconnect from HomeNet"),
        None,
        &secure_list(),
    );
    assert!(
        run.output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    let log = run.log();
    assert!(
        log.contains("nmcli -t -f DEVICE,TYPE device"),
        "logs the interface probe: {log:?}"
    );
    assert!(
        log.contains("nmcli device disconnect wlan0"),
        "disconnects the interface: {log:?}"
    );
    assert!(
        log.contains("notify-send -a Wi-Fi Disconnected HomeNet"),
        "notifies with the unescaped label: {log:?}"
    );
}

#[test]
fn wrapper_connects_an_open_network_without_a_prompt() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper(
        "open",
        Some("ACTION: wifi wifi Coffee Shop"),
        None,
        &secure_list(),
    );
    assert!(
        run.output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    let log = run.log();
    assert!(
        log.contains("nmcli device wifi connect Coffee Shop ifname wlan0"),
        "open network connects directly: {log:?}"
    );
    assert!(
        !log.contains("password"),
        "no password on an open network: {log:?}"
    );
    assert!(log.contains("notify-send -a Wi-Fi Connected Coffee Shop"));
}

#[test]
fn wrapper_prompts_for_a_secured_network_password() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper(
        "secure",
        Some("ACTION: wifi wifi Corp:Net"),
        Some("s3cret"),
        &secure_list(),
    );
    assert!(
        run.output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    let log = run.log();
    assert!(
        log.contains("nmcli device wifi connect Corp:Net password s3cret ifname wlan0"),
        "secured network uses the password: {log:?}"
    );
    assert!(
        String::from_utf8_lossy(&run.output.stderr).contains("Connecting to Corp:Net"),
        "the popup says what it is doing instead of going blank"
    );
}

/// Regression (B-017): the prompt must be *visible*. A `read -p` prompt goes
/// to stderr, and an early cut sent that stderr to `/dev/null`, so the popup
/// sat blank and looked hung; the next Enter then silently cancelled the
/// connect.
#[test]
fn wrapper_prints_the_password_prompt_and_never_fails_silently() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper(
        "prompt",
        Some("ACTION: wifi wifi Corp:Net"),
        None, // no FLEX_WIFI_PASSWORD: the wrapper has to ask
        &secure_list(),
    );
    let stderr = String::from_utf8_lossy(&run.output.stderr).into_owned();
    assert!(
        stderr.contains("Password for Corp:Net: "),
        "the prompt reaches the user: {stderr:?}"
    );
    assert!(
        stderr.contains("No password entered"),
        "an empty answer is stated, not swallowed: {stderr:?}"
    );
    assert!(
        run.output.status.success(),
        "declining to connect is not an error"
    );
    assert!(
        !run.log().contains("device wifi connect"),
        "no connect without a password: {:?}",
        run.log()
    );
}

/// An open network connects without prompting, and still reports progress.
#[test]
fn wrapper_reports_progress_for_open_networks() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper(
        "open-progress",
        Some("ACTION: wifi wifi Coffee Shop"),
        None,
        &secure_list(),
    );
    let stderr = String::from_utf8_lossy(&run.output.stderr).into_owned();
    assert!(
        stderr.contains("Connecting to Coffee Shop"),
        "open connect reports progress: {stderr:?}"
    );
    assert!(
        !stderr.contains("Password for"),
        "no prompt for an open network: {stderr:?}"
    );
}

#[test]
fn wrapper_selecting_the_connected_network_disconnects_it() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper(
        "toggle",
        Some("ACTION: wifi wifi HomeNet"),
        None,
        &secure_list(),
    );
    assert!(run.output.status.success());
    let log = run.log();
    assert!(
        log.contains("nmcli device disconnect wlan0"),
        "toggles off the active network: {log:?}"
    );
    assert!(
        !log.contains("device wifi connect"),
        "never re-connects: {log:?}"
    );
    assert!(log.contains("notify-send -a Wi-Fi Disconnected HomeNet"));
}

#[test]
fn wrapper_noop_row_runs_nothing() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper(
        "noop",
        Some("ACTION: wifi noop (No Wi-Fi networks)"),
        None,
        "",
    );
    assert!(run.output.status.success());
    assert_eq!(run.log(), "", "noop touches no command");
}

#[test]
fn wrapper_unescapes_a_backslash_label() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    // `CORP\NET` on the wire is the SSID `CORP\NET` (ACTION: escapes `\\`).
    let run = run_wrapper(
        "escape",
        Some(r"ACTION: wifi wifi CORP\\NET"),
        None,
        &secure_list(),
    );
    assert!(
        run.output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    let log = run.log();
    assert!(
        log.contains(r"nmcli device wifi connect CORP\NET ifname wlan0"),
        "single backslash reaches nmcli: {log:?}"
    );
}

#[test]
fn wrapper_propagates_cancel_without_acting() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper("cancel", None, None, &secure_list());
    assert_eq!(
        run.output.status.code(),
        Some(130),
        "Esc/Ctrl-C passes through"
    );
    assert_eq!(run.log(), "", "cancel runs no command");
}

#[test]
fn wrapper_rejects_malformed_and_unknown_actions() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let malformed = run_wrapper("bad", Some("GARBAGE LINE"), None, &secure_list());
    assert!(!malformed.output.status.success(), "malformed must fail");
    assert_eq!(malformed.log(), "", "nothing runs on a bad line");

    let unknown = run_wrapper(
        "unknown",
        Some("ACTION: wifi format Format"),
        None,
        &secure_list(),
    );
    assert!(!unknown.output.status.success(), "unknown id must fail");
    assert_eq!(unknown.log(), "", "nothing runs on an unknown id");
}

/// The user-visible rule: a network `NetworkManager` has saved must connect from
/// its stored profile — never re-asking for a password it already holds.
#[test]
fn wrapper_uses_the_saved_profile_instead_of_asking_for_a_password() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper_with_profiles(
        "saved",
        Some("ACTION: wifi wifi Corp:Net"),
        None, // no FLEX_WIFI_PASSWORD: a prompt would be visible in stderr
        &secure_list(),
        "HomeNet:802-11-wireless\nCorp\\:Net:802-11-wireless\nlo:loopback\n",
    );
    let stderr = String::from_utf8_lossy(&run.output.stderr).into_owned();
    assert!(
        !stderr.contains("Password for"),
        "a saved network must not be asked for its stored password: {stderr:?}"
    );
    assert!(stderr.contains("Connecting to Corp:Net"), "{stderr:?}");
    let log = run.log();
    assert!(
        log.contains("nmcli device wifi connect Corp:Net ifname wlan0"),
        "connected from the profile: {log:?}"
    );
    assert!(
        !log.contains("password"),
        "no password is passed for a saved network: {log:?}"
    );
    assert!(
        log.contains("notify-send -a Wi-Fi Connected Corp:Net"),
        "{log:?}"
    );
}

/// A non-Wi-Fi profile with the same name must not count as saved.
#[test]
fn wrapper_ignores_non_wifi_profiles_when_deciding_to_prompt() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper_with_profiles(
        "wired-namesake",
        Some("ACTION: wifi wifi Corp:Net"),
        None,
        &secure_list(),
        "Corp\\:Net:802-3-ethernet\n",
    );
    let stderr = String::from_utf8_lossy(&run.output.stderr).into_owned();
    assert!(
        stderr.contains("Password for Corp:Net: "),
        "an ethernet profile named like the SSID is not a saved Wi-Fi profile: {stderr:?}"
    );
    assert!(
        !run.log().contains("device wifi connect"),
        "nothing is attempted before the password is known"
    );
}

/// Saved credentials can be stale (the network's key changed): the wrapper
/// tries the profile, then asks instead of giving up.
#[test]
fn wrapper_asks_when_the_saved_credentials_are_rejected() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let run = run_wrapper_with_profiles(
        "stale",
        Some("ACTION: wifi wifi StaleNet"),
        Some("s3cret"), // the password the retry must use
        ":StaleNet:70:WPA2\n",
        "StaleNet:802-11-wireless\n",
    );
    let stderr = String::from_utf8_lossy(&run.output.stderr).into_owned();
    assert!(
        stderr.contains("Saved credentials for StaleNet were rejected"),
        "the fallback explains itself: {stderr:?}"
    );
    let log = run.log();
    assert_eq!(
        log.matches("nmcli device wifi connect StaleNet").count(),
        2,
        "profile attempt, then the password attempt: {log:?}"
    );
    assert!(
        log.contains("nmcli device wifi connect StaleNet password s3cret ifname wlan0"),
        "{log:?}"
    );
    assert!(
        log.contains("notify-send -a Wi-Fi Connected StaleNet"),
        "{log:?}"
    );
}

// --- Executor tests ----------------------------------------------------------
//
// The `exec::wifi` port of the wrapper above: same stub idioms (per-tool
// logging stubs, byte-compared call logs, scratch dirs). Process-env
// mutations ride under the file's `ENV_LOCK` with the shared [`EnvGuard`]
// save/restore. No test touches the network, a TUI, or a pty: the password
// prompt runs through `execute_with_stdio` with `None` tty (the stdin
// fallback) plus a piped-stdin `Cursor`, and stderr is a captured buffer.

/// `PATH` shadow for executor calls: stub dir first, ambient `PATH` after
/// (stubs are `bash` scripts, and the tools they exec stay ambient).
fn exec_path_env(dir: &Path) -> String {
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// `nmcli` stub: interface + `$WIFI_LIST`/`$PROFILES` answers, every call
/// logged to `$STUB_LOG`. Connects succeed, except a non-`s3cret` password
/// and a passwordless `StaleNet` (the stale-key case: only the explicit
/// password path succeeds); `${OPEN_EXIT:-0}` fails open connects on
/// demand (the `Failed`-notify case).
const EXEC_NMCLI_STUB: &str = r#"#!/usr/bin/env bash
printf 'nmcli %s\n' "$*" >> "$STUB_LOG"
case "$1" in
    -t) case "$3" in
        DEVICE,TYPE) printf 'wlan0:wifi\neth0:ethernet\n' ;;
        NAME,TYPE) cat "$PROFILES" ;;
        IN-USE,SSID,SIGNAL,SECURITY) cat "$WIFI_LIST" ;;
    esac ;;
    radio) exit 0 ;;
    device) case "$2" in
        disconnect) exit 0 ;;
        wifi) case "$*" in
            *'password s3cret'*) exit 0 ;;
            *'password '*) exit 1 ;;
            *'StaleNet'*) exit 1 ;;
            *) exit "${OPEN_EXIT:-0}" ;;
        esac ;;
    esac ;;
esac
exit 0
"#;

/// `notify-send` stub: logs every call, always succeeds.
const EXEC_NOTIFY_STUB: &str =
    "#!/usr/bin/env bash\nprintf 'notify-send %s\\n' \"$*\" >> \"$STUB_LOG\"\nexit 0\n";

/// One executor run with injected stdio: no tty (the stdin-fallback path),
/// piped `stdin_bytes`, captured stderr. Returns the result and the stderr
/// text. `FLEX_WIFI_PASSWORD` rides on process env (set via [`EnvGuard`]).
fn run_exec(
    dir: &Path,
    id: &str,
    label: &str,
    stdin_bytes: &[u8],
) -> (anyhow::Result<exec_wifi::ExecuteReport>, String) {
    let path_env = exec_path_env(dir);
    let mut stdin = Cursor::new(stdin_bytes.to_vec());
    let mut err = Vec::new();
    let result =
        exec_wifi::execute_with_stdio(id, label, Some(&path_env), None, &mut stdin, &mut err);
    (
        result,
        String::from_utf8(err).expect("executor stderr is utf-8"),
    )
}

fn exec_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

/// Stub dir with the executor's tools, list, and profiles installed; env
/// seams pointed at them. Returns the dir (the caller holds `ENV_LOCK` and
/// drops the dir when done).
fn install_exec_stubs(name: &str, list: &str, profiles: &str) -> PathBuf {
    let dir = stub_dir(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    write_stub(&dir.join("nmcli"), EXEC_NMCLI_STUB);
    write_stub(&dir.join("notify-send"), EXEC_NOTIFY_STUB);
    std::fs::write(dir.join("wifi-list.txt"), list).expect("wifi list");
    std::fs::write(dir.join("profiles.txt"), profiles).expect("profiles");
    dir
}

fn seam_guard(dir: &Path, password: &str) -> EnvGuard {
    EnvGuard::set(&[
        ("NMCLI", dir.join("nmcli").to_str().expect("utf-8 path")),
        (
            "NOTIFY_SEND",
            dir.join("notify-send").to_str().expect("utf-8 path"),
        ),
        ("FLEX_WIFI_PASSWORD", password),
        (
            "STUB_LOG",
            dir.join("calls.log").to_str().expect("utf-8 path"),
        ),
        (
            "WIFI_LIST",
            dir.join("wifi-list.txt").to_str().expect("utf-8 path"),
        ),
        (
            "PROFILES",
            dir.join("profiles.txt").to_str().expect("utf-8 path"),
        ),
    ])
}

const EXEC_LIST: &str = "*:HomeNet:87:WPA2\n:Coffee Shop:41:--\n:MyNet:70:WPA2\n";

/// Full action matrix through one shared call log: radio on/off, the
/// explicit disconnect arm, an open connect, selecting the connected
/// network (disconnects), and the quiet `noop`. Byte-compared against the
/// wrapper's shapes.
#[test]
fn executor_matrix_runs_every_action_through_stub_tools() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = install_exec_stubs("exec-matrix", EXEC_LIST, "");
    let _guard = seam_guard(&dir, "");
    let run = |id: &str, label: &str| run_exec(&dir, id, label, b"");
    let (report, err) = run("on", "Turn Wi-Fi On");
    assert_eq!(report.expect("on succeeds").detail, None);
    assert_eq!(err, "", "radio arms are silent");
    let (report, err) = run("off", "Turn Wi-Fi Off");
    assert_eq!(report.expect("off succeeds").detail, None);
    assert_eq!(err, "");
    let (report, err) = run("disconnect", "Disconnect from HomeNet");
    assert_eq!(
        report.expect("disconnect succeeds").detail.as_deref(),
        Some("HomeNet"),
        "the notify body strips the Disconnect prefix"
    );
    assert_eq!(err, "", "the disconnect arm prints no progress");
    let (report, err) = run("wifi", "Coffee Shop");
    assert_eq!(
        report.expect("open connect succeeds").detail.as_deref(),
        Some("Coffee Shop")
    );
    assert_eq!(err, "Connecting to Coffee Shop…\n");
    let (report, err) = run("wifi", "HomeNet");
    assert_eq!(
        report.expect("connected toggle succeeds").detail.as_deref(),
        Some("HomeNet")
    );
    assert_eq!(err, "", "disconnecting prints no progress");
    let (report, err) = run("noop", "(No Wi-Fi networks)");
    assert_eq!(report.expect("noop succeeds").detail, None);
    assert_eq!(err, "");
    assert_eq!(
        exec_log(&dir).lines().collect::<Vec<_>>(),
        vec![
            "nmcli radio wifi on",
            "nmcli radio wifi off",
            "nmcli -t -f DEVICE,TYPE device",
            "nmcli device disconnect wlan0",
            "notify-send -a Wi-Fi Disconnected HomeNet",
            "nmcli -t -f DEVICE,TYPE device",
            "nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname wlan0 --rescan no",
            "nmcli device wifi connect Coffee Shop ifname wlan0",
            "notify-send -a Wi-Fi Connected Coffee Shop",
            "nmcli -t -f DEVICE,TYPE device",
            "nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname wlan0 --rescan no",
            "nmcli device disconnect wlan0",
            "notify-send -a Wi-Fi Disconnected HomeNet",
        ],
        "one tool sequence per action (describe: one line per step)",
    );
}

/// Secure wifi uses the stored password (no `/dev/tty` read) and notifies.
#[test]
fn executor_secure_uses_the_stored_password() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = install_exec_stubs("exec-secure", EXEC_LIST, "");
    let _guard = seam_guard(&dir, "s3cret");
    let (result, err) = run_exec(&dir, "wifi", "MyNet", b"");
    assert_eq!(
        result.expect("secure connect succeeds").detail.as_deref(),
        Some("MyNet")
    );
    assert_eq!(err, "Connecting to MyNet…\n", "progress only, no prompt");
    assert_eq!(
        exec_log(&dir).lines().collect::<Vec<_>>(),
        vec![
            "nmcli -t -f DEVICE,TYPE device",
            "nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname wlan0 --rescan no",
            "nmcli -t -f NAME,TYPE connection show",
            "nmcli device wifi connect MyNet password s3cret ifname wlan0",
            "notify-send -a Wi-Fi Connected MyNet",
        ],
    );
}

/// A network `NetworkManager` has saved connects from its stored profile —
/// never re-asking for a password it already holds (and never reading
/// stdin for one).
#[test]
fn executor_uses_the_saved_profile_instead_of_asking_for_a_password() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = install_exec_stubs(
        "exec-saved",
        EXEC_LIST,
        "HomeNet:802-11-wireless\nCorp\\:Net:802-11-wireless\nlo:loopback\n",
    );
    let _guard = seam_guard(&dir, "");
    let mut stdin = Cursor::new(b"must-not-be-read\n".to_vec());
    let mut err = Vec::new();
    let result = exec_wifi::execute_with_stdio(
        "wifi",
        "Corp:Net",
        Some(&exec_path_env(&dir)),
        None,
        &mut stdin,
        &mut err,
    );
    assert_eq!(
        result.expect("saved connect succeeds").detail.as_deref(),
        Some("Corp:Net")
    );
    assert_eq!(stdin.position(), 0, "no prompt means no stdin read");
    let err = String::from_utf8(err).expect("utf-8 stderr");
    assert_eq!(err, "Connecting to Corp:Net…\n", "{err:?}");
    let log = exec_log(&dir);
    assert!(
        log.contains("nmcli device wifi connect Corp:Net ifname wlan0"),
        "connected from the profile: {log:?}"
    );
    assert!(
        !log.contains("password"),
        "no password is passed for a saved network: {log:?}"
    );
    assert!(
        log.contains("notify-send -a Wi-Fi Connected Corp:Net"),
        "{log:?}"
    );
}

/// Saved credentials can be stale: the profile attempt fails, the fallback
/// explains itself, and the retry uses the prompted password.
#[test]
fn executor_asks_when_the_saved_credentials_are_rejected() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = install_exec_stubs(
        "exec-stale",
        ":StaleNet:70:WPA2\n",
        "StaleNet:802-11-wireless\n",
    );
    let _guard = seam_guard(&dir, "s3cret");
    let (result, err) = run_exec(&dir, "wifi", "StaleNet", b"");
    assert_eq!(
        result.expect("stale retry succeeds").detail.as_deref(),
        Some("StaleNet")
    );
    assert_eq!(
        err,
        "Connecting to StaleNet…\n\
         Saved credentials for StaleNet were rejected — enter the password.\n\
         Connecting to StaleNet…\n",
        "the fallback explains itself: {err:?}"
    );
    let log = exec_log(&dir);
    assert_eq!(
        log.matches("nmcli device wifi connect StaleNet").count(),
        2,
        "profile attempt, then the password attempt: {log:?}"
    );
    assert!(
        log.contains("nmcli device wifi connect StaleNet password s3cret ifname wlan0"),
        "{log:?}"
    );
    assert!(
        log.contains("notify-send -a Wi-Fi Connected StaleNet"),
        "{log:?}"
    );
}

/// The prompt path with piped stdin and no tty: an answered read connects
/// with the password; an empty answer prints the explicit line and connects
/// nothing (mandates 1–3, byte-exact on stderr).
#[test]
fn executor_prompt_answers_from_stdin_and_states_empty_answers() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = install_exec_stubs("exec-prompt", EXEC_LIST, "");
    let _guard = seam_guard(&dir, "");
    // Answered: the stdin fallback supplies the password.
    let (result, err) = run_exec(&dir, "wifi", "MyNet", b"s3cret\n");
    assert_eq!(
        result.expect("prompted connect succeeds").detail.as_deref(),
        Some("MyNet")
    );
    assert_eq!(
        err, "Password for MyNet: \nConnecting to MyNet…\n",
        "prompt to stderr, then progress: {err:?}"
    );
    let log = exec_log(&dir);
    assert!(
        log.contains("nmcli -t -f NAME,TYPE connection show"),
        "unsaved secure networks still probe the profiles first: {log:?}"
    );
    assert!(
        log.contains("nmcli device wifi connect MyNet password s3cret ifname wlan0"),
        "{log:?}"
    );
    // Empty: stated, not swallowed; nothing is attempted.
    std::fs::remove_file(dir.join("calls.log")).expect("reset log");
    let (result, err) = run_exec(&dir, "wifi", "MyNet", b"");
    assert_eq!(
        result.expect("declining to connect is not an error").detail,
        None
    );
    assert_eq!(
        err, "Password for MyNet: \nNo password entered — not connecting to MyNet.\n",
        "{err:?}"
    );
    assert!(
        !exec_log(&dir).contains("device wifi connect"),
        "no connect without a password: {:?}",
        exec_log(&dir)
    );
}

/// A failing connect notifies `Failed` instead of `Connected` — on both
/// the open and the secure arms (the wifi wrapper's `if/else`; still `Ok`,
/// like every quiet arm).
#[test]
fn executor_failed_connect_notifies_failed() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = install_exec_stubs("exec-fail", ":Open:41:--\n:MyNet:70:WPA2\n", "");
    let _guard = EnvGuard::set(&[
        ("NMCLI", dir.join("nmcli").to_str().expect("utf-8 path")),
        (
            "NOTIFY_SEND",
            dir.join("notify-send").to_str().expect("utf-8 path"),
        ),
        ("FLEX_WIFI_PASSWORD", "wrong"),
        (
            "STUB_LOG",
            dir.join("calls.log").to_str().expect("utf-8 path"),
        ),
        (
            "WIFI_LIST",
            dir.join("wifi-list.txt").to_str().expect("utf-8 path"),
        ),
        (
            "PROFILES",
            dir.join("profiles.txt").to_str().expect("utf-8 path"),
        ),
        ("OPEN_EXIT", "1"),
    ]);
    let path_env = exec_path_env(&dir);
    // Open failure: connect logged, `Failed` notified, still `Ok`.
    let mut err = Vec::new();
    exec_wifi::execute_with_stdio(
        "wifi",
        "Open",
        Some(&path_env),
        None,
        &mut Cursor::new(Vec::new()),
        &mut err,
    )
    .expect("failed open connect is quiet");
    let log = exec_log(&dir);
    assert!(
        log.contains("nmcli device wifi connect Open ifname wlan0"),
        "connect attempted: {log:?}"
    );
    assert!(
        log.contains("notify-send -a Wi-Fi Failed Open"),
        "failure notifies: {log:?}"
    );
    assert!(!log.contains("Connected Open"), "{log:?}");
    // Secure failure (a wrong stored password): same gating.
    std::fs::remove_file(dir.join("calls.log")).expect("reset log");
    let mut err = Vec::new();
    exec_wifi::execute_with_stdio(
        "wifi",
        "MyNet",
        Some(&path_env),
        None,
        &mut Cursor::new(Vec::new()),
        &mut err,
    )
    .expect("failed secure connect is quiet");
    let log = exec_log(&dir);
    assert!(
        log.contains("notify-send -a Wi-Fi Failed MyNet"),
        "failure notifies: {log:?}"
    );
    assert!(!log.contains("Connected MyNet"), "{log:?}");
}

/// Malformed ids (`bad id`) and well-formed-but-unsupported rows are
/// errors with no `flex:` prefix of their own — the runner adds the single
/// prefix at the binary boundary.
#[test]
fn executor_rejects_bad_and_unknown_ids_without_its_own_prefix() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = install_exec_stubs("exec-bad", EXEC_LIST, "");
    let _guard = seam_guard(&dir, "");
    let path_env = exec_path_env(&dir);
    for (id, expected) in [
        ("", "wifi: bad id ''"),
        ("a/b", "wifi: bad id 'a/b'"),
        ("a\nb", "wifi: bad id 'a\nb'"),
        ("format", "wifi: unknown action: format"),
    ] {
        let mut err = Vec::new();
        let failure = exec_wifi::execute_with_stdio(
            id,
            id,
            Some(&path_env),
            None,
            &mut Cursor::new(Vec::new()),
            &mut err,
        )
        .expect_err("bad/unknown id must fail");
        let message = format!("{failure:#}");
        assert!(
            message.starts_with(expected),
            "unexpected message for {id:?}: {message:?}"
        );
        assert!(
            !message.contains("flex:"),
            "no runner prefix below the runner: {message:?}"
        );
    }
    assert!(
        !dir.join("calls.log").exists(),
        "no id failure runs anything"
    );
}

/// Binary level without a pty: the menu cannot start, so the binary exits 1
/// with exactly one `flex: error:` prefix (the runner owns it; executor
/// errors carry none).
#[test]
fn binary_errors_carry_a_single_prefix() {
    let output = std::process::Command::new("setsid")
        .arg(env!("CARGO_BIN_EXE_flex-wifi"))
        .env("POPUP_KITTY", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-wifi without a pty");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "no stdout on error");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.starts_with("flex: error: "),
        "runner prefix first: {stderr:?}"
    );
    assert_eq!(
        stderr.matches("flex: error:").count(),
        1,
        "exactly one prefix: {stderr:?}"
    );
}

/// Outside a popup the binary re-execs into the `menu` popup (the shared
/// runner guard); the kitty spawn is asserted against a stub `PATH`. The
/// stub `PATH` rides on the child env only — no process-env mutation,
/// hence no lock.
#[test]
fn binary_outside_a_popup_reexecs_into_the_menu_popup() {
    let dir = stub_dir("exec-guard");
    std::fs::create_dir_all(&dir).expect("stub dir");
    let kitty_log = dir.join("kitty.log");
    write_stub(&dir.join("pgrep"), "#!/usr/bin/env bash\nexit 1\n");
    write_stub(
        &dir.join("kitty"),
        &format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > '{}.tmp'\nmv '{}.tmp' '{}'\n",
            kitty_log.display(),
            kitty_log.display(),
            kitty_log.display(),
        ),
    );
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("scratch HOME");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex-wifi"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", &home)
        .env_remove("POPUP_KITTY")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-wifi outside a popup");
    assert!(
        output.status.success(),
        "toggle exits 0: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let logged = loop {
        if let Ok(body) = std::fs::read_to_string(&kitty_log) {
            break body.lines().map(str::to_string).collect::<Vec<_>>();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "kitty spawn never logged"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(
        logged.contains(&String::from("--class"))
            && logged.contains(&String::from("flex-menu"))
            && logged.contains(&String::from("POPUP_KITTY=1")),
        "re-exec uses the menu popup template: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Parity with the untouched wrapper ---------------------------------------
//
// For every action the wrapper supports, the wrapper (under stub `PATH`)
// and the executor must produce byte-identical tool-call sequences AND
// byte-identical stderr text for the prompt cases. One deliberate
// departure, asserted below: none on the tool sequences — the wifi flow
// needs no `flex --resolve` subprocess (unlike center's launch arm), so
// the wrapper's single `flex` call is the menu read both sides share and
// every stub-tool call matches byte for byte.

/// One parity case: the wrapper `ACTION:` line (via `$STUB_ACTION`), the
/// executor id/label, the piped stdin for the prompt path, and the
/// `FLEX_WIFI_PASSWORD` value (`None` = unset).
struct WifiParityCase {
    name: &'static str,
    action_line: &'static str,
    id: &'static str,
    label: String,
    password: Option<&'static str>,
    stdin: &'static [u8],
}

/// Fixed parity inputs: stub dirs, the wrapper, and the scratch `HOME` both
/// sides resolve against. One parity case runs the wrapper under stub
/// `PATH`, then the executor with injected stdio.
struct WifiParityHarness {
    dir: PathBuf,
    wrapper: PathBuf,
    path_env: String,
    home: PathBuf,
}

impl WifiParityHarness {
    /// Returns wrapper success, the wrapper tool log, the wrapper stderr,
    /// the executor tool log, the executor stderr, and executor success.
    fn run(&self, case: &WifiParityCase) -> (bool, String, String, String, String, bool) {
        for log in ["tools-wrap.log", "tools-exec.log"] {
            let _ = std::fs::remove_file(self.dir.join(log));
        }
        let wrap_tools = self.dir.join("tools-wrap.log");
        let exec_tools = self.dir.join("tools-exec.log");
        let mut wrap_cmd = std::process::Command::new("bash");
        wrap_cmd.arg(&self.wrapper);
        wrap_cmd.env("PATH", &self.path_env);
        wrap_cmd.env("HOME", &self.home);
        wrap_cmd.env("STUB_ACTION", case.action_line);
        wrap_cmd.env("POPUP_KITTY", "1");
        wrap_cmd.stdin(std::process::Stdio::null());
        // Tool seams + stub data ride on the child env (wrapper side).
        for key in ["NMCLI", "NOTIFY_SEND", "WIFI_LIST", "PROFILES", "OPEN_EXIT"] {
            if let Ok(value) = std::env::var(key) {
                wrap_cmd.env(key, value);
            }
        }
        match case.password {
            Some(password) => wrap_cmd.env("FLEX_WIFI_PASSWORD", password),
            None => wrap_cmd.env_remove("FLEX_WIFI_PASSWORD"),
        };
        wrap_cmd.env("STUB_LOG", &wrap_tools);
        let wrapper_out = wrap_cmd.output().expect("run wrapper");
        let wrap_log = std::fs::read_to_string(&wrap_tools).unwrap_or_default();
        let wrap_err = String::from_utf8_lossy(&wrapper_out.stderr).into_owned();
        let (exec_log, exec_err, exec_ok) = {
            // Same seam values, but the in-process executor reads them from
            // process env; only the tool log path differs.
            let saved = std::env::var("STUB_LOG").ok();
            std::env::set_var("STUB_LOG", &exec_tools);
            let mut stdin = Cursor::new(case.stdin.to_vec());
            let mut err = Vec::new();
            let result = exec_wifi::execute_with_stdio(
                case.id,
                &case.label,
                Some(&self.path_env),
                None,
                &mut stdin,
                &mut err,
            );
            match saved {
                Some(value) => std::env::set_var("STUB_LOG", value),
                None => std::env::remove_var("STUB_LOG"),
            }
            (
                std::fs::read_to_string(&exec_tools).unwrap_or_default(),
                String::from_utf8(err).expect("utf-8 stderr"),
                result.is_ok(),
            )
        };
        (
            wrapper_out.status.success(),
            wrap_log,
            wrap_err,
            exec_log,
            exec_err,
            exec_ok,
        )
    }
}

/// Strip bash's `/dev/tty` redirection-failure diagnostic from wrapper
/// stderr (`<wrapper>: line 154: /dev/tty: No such device or address`): on a
/// tty-less machine the wrapper's `read -rs pw </dev/tty` complains before
/// the `|| read -rs` fallback runs. The executor never shells out, so it
/// has no counterpart — it falls back silently. The prompt, newline, and
/// empty-answer bytes must still match exactly (asserted separately).
/// `script` is the `$0` path the wrapper was invoked with (bash prefixes
/// the diagnostic with it, glued to the prompt's line).
fn strip_tty_diagnostic(stderr: &str, script: &str) -> (String, Vec<String>) {
    let mut stripped = Vec::new();
    let mut kept = String::new();
    for chunk in stderr.split_inclusive('\n') {
        // The diagnostic shares the prompt's line (the prompt has no
        // trailing newline): keep the prompt bytes, drop the rest.
        if let Some(pos) = chunk.find(script) {
            stripped.push(chunk[pos..].to_string());
            kept.push_str(&chunk[..pos]);
        } else {
            kept.push_str(chunk);
        }
    }
    (kept, stripped)
}
/// the disconnect arm, open, connected-toggle, secure via the seam, saved,
/// stale-key retry, backslash SSID, the empty-answer prompt, and noop).
/// One parity case per wrapper-supported action (11 cases: radio on/off,
/// the disconnect arm, open, connected-toggle, secure via the seam, saved,
/// stale-key retry, backslash SSID, the empty-answer prompt, and noop).
fn parity_cases() -> Vec<WifiParityCase> {
    vec![
        WifiParityCase {
            name: "on",
            action_line: "ACTION: wifi on Turn Wi-Fi On",
            id: "on",
            label: String::from("Turn Wi-Fi On"),
            password: None,
            stdin: b"",
        },
        WifiParityCase {
            name: "off",
            action_line: "ACTION: wifi off Turn Wi-Fi Off",
            id: "off",
            label: String::from("Turn Wi-Fi Off"),
            password: None,
            stdin: b"",
        },
        WifiParityCase {
            name: "disconnect",
            action_line: "ACTION: wifi disconnect Disconnect from HomeNet",
            id: "disconnect",
            label: String::from("Disconnect from HomeNet"),
            password: None,
            stdin: b"",
        },
        WifiParityCase {
            name: "open",
            action_line: "ACTION: wifi wifi Coffee Shop",
            id: "wifi",
            label: String::from("Coffee Shop"),
            password: None,
            stdin: b"",
        },
        WifiParityCase {
            name: "connected-toggle",
            action_line: "ACTION: wifi wifi HomeNet",
            id: "wifi",
            label: String::from("HomeNet"),
            password: None,
            stdin: b"",
        },
        WifiParityCase {
            name: "secure",
            action_line: "ACTION: wifi wifi MyNet",
            id: "wifi",
            label: String::from("MyNet"),
            password: Some("s3cret"),
            stdin: b"",
        },
        WifiParityCase {
            name: "saved",
            action_line: "ACTION: wifi wifi Corp:Net",
            id: "wifi",
            label: String::from("Corp:Net"),
            password: None,
            stdin: b"",
        },
        WifiParityCase {
            name: "stale",
            action_line: "ACTION: wifi wifi StaleNet",
            id: "wifi",
            label: String::from("StaleNet"),
            password: Some("s3cret"),
            stdin: b"",
        },
        WifiParityCase {
            name: "backslash",
            action_line: r"ACTION: wifi wifi CORP\\NET",
            id: "wifi",
            label: String::from("CORP\\NET"),
            password: Some("s3cret"),
            stdin: b"",
        },
        WifiParityCase {
            name: "prompt-empty",
            action_line: "ACTION: wifi wifi MyNet",
            id: "wifi",
            label: String::from("MyNet"),
            password: None,
            stdin: b"",
        },
        WifiParityCase {
            name: "noop",
            action_line: "ACTION: wifi noop (No Wi-Fi networks)",
            id: "noop",
            label: String::from("(No Wi-Fi networks)"),
            password: None,
            stdin: b"",
        },
    ]
}

#[test]
fn parity_wifi_matches_the_untouched_wrapper_per_action() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("parity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    write_stub(&dir.join("nmcli"), EXEC_NMCLI_STUB);
    write_stub(&dir.join("notify-send"), EXEC_NOTIFY_STUB);
    // The `flex` stub answers the wrapper's single menu read (`$STUB_ACTION`).
    write_stub(
        &dir.join("flex"),
        "#!/usr/bin/env bash\nprintf '%s\\n' \"$STUB_ACTION\"\n",
    );
    std::fs::write(
        dir.join("wifi-list.txt"),
        "*:HomeNet:87:WPA2\n:Coffee Shop:41:--\n:MyNet:70:WPA2\n\
         :Corp\\:Net:70:WPA2\n:StaleNet:70:WPA2\n:CORP\\\\NET:70:WPA2\n",
    )
    .expect("wifi list");
    std::fs::write(
        dir.join("profiles.txt"),
        "Corp\\:Net:802-11-wireless\nStaleNet:802-11-wireless\nlo:loopback\n",
    )
    .expect("profiles");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("scratch HOME");
    let path_env = exec_path_env(&dir);
    let _guard = EnvGuard::set(&[
        ("NMCLI", dir.join("nmcli").to_str().expect("utf-8 path")),
        (
            "NOTIFY_SEND",
            dir.join("notify-send").to_str().expect("utf-8 path"),
        ),
        (
            "STUB_LOG",
            dir.join("calls.log").to_str().expect("utf-8 path"),
        ),
        (
            "WIFI_LIST",
            dir.join("wifi-list.txt").to_str().expect("utf-8 path"),
        ),
        (
            "PROFILES",
            dir.join("profiles.txt").to_str().expect("utf-8 path"),
        ),
    ]);
    // `FLEX_WIFI_PASSWORD` rides per-case (set or removed around each run).
    let saved_password = std::env::var("FLEX_WIFI_PASSWORD").ok();
    let harness = WifiParityHarness {
        dir: dir.clone(),
        wrapper: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("wrappers")
            .join("flex-wifi.sh"),
        path_env,
        home: home.clone(),
    };
    for case in &parity_cases() {
        match case.password {
            Some(password) => std::env::set_var("FLEX_WIFI_PASSWORD", password),
            None => std::env::remove_var("FLEX_WIFI_PASSWORD"),
        }
        let (wrapper_ok, wrap_log, wrap_err, exec_log, exec_err, exec_ok) = harness.run(case);
        assert_eq!(
            wrapper_ok, exec_ok,
            "{}: wrapper and executor must agree on success",
            case.name
        );
        assert_eq!(
            wrap_log, exec_log,
            "{}: tool-call sequences must be byte-identical",
            case.name
        );
        let script = harness.wrapper.to_str().expect("utf-8 path").to_string();
        let (wrap_err_cmp, stripped) = strip_tty_diagnostic(&wrap_err, &script);
        assert!(
            stripped
                .iter()
                .all(|line| line.contains("/dev/tty") && line.contains("No such device")),
            "{}: only bash's redirection diagnostic may differ: {stripped:?}",
            case.name
        );
        assert_eq!(
            wrap_err_cmp, exec_err,
            "{}: stderr text must be byte-identical past the diagnostic",
            case.name
        );
    }
    match saved_password {
        Some(value) => std::env::set_var("FLEX_WIFI_PASSWORD", value),
        None => std::env::remove_var("FLEX_WIFI_PASSWORD"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
