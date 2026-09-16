//! `flex wifi` cutover tests (M8): provider fixtures, key-seq replays
//! (filter/navigate/cancel), the 80x24 golden, the live `$WIFI_*` seam
//! path, and stubbed-pipeline wrapper dispatch.
//!
//! Reference-data fixtures live in `tests/fixtures/wifi/` (captured
//! `nmcli` stdout); expected labels/metas are hand-computed from the
//! shared `center` row builders. No test touches the network.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Mutex;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::{run, Menu};
use flex_rice::providers::{center, wifi};

/// Serializes the tests that mutate process env (`WIFI_*` seams). Wrapper
/// tests need no lock (per-child `Command::env` only).
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
