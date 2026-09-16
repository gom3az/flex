//! `flex center` cutover tests (M5): provider fixtures per tab, the
//! 125x30 golden (5-tab bar + gauge states), key-seq replays (tab/digit
//! navigation, 60-tick soak, toggle flow, danger-arm on power), and
//! stubbed-pipeline wrapper tests.
//!
//! Reference-data fixtures live in `tests/fixtures/center/` (captured
//! `nmcli`/`bluetoothctl`/`wpctl` stdout); expected metas/labels below
//! are hand-computed from the bash formulas (`flex_bar`, gauge renderer).
//! No test touches the network, `bluetoothd`, or the live theme dirs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::{run, width, Menu};
use flex_rice::providers::{center, launch, theme_};

/// Serializes the tests that mutate process env (`CENTER_*` seams).
/// The wrapper tests need no lock (per-child `Command::env` only).
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("center")
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(fixtures_dir().join(name)).expect("read fixture")
}

fn launch_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("launch")
}

fn fixture_themes() -> Vec<theme_::ThemeEntry> {
    vec![
        theme_::ThemeEntry {
            name: "mocha".to_string(),
            wallpaper: "mocha-wall.png".to_string(),
            active: false,
        },
        theme_::ThemeEntry {
            name: "tokyo-night".to_string(),
            wallpaper: "tokyo.png".to_string(),
            active: true,
        },
    ]
}

fn fixture_menu() -> Menu {
    let launchers =
        center::launchers_tab_from_entries(&launch::scan_dirs(&[launch_fixtures_dir()]));
    let networks = center::networks_tab_from(
        Some(&fixture("nmcli-devices.txt")),
        Some(&fixture("nmcli-wifi.txt")),
    );
    let connected = fixture("bt-info-connected.txt");
    let disconnected = fixture("bt-info-disconnected.txt");
    let bluetooth = center::bluetooth_tab_from(Some(&fixture("bt-devices.txt")), &|mac| {
        if mac == "AA:BB:CC:DD:EE:FF" {
            Some(connected.clone())
        } else {
            Some(disconnected.clone())
        }
    });
    let settings = center::settings_tab_from(
        Some(&fixture("wpctl-50.txt")),
        Some("1200\n"),
        Some("2400\n"),
        &fixture_themes(),
    );
    flex_rice::menu(
        center::PROVIDER,
        vec![
            launchers,
            networks,
            bluetooth,
            center::power_tab(),
            settings,
        ],
    )
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn rune(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

fn alt_rune(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}

fn ctrl_rune(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Replay setup keystrokes that must be consumed (navigation, typing).
fn tap(menu: &mut Menu, keys: &[KeyEvent], base: std::time::Instant) {
    assert_eq!(run::replay_keys(menu, keys, base), KeyOutcome::Consumed);
}

/// Process-env guard: restores overwritten vars on drop so parallel
/// suites never observe leaked seams (see B-007).
struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn set(pairs: &[(&str, &str)]) -> Self {
        let mut saved = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            saved.push(((*key).to_string(), std::env::var(key).ok()));
            std::env::set_var(key, value);
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prev) in self.saved.drain(..) {
            match prev {
                Some(value) => std::env::set_var(&key, value),
                None => std::env::remove_var(&key),
            }
        }
    }
}

// --- Provider fixtures per tab ------------------------------------------------

#[test]
fn tab_order_and_names_match_bash_add_tab_order() {
    let menu = fixture_menu();
    assert_eq!(menu.provider, center::PROVIDER);
    let names: Vec<&str> = menu.app.tabs.iter().map(|tab| tab.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Launchers", "Networks", "Bluetooth", "Power", "Settings"]
    );
    // Only Settings opts into `m` → Toggle (volume mute); the rest keep
    // the legacy mark/Delete behavior.
    let deletable: Vec<bool> = menu.app.tabs.iter().map(|tab| tab.deletable).collect();
    assert_eq!(deletable, vec![false, false, false, false, true]);
    assert!(
        menu.app.tabs[1..].iter().all(|tab| tab.bare_rows),
        "center non-Launchers tabs are bare"
    );
}

#[test]
fn launchers_reuse_desktop_scan_with_kind_prefixed_ids() {
    let dirs = [launch_fixtures_dir()];
    let tab = center::launchers_tab_from_entries(&launch::scan_dirs(&dirs));
    assert_eq!(tab.rows.len(), 4, "same survivor set as flex launch");
    let first = &tab.rows[0];
    assert_eq!(first.label, "Firefox");
    assert_eq!(first.meta, None);
    let htop = &tab.rows[1];
    assert_eq!(htop.meta.as_deref(), Some(launch::TERMINAL_META));
    // `launch:` + the space-free row hash from `launch::rows` (B-021); the
    // wrapper resolves the hash back to a desktop-id before launching.
    for (row, desktop_id) in tab.rows.iter().zip([
        "firefox.desktop",
        "terminal-app.desktop",
        "onlyshow-app.desktop",
        "percent-app.desktop",
    ]) {
        let hash = row
            .id
            .as_str()
            .strip_prefix("launch:")
            .expect("kind prefix");
        assert!(
            !hash.contains(char::is_whitespace),
            "hash part is one token: {hash:?}"
        );
        assert_eq!(
            launch::resolve_id_in(&dirs, hash).as_deref(),
            Some(desktop_id),
            "the kind-prefixed id resolves back to its desktop-id"
        );
    }
}

#[test]
fn empty_launchers_keep_bash_parenthetical() {
    let tab = center::launchers_tab_from_entries(&[]);
    assert_eq!(tab.rows.len(), 1);
    assert_eq!(tab.rows[0].id.as_str(), center::NOOP_ID);
    assert_eq!(tab.rows[0].label, "(No applications found)");
}

#[test]
fn networks_rows_match_bash_metas_on_reference_data() {
    let tab = center::networks_tab_from(
        Some(&fixture("nmcli-devices.txt")),
        Some(&fixture("nmcli-wifi.txt")),
    );
    assert_eq!(tab.name, center::TAB_NETWORKS);
    let rows: Vec<(&str, Option<&str>)> = tab
        .rows
        .iter()
        .map(|row| (row.label.as_str(), row.meta.as_deref()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("HomeNet", Some("◇  87% [███████░] 🔒 WPA2  Connected")),
            ("Corp:Net", Some("◇  63% [█████░░░] 🔒 WPA2")),
            ("Coffee Shop", Some("◇  41% [███░░░░░] 🔓 Open")),
            ("Lobby", Some("◇  5% [░░░░░░░░] 🔒 WPA3")),
        ],
        "colon SSID survives; bars hand-computed from flex_bar"
    );
    assert!(
        tab.rows
            .iter()
            .all(|row| row.id.as_str() == center::WIFI_ID),
        "SSID travels in the label (spaces break id tokens)"
    );
}

#[test]
fn networks_offline_when_no_interface_or_failed_command() {
    for tab in [
        center::networks_tab_from(
            Some(&fixture("nmcli-devices-nowifi.txt")),
            Some(&fixture("nmcli-wifi.txt")),
        ),
        center::networks_tab_from(None, None),
        center::networks_tab_from(Some("garbage without colons\n"), None),
    ] {
        assert_eq!(tab.rows.len(), 1);
        assert_eq!(tab.rows[0].label, center::OFFLINE_LABEL);
        assert!(tab.rows[0].offline, "offline rows render dim");
        assert_eq!(tab.rows[0].id.as_str(), center::NOOP_ID);
    }
}

#[test]
fn networks_empty_scan_keeps_bash_parenthetical() {
    let tab = center::networks_tab_from(Some(&fixture("nmcli-devices.txt")), Some(""));
    assert_eq!(tab.rows.len(), 1);
    assert_eq!(tab.rows[0].label, "(No Wi-Fi networks)");
    assert!(!tab.rows[0].offline);
}

#[test]
fn bluetooth_rows_carry_mac_and_connected_status() {
    let connected = fixture("bt-info-connected.txt");
    let disconnected = fixture("bt-info-disconnected.txt");
    let tab = center::bluetooth_tab_from(Some(&fixture("bt-devices.txt")), &|mac| {
        if mac == "AA:BB:CC:DD:EE:FF" {
            Some(connected.clone())
        } else {
            Some(disconnected.clone())
        }
    });
    let rows: Vec<(&str, &str, Option<&str>)> = tab
        .rows
        .iter()
        .map(|row| (row.id.as_str(), row.label.as_str(), row.meta.as_deref()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "bt:AA:BB:CC:DD:EE:FF",
                "Headphones",
                Some("AA:BB:CC:DD:EE:FF  Connected")
            ),
            (
                "bt:11:22:33:44:55:66",
                "Speaker One",
                Some("11:22:33:44:55:66")
            ),
        ],
        "Controller line ignored; names keep spaces"
    );
}

#[test]
fn bluetooth_offline_or_empty_states() {
    let offline = center::bluetooth_tab_from(None, &|_| None);
    assert_eq!(offline.rows.len(), 1);
    assert_eq!(offline.rows[0].label, center::OFFLINE_LABEL);
    assert!(offline.rows[0].offline);
    let empty = center::bluetooth_tab_from(Some(""), &|_| None);
    assert_eq!(empty.rows[0].label, "(No paired devices)");
    assert!(!empty.rows[0].offline);
    // Failed per-device probes degrade to disconnected, like bash.
    let tab = center::bluetooth_tab_from(Some(&fixture("bt-devices.txt")), &|_| None);
    assert!(
        tab.rows.iter().all(|row| row
            .meta
            .as_deref()
            .is_some_and(|meta| !meta.contains("Connected"))),
        "no Connected suffix without a successful probe"
    );
}

#[test]
fn settings_gauge_labels_match_bash_renderer() {
    let tab = center::settings_tab_from(
        Some(&fixture("wpctl-55.txt")),
        Some("1200\n"),
        Some("2400\n"),
        &fixture_themes(),
    );
    assert!(tab.deletable, "Settings opts into m → Toggle for mute");
    let rows: Vec<(&str, Option<&str>)> = tab
        .rows
        .iter()
        .map(|row| (row.id.as_str(), row.meta.as_deref()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("vol", Some("sink")),
            ("bright", Some("backlight")),
            ("theme", Some("Theme")),
            ("theme", Some("Theme  Active")),
        ]
    );
    assert_eq!(tab.rows[0].label, "Volume  55%  [██████░░░░]");
    assert_eq!(tab.rows[1].label, "Brightness  50%  [█████░░░░░]");
    assert_eq!(tab.rows[2].label, "Theme: mocha");
    assert_eq!(tab.rows[3].label, "Theme: tokyo-night");
}

#[test]
fn settings_muted_and_offline_gauge_states() {
    let muted = center::settings_tab_from(Some(&fixture("wpctl-muted.txt")), None, None, &[]);
    assert_eq!(muted.rows[0].label, "Volume  0%  [░░░░░░░░░░]  Muted");
    assert_eq!(muted.rows[0].meta.as_deref(), Some("sink"));
    let offline = center::settings_tab_from(None, None, None, &[]);
    assert_eq!(offline.rows.len(), 2, "gauge slots persist when offline");
    assert_eq!(offline.rows[0].id.as_str(), center::VOL_ID);
    assert_eq!(offline.rows[0].label, "Volume — offline");
    assert!(offline.rows[0].offline);
    assert_eq!(offline.rows[1].id.as_str(), center::BRIGHT_ID);
    assert_eq!(offline.rows[1].label, "Brightness — offline");
    assert!(offline.rows[1].offline);
}

// --- Golden (125x30 wide viewport) ----------------------------------------------

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
            let symbol_w = width::str_width(cell.symbol());
            total += symbol_w;
            x += u16::try_from(symbol_w.max(1)).expect("row width fits u16");
        }
        assert_eq!(total, usize::from(w), "row {y} must be exactly {w} cells");
    }
}

#[test]
fn center_default_view_golden_at_125x30() {
    let mut menu = fixture_menu();
    let buf = draw(&mut menu, 125, 30);
    assert_full_width(&buf, 125, 30);
    // Tab bar is at bottom (wiremix layout)
    let tab_bar = row_text(&buf, 29, 125);
    for tab in ["Launchers", "Networks", "Bluetooth", "Power", "Settings"] {
        assert!(tab_bar.contains(tab), "tab bar shows {tab:?}: {tab_bar:?}");
    }
    let mut all = String::new();
    for y in 0..30 {
        all.push_str(&row_text(&buf, y, 125));
    }
    for token in ["Firefox", "Terminal", "filter", "navigate"] {
        assert!(all.contains(token), "125x30 contains {token:?}");
    }
    // Selection indicator: top selector char on col 0 of the first node row,
    // which sits below the reserved `•••` indicator line.
    let first_y = flex_core::render::LIST_INDICATOR_ROWS / 2;
    let selector_cell = buf.cell((0, first_y)).expect("first list row selector");
    assert_eq!(selector_cell.symbol(), "░");
    assert_eq!(
        selector_cell.fg,
        flex_core::theme::Theme::DEFAULT
            .selector
            .fg
            .expect("selector sets a fg")
    );
}

#[test]
fn gauge_states_golden_zero_half_full_muted_offline() {
    // 0% / 50% / 100% volume rows render exact bash labels + bars.
    for (name, pct_label) in [
        ("wpctl-0.txt", "Volume  0%  [░░░░░░░░░░]"),
        ("wpctl-50.txt", "Volume  50%  [█████░░░░░]"),
        ("wpctl-100.txt", "Volume  100%  [██████████]"),
    ] {
        let mut menu = flex_rice::menu(
            center::PROVIDER,
            vec![center::settings_tab_from(
                Some(&fixture(name)),
                Some("1200\n"),
                Some("2400\n"),
                &[],
            )],
        );
        menu.app.switch_tab(0);
        let buf = draw(&mut menu, 125, 30);
        assert_full_width(&buf, 125, 30);
        let mut all = String::new();
        for y in 0..30 {
            all.push_str(&row_text(&buf, y, 125));
        }
        assert!(all.contains(pct_label), "{name} renders {pct_label:?}");
        assert!(
            all.contains("Brightness  50%  [█████░░░░░]"),
            "{name} keeps the brightness row"
        );
    }
    // Muted appends the bash `$status` suffix.
    let mut menu = flex_rice::menu(
        center::PROVIDER,
        vec![center::settings_tab_from(
            Some(&fixture("wpctl-muted.txt")),
            Some("1200\n"),
            Some("2400\n"),
            &[],
        )],
    );
    let buf = draw(&mut menu, 125, 30);
    let mut all = String::new();
    for y in 0..30 {
        all.push_str(&row_text(&buf, y, 125));
    }
    assert!(
        all.contains("Volume  0%  [░░░░░░░░░░]  Muted"),
        "muted state"
    );
    // Offline gauge slots render dim `— offline` (style spot-check).
    let mut menu = flex_rice::menu(
        center::PROVIDER,
        vec![center::settings_tab_from(None, None, None, &[])],
    );
    let buf = draw(&mut menu, 125, 30);
    assert_full_width(&buf, 125, 30);
    let mut offline_row = None;
    for y in 0..30 {
        if row_text(&buf, y, 125).contains("Volume — offline") {
            offline_row = Some(y);
        }
    }
    let y = offline_row.expect("offline volume row renders");
    // Labels start at col 4 (col 2 is the `◇` marker column, col 3 a space).
    let label_cell = buf.cell((4, y)).expect("label cell");
    assert_eq!(label_cell.symbol(), "V", "first label glyph");
    assert_eq!(
        label_cell.fg,
        flex_core::theme::Theme::DEFAULT
            .offline
            .fg
            .expect("offline sets a fg"),
        "offline rows render dim"
    );
}

// --- Key-seq replays ---------------------------------------------------------------

#[test]
fn tab_and_digit_navigation_across_five_tabs() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    // Tab cycles 0→1→…→4→0.
    for expected in [1, 2, 3, 4, 0] {
        let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Tab)], base);
        assert_eq!(outcome, KeyOutcome::Consumed);
        assert_eq!(menu.app.active, expected);
    }
    // Bare digit switches iff the filter is empty (Q1).
    let outcome = run::replay_keys(&mut menu, &[rune('3')], base);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(menu.app.active, 2, "'3' jumps to Bluetooth");
    // Non-empty filter: digits type instead of switching.
    let outcome = run::replay_keys(&mut menu, &[rune('x'), rune('4')], base);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(menu.app.active, 2, "digits type into a non-empty filter");
    assert_eq!(
        menu.app.active_tab().expect("tab").state.filter,
        "x4",
        "both digits landed in the filter"
    );
    // Alt-digit always switches, even with filter text (Q1).
    let outcome = run::replay_keys(&mut menu, &[alt_rune('5')], base);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(menu.app.active, 4, "Alt-5 jumps to Settings");
}

#[test]
fn filter_and_focus_survive_tab_switches() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    tap(&mut menu, &[rune('f'), rune('i')], base);
    tap(&mut menu, &[press(KeyCode::Down)], base);
    assert_eq!(menu.app.active_tab().expect("tab").state.filter, "fi");
    tap(&mut menu, &[press(KeyCode::Tab)], base);
    tap(&mut menu, &[rune('z'), rune('z')], base);
    assert_eq!(menu.app.active, 1);
    tap(&mut menu, &[press(KeyCode::BackTab)], base);
    assert_eq!(menu.app.active, 0);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.filter,
        "fi",
        "per-tab filter preserved"
    );
    tap(&mut menu, &[press(KeyCode::Tab)], base);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.filter,
        "zz",
        "sibling filter preserved too"
    );
}

#[test]
fn gauge_tick_soak_60_ticks_preserve_state_and_refresh_values() {
    let _env = ENV_LOCK.lock().expect("env lock");
    let dir = std::env::temp_dir().join(format!("flex-center-soak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("soak dir");
    let vol = dir.join("volume");
    let cur = dir.join("cur");
    let max = dir.join("max");
    std::fs::write(&vol, fixture("wpctl-55.txt")).expect("vol fixture");
    std::fs::write(&cur, "1200\n").expect("cur fixture");
    std::fs::write(&max, "2400\n").expect("max fixture");
    let _guard = EnvGuard::set(&[
        (center::WPCTL_FILE_ENV, vol.to_str().expect("utf8")),
        (center::BRIGHT_CUR_FILE_ENV, cur.to_str().expect("utf8")),
        (center::BRIGHT_MAX_FILE_ENV, max.to_str().expect("utf8")),
    ]);
    // Production constructor (live commands would run without the seams).
    let mut menu = flex_rice::menu(center::PROVIDER, vec![center::settings_tab()]);
    let tab = menu.app.active_tab().expect("settings tab");
    assert_eq!(tab.rows[0].id.as_str(), center::VOL_ID);
    assert_eq!(tab.rows[1].id.as_str(), center::BRIGHT_ID);
    // Arbitrary interaction state a tick must never disturb (R7).
    menu.app.active_tab_mut().expect("tab").state.filter = "vol".to_string();
    menu.app.active_tab_mut().expect("tab").state.focus = 1;
    menu.app.active_tab_mut().expect("tab").state.scroll = 1;
    let snapshot_rows = |menu: &Menu| {
        menu.app
            .active_tab()
            .expect("tab")
            .rows
            .iter()
            .map(|row| {
                (
                    row.id.as_str().to_string(),
                    row.label.clone(),
                    row.meta.clone(),
                    row.offline,
                )
            })
            .collect::<Vec<_>>()
    };
    let before = snapshot_rows(&menu);
    assert_eq!(before.len(), menu.app.active_tab().expect("tab").rows.len());
    let base = run::test_base();
    for tick in 0_u32..60 {
        // Same stamping shape as the event loop; no key events involved.
        menu.tick(base + run::FLEX_TEST_STEP * tick);
    }
    let state = &menu.app.active_tab().expect("tab").state;
    assert_eq!(state.filter, "vol", "tick never clears the filter");
    assert_eq!(state.focus, 1, "tick never moves focus");
    assert_eq!(state.scroll, 1, "tick never resets scroll");
    assert_eq!(
        snapshot_rows(&menu),
        before,
        "steady snapshots never rewrite rows"
    );
    // Fresh values flow through on the next tick, state still intact.
    std::fs::write(&vol, fixture("wpctl-muted.txt")).expect("mute fixture");
    menu.tick(base + run::FLEX_TEST_STEP * 61);
    let rows = &menu.app.active_tab().expect("tab").rows;
    assert!(
        rows[0].label.contains("Muted"),
        "tick refreshes gauge values: {:?}",
        rows[0].label
    );
    assert_eq!(rows[1].label, "Brightness  50%  [█████░░░░░]");
    let state = &menu.app.active_tab().expect("tab").state;
    assert_eq!(
        (state.filter.as_str(), state.focus, state.scroll),
        ("vol", 1, 1)
    );
    // Command failure flips the slot offline in place (no rebuild).
    std::fs::write(&cur, "bogus\n").expect("broken fixture");
    menu.tick(base + run::FLEX_TEST_STEP * 62);
    let rows = &menu.app.active_tab().expect("tab").rows;
    assert_eq!(
        rows.len(),
        before.len(),
        "row count stable across transitions"
    );
    assert_eq!(rows[1].id.as_str(), center::BRIGHT_ID, "slot id stable");
    assert_eq!(rows[1].label, "Brightness — offline");
    assert!(rows[1].offline);
    // Recovery reclaims the same slot.
    std::fs::write(&cur, "1200\n").expect("restore fixture");
    menu.tick(base + run::FLEX_TEST_STEP * 63);
    let rows = &menu.app.active_tab().expect("tab").rows;
    assert_eq!(rows[1].label, "Brightness  50%  [█████░░░░░]");
    assert!(!rows[1].offline);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gauge_tick_skips_snapshots_off_the_settings_tab() {
    // B-025 cheap guard: the ~11 ms of gauge spawns is only paid where it
    // is visible. While another tab is active the Settings rows keep their
    // values even when the seams would flip them offline; switching back
    // picks the fresh values up within one tick (≤1 s staleness).
    let _env = ENV_LOCK.lock().expect("env lock");
    let dir = std::env::temp_dir().join(format!("flex-center-guard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("guard dir");
    let vol = dir.join("volume");
    let cur = dir.join("cur");
    let max = dir.join("max");
    std::fs::write(&vol, fixture("wpctl-55.txt")).expect("vol fixture");
    std::fs::write(&cur, "1200\n").expect("cur fixture");
    std::fs::write(&max, "2400\n").expect("max fixture");
    let _guard = EnvGuard::set(&[
        (center::WPCTL_FILE_ENV, vol.to_str().expect("utf8")),
        (center::BRIGHT_CUR_FILE_ENV, cur.to_str().expect("utf8")),
        (center::BRIGHT_MAX_FILE_ENV, max.to_str().expect("utf8")),
    ]);
    let mut menu = flex_rice::menu(
        center::PROVIDER,
        vec![center::power_tab(), center::settings_tab()],
    );
    assert_eq!(menu.app.active, 0, "power tab active");
    let online: Vec<(String, String)> = menu.app.tabs[1]
        .rows
        .iter()
        .map(|row| (row.id.as_str().to_string(), row.label.clone()))
        .collect();
    // Seams now force failure; the tick must not read them off-tab.
    std::env::set_var(center::WPCTL_FILE_ENV, "");
    std::env::set_var(center::BRIGHT_CUR_FILE_ENV, "");
    let base = run::test_base();
    menu.tick(base);
    let kept: Vec<(String, String)> = menu.app.tabs[1]
        .rows
        .iter()
        .map(|row| (row.id.as_str().to_string(), row.label.clone()))
        .collect();
    assert_eq!(kept, online, "off-tab tick leaves Settings rows alone");
    assert!(
        menu.app.tabs[1].rows.iter().all(|row| !row.offline),
        "off-tab tick never flips slots offline"
    );
    // Switching to Settings re-arms the refresh on the next tick.
    menu.app.active = 1;
    menu.tick(base + run::FLEX_TEST_STEP);
    assert_eq!(menu.app.tabs[1].rows[0].label, "Volume — offline");
    assert!(menu.app.tabs[1].rows[0].offline);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn toggle_flow_mutes_volume_on_deletable_settings_only() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    // Settings tab (index 4): NAVIGATE `m` on Volume emits Toggle (mute).
    tap(&mut menu, &[rune('5')], base);
    assert_eq!(menu.app.active, 4);
    let outcome = run::replay_keys(&mut menu, &[ctrl_rune('o'), rune('m')], base);
    assert_eq!(outcome, KeyOutcome::Toggle, "NAVIGATE m → Toggle");
    assert_eq!(
        menu.app.focused_row().expect("row").id.as_str(),
        center::VOL_ID
    );
    // NORMAL-mode `m` types into the filter instead.
    let mut menu = fixture_menu();
    tap(&mut menu, &[rune('5')], base);
    let outcome = run::replay_keys(&mut menu, &[rune('m')], base);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.filter,
        "m",
        "NORMAL m filters (Q: mute needs NAVIGATE or Enter)"
    );
    // Launchers tab is non-deletable: NAVIGATE `m` marks in place.
    let outcome = run::replay_keys(&mut menu, &[rune('1'), ctrl_rune('o'), rune('m')], base);
    assert_eq!(
        outcome,
        KeyOutcome::Consumed,
        "no Toggle off deletable tabs"
    );
}

#[test]
fn confirmable_arm_confirms_reboot_on_second_enter() {
    let mut menu = fixture_menu();
    // Power tab (index 3), Reboot row (index 2, danger).
    tap(&mut menu, &[rune('4')], run::test_base());
    assert_eq!(menu.app.active, 3);
    tap(
        &mut menu,
        &[press(KeyCode::Down), press(KeyCode::Down)],
        run::test_base(),
    );
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "pwreboot");
    let t0 = run::test_base();
    // First Enter arms (never selects).
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Enter), t0),
        KeyOutcome::Consumed
    );
    assert!(menu.app.is_armed());
    // Early second Enter is swallowed (hold protection), stays armed.
    assert_eq!(
        handle_key(
            &mut menu,
            press(KeyCode::Enter),
            t0 + std::time::Duration::from_millis(10)
        ),
        KeyOutcome::Consumed
    );
    assert!(menu.app.is_armed());
    // Mature second Enter confirms (wrapper runs `systemctl reboot`).
    assert_eq!(
        handle_key(
            &mut menu,
            press(KeyCode::Enter),
            t0 + std::time::Duration::from_secs(1)
        ),
        KeyOutcome::Select
    );
    // Plain rows still select on the first Enter.
    let mut menu = fixture_menu();
    tap(&mut menu, &[rune('4')], run::test_base());
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Enter)], run::test_base());
    assert_eq!(outcome, KeyOutcome::Select, "Lock Screen selects at once");
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "pwlock");
}

#[test]
fn esc_disarms_confirmable_and_delete_never_fires_on_power() {
    let mut menu = fixture_menu();
    tap(&mut menu, &[rune('4')], run::test_base());
    tap(
        &mut menu,
        &[press(KeyCode::Down), press(KeyCode::Down)],
        run::test_base(),
    );
    let t0 = run::test_base();
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Enter), t0),
        KeyOutcome::Consumed
    );
    assert!(menu.app.is_armed());
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Esc), t0),
        KeyOutcome::Consumed
    );
    assert!(!menu.app.is_armed(), "Esc disarms");
    // Next Enter re-arms instead of confirming.
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Enter), t0),
        KeyOutcome::Consumed
    );
    assert!(menu.app.is_armed());
    // Delete is dead on the non-deletable power tab.
    let mut menu = fixture_menu();
    tap(&mut menu, &[rune('4')], run::test_base());
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Delete), press(KeyCode::Delete)],
        run::test_base(),
    );
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert!(!menu.app.active_state().expect("state").confirm_pending);
    // Cancelling still exits 130 with no ACTION:.
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], run::test_base());
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
}

// --- Live-builder smoke (fixture seams, no real commands) --------------------------

#[test]
fn live_builders_honor_fixture_seams() {
    let _env = ENV_LOCK.lock().expect("env lock");
    let dir = fixtures_dir();
    let _guard = EnvGuard::set(&[
        (
            center::NMCLI_DEVICES_FILE_ENV,
            dir.join("nmcli-devices.txt").to_str().expect("utf8"),
        ),
        (
            center::NMCLI_WIFI_FILE_ENV,
            dir.join("nmcli-wifi.txt").to_str().expect("utf8"),
        ),
        (
            center::BT_DEVICES_FILE_ENV,
            dir.join("bt-devices.txt").to_str().expect("utf8"),
        ),
        (center::BT_INFO_DIR_ENV, ""),
        (
            center::WPCTL_FILE_ENV,
            dir.join("wpctl-55.txt").to_str().expect("utf8"),
        ),
        (
            center::BRIGHT_CUR_FILE_ENV,
            dir.join("brightness-cur-1200.txt").to_str().expect("utf8"),
        ),
        (
            center::BRIGHT_MAX_FILE_ENV,
            dir.join("brightness-max-2400.txt").to_str().expect("utf8"),
        ),
    ]);
    // Empty BT_INFO_DIR forces probe failure → all rows disconnected.
    let bluetooth = center::bluetooth_tab();
    assert_eq!(bluetooth.rows.len(), 2, "fixture devices, no probes");
    assert!(
        bluetooth.rows.iter().all(|row| !row.offline),
        "successful call is not offline"
    );
    let networks = center::networks_tab();
    assert_eq!(networks.rows.len(), 4, "fixture wifi list");
    assert_eq!(networks.rows[0].label, "HomeNet");
    let settings = center::settings_tab();
    assert!(settings.rows.len() >= 2, "at least the gauge slots");
    assert_eq!(settings.rows[0].label, "Volume  55%  [██████░░░░]");
    assert_eq!(settings.rows[1].label, "Brightness  50%  [█████░░░░░]");
}

#[test]
fn center_menu_smoke_has_five_tabs_without_panicking() {
    // Live system state (may be offline/empty here); only structure asserts.
    let menu = center::center_menu();
    assert_eq!(menu.provider, center::PROVIDER);
    assert_eq!(menu.app.tabs.len(), 5);
    let names: Vec<&str> = menu.app.tabs.iter().map(|tab| tab.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Launchers", "Networks", "Bluetooth", "Power", "Settings"]
    );
}

// --- Wrapper stub tests -----------------------------------------------------------------

fn stub_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flex-center-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    dir
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

fn flex_stub(dir: &Path, action_line: &str) {
    write_exe(
        &dir.join("flex"),
        &format!("#!/usr/bin/env bash\necho '{action_line}'\n"),
    );
}

/// Like [`flex_stub`], but also answers the `--resolve` lookup the wrapper
/// performs for launcher rows: the row hash is resolved to `desktop_id`.
///
/// The call is logged so a test can prove the wrapper resolved the hash
/// instead of treating it as a file name (B-021).
fn flex_resolving_stub(dir: &Path, action_line: &str, desktop_id: &str) {
    let log = dir.join("resolve.log");
    write_exe(
        &dir.join("flex"),
        &format!(
            "#!/usr/bin/env bash\nif [[ \"${{2:-}}\" == \"--resolve\" ]]; then\nprintf 'resolve %s\\n' \"${{3:-}}\" >> '{}'\nprintf '%s\\n' '{desktop_id}'\nexit 0\nfi\necho '{action_line}'\n",
            log.display()
        ),
    );
}

fn log_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    write_exe(&path, body);
    path
}

/// `nmcli` stub: interface + `$WIFI_LIST` answers, connect/disconnect
/// logged to `$STUB_LOG` (always succeeding).
const NMCLI_STUB: &str = r#"#!/usr/bin/env bash
printf 'nmcli %s\n' "$*" >> "$STUB_LOG"
case "$1" in
    -t) case "$3" in
        DEVICE,TYPE) printf 'wlan0:wifi\neth0:ethernet\n' ;;
        IN-USE,SSID,SIGNAL,SECURITY) cat "$WIFI_LIST" ;;
        *) echo "unexpected nmcli query: $*" >&2; exit 1 ;;
    esac ;;
    device) exit 0 ;;
    *) echo "unexpected nmcli: $*" >&2; exit 1 ;;
esac
"#;

/// `bluetoothctl` stub: `$BT_INFO` answers, connect/disconnect logged.
const BT_STUB: &str = r#"#!/usr/bin/env bash
printf 'bt %s\n' "$*" >> "$STUB_LOG"
case "$1" in
    info) cat "$BT_INFO" ;;
    connect|disconnect) exit 0 ;;
    *) echo "unexpected bt: $*" >&2; exit 1 ;;
esac
"#;

const LOG_STUB: &str = r#"#!/usr/bin/env bash
printf '%s %s\n' "$(basename "$0")" "$*" >> "$STUB_LOG"
exit 0
"#;

fn run_wrapper(dir: &Path, extra_env: &[(&str, PathBuf)]) -> std::process::Output {
    let wrapper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-center.sh");
    let mut cmd = std::process::Command::new("bash");
    cmd.arg(&wrapper).env(
        "PATH",
        format!(
            "{}:{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    // The popup re-exec is bind-path behavior; wrapper tests emulate the
    // in-popup half (stubbed HOME has no popup.sh).
    cmd.env("POPUP_KITTY", "1");
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.output().expect("run wrapper")
}

fn read_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

#[test]
fn wrapper_launches_gui_and_terminal_apps() {
    let dir = stub_dir("launch");
    let home = dir.join("home");
    let apps = home.join(".local/share/applications");
    std::fs::create_dir_all(&apps).expect("apps dir");
    std::fs::write(
        apps.join("firefox.desktop"),
        "[Desktop Entry]\nName=Firefox\nExec=firefox %U\nTerminal=false\n",
    )
    .expect("desktop entry");
    // Space-bearing file name: legal, and exactly what hashing the row id
    // makes survivable (B-021).
    std::fs::write(
        apps.join("Htop Terminal.desktop"),
        "[Desktop Entry]\nName=Htop\nExec=htop\nTerminal=true\n",
    )
    .expect("desktop entry");
    for name in ["setsid", "kitty"] {
        log_stub(&dir, name, LOG_STUB);
    }
    let log = dir.join("calls.log");
    // GUI app: setsid without kitty.
    flex_resolving_stub(
        &dir,
        "ACTION: center launch:0123456789abcdef Firefox",
        "firefox.desktop",
    );
    let output = run_wrapper(&dir, &[("HOME", home.clone()), ("STUB_LOG", log.clone())]);
    assert!(
        output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let logged = read_log(&dir);
    assert!(
        logged.contains("setsid -f firefox"),
        "strips %U: {logged:?}"
    );
    assert!(!logged.contains("kitty"), "GUI app skips kitty: {logged:?}");
    // Terminal app: setsid via kitty -e, desktop-id resolved from the hash.
    std::fs::remove_file(&log).expect("reset log");
    flex_resolving_stub(
        &dir,
        "ACTION: center launch:fedcba9876543210 Htop",
        "Htop Terminal.desktop",
    );
    let output = run_wrapper(&dir, &[("HOME", home), ("STUB_LOG", log)]);
    assert!(output.status.success());
    let logged = read_log(&dir);
    assert!(
        logged.contains("setsid -f kitty -e htop"),
        "kitty path: {logged:?}"
    );
    let resolved = std::fs::read_to_string(dir.join("resolve.log")).expect("resolve log");
    assert_eq!(
        resolved.lines().collect::<Vec<_>>(),
        ["resolve 0123456789abcdef", "resolve fedcba9876543210"],
        "each ACTION id is resolved, never used as a file name"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_connects_open_wifi_and_notifies() {
    let dir = stub_dir("wifi-open");
    log_stub(&dir, "notify-send", LOG_STUB);
    write_exe(&dir.join("nmcli"), NMCLI_STUB);
    let wifi_list = dir.join("wifi-list.txt");
    std::fs::write(&wifi_list, ":Coffee Shop:41:--\n").expect("wifi list");
    let log = dir.join("calls.log");
    // SSID with a space travels in the escaped label, not the id.
    flex_stub(&dir, "ACTION: center wifi Coffee Shop");
    let output = run_wrapper(
        &dir,
        &[
            ("STUB_LOG", log),
            ("WIFI_LIST", wifi_list),
            ("NMCLI", dir.join("nmcli")),
        ],
    );
    assert!(
        output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let logged = read_log(&dir);
    assert!(
        logged.contains("nmcli device wifi connect Coffee Shop ifname wlan0"),
        "open connect: {logged:?}"
    );
    assert!(
        logged.contains("notify-send -a Control Center Connected Coffee Shop"),
        "connected notify: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_prompts_password_for_secure_wifi() {
    let dir = stub_dir("wifi-sec");
    log_stub(&dir, "notify-send", LOG_STUB);
    write_exe(&dir.join("nmcli"), NMCLI_STUB);
    let wifi_list = dir.join("wifi-list.txt");
    std::fs::write(&wifi_list, ":MyNet:70:WPA2\n").expect("wifi list");
    let log = dir.join("calls.log");
    flex_stub(&dir, "ACTION: center wifi MyNet");
    let output = run_wrapper(
        &dir,
        &[
            ("STUB_LOG", log),
            ("WIFI_LIST", wifi_list),
            ("NMCLI", dir.join("nmcli")),
            ("FLEX_CENTER_PASSWORD", PathBuf::from("s3cret")),
        ],
    );
    assert!(
        output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let logged = read_log(&dir);
    assert!(
        logged.contains("nmcli device wifi connect MyNet password s3cret ifname wlan0"),
        "password plumbed (no /dev/tty read): {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_toggles_bluetooth_connection() {
    let dir = stub_dir("bt");
    write_exe(&dir.join("bluetoothctl"), BT_STUB);
    let info = dir.join("info.txt");
    let log = dir.join("calls.log");
    // TOGGLE on a connected device disconnects.
    std::fs::write(&info, "Device X\n\tConnected: yes\n").expect("info");
    flex_stub(&dir, "ACTION:TOGGLE center bt:AA:BB:CC:DD:EE:FF Headphones");
    let output = run_wrapper(
        &dir,
        &[
            ("STUB_LOG", log.clone()),
            ("BT_INFO", info.clone()),
            ("BLUETOOTHCTL", dir.join("bluetoothctl")),
        ],
    );
    assert!(output.status.success());
    assert!(
        read_log(&dir).contains("bt disconnect AA:BB:CC:DD:EE:FF"),
        "toggle disconnects: {:?}",
        read_log(&dir)
    );
    // SELECT on a disconnected device connects.
    std::fs::remove_file(&log).expect("reset log");
    std::fs::write(&info, "Device X\n\tConnected: no\n").expect("info");
    flex_stub(&dir, "ACTION: center bt:11:22:33:44:55:66 Speaker");
    let output = run_wrapper(
        &dir,
        &[
            ("STUB_LOG", log),
            ("BT_INFO", info),
            ("BLUETOOTHCTL", dir.join("bluetoothctl")),
        ],
    );
    assert!(output.status.success());
    assert!(
        read_log(&dir).contains("bt connect 11:22:33:44:55:66"),
        "select connects: {:?}",
        read_log(&dir)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_toggles_volume_mute() {
    let dir = stub_dir("mute");
    write_exe(&dir.join("wpctl"), LOG_STUB);
    let log = dir.join("calls.log");
    // `m` in NAVIGATE arrives as TOGGLE; Enter on Volume arrives as SELECT.
    for action in [
        "ACTION:TOGGLE center vol Volume",
        "ACTION: center vol Volume  55%  [bar]",
    ] {
        std::fs::remove_file(&log).unwrap_or(());
        flex_stub(&dir, action);
        let output = run_wrapper(
            &dir,
            &[("STUB_LOG", log.clone()), ("WPCTL", dir.join("wpctl"))],
        );
        assert!(output.status.success(), "{action}");
        assert!(
            read_log(&dir).contains("wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"),
            "{action} mutes: {:?}",
            read_log(&dir)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_dispatches_power_and_theme() {
    let dir = stub_dir("power-theme");
    for name in ["systemctl", "hyprlock", "pkill", "notify-send"] {
        log_stub(&dir, name, LOG_STUB);
    }
    let switcher = dir.join("theme-switcher.sh");
    let log = dir.join("calls.log");
    write_exe(
        &switcher,
        "#!/usr/bin/env bash\nprintf 'switcher %s\\n' \"$*\" >> \"$STUB_LOG\"\n",
    );
    // Danger-confirmed Reboot arrives as a plain SELECT (keys armed it).
    flex_stub(&dir, "ACTION: center pwreboot Reboot");
    let output = run_wrapper(&dir, &[("STUB_LOG", log.clone())]);
    assert!(output.status.success());
    assert!(
        read_log(&dir).contains("systemctl reboot"),
        "power op: {:?}",
        read_log(&dir)
    );
    // Theme name travels in the `Theme: {name}` label.
    std::fs::remove_file(&log).expect("reset log");
    flex_stub(&dir, "ACTION: center theme Theme: tokyo-night");
    let output = run_wrapper(&dir, &[("STUB_LOG", log), ("THEME_SWITCHER", switcher)]);
    assert!(output.status.success());
    assert!(
        read_log(&dir).contains("switcher activate tokyo-night"),
        "theme dispatch: {:?}",
        read_log(&dir)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_noops_and_rejects() {
    let dir = stub_dir("noop");
    // noop SELECT and any DELETE exit 0 with no side effects.
    for action in [
        "ACTION: center noop (No Wi-Fi networks)",
        "ACTION:DELETE center vol Volume",
        "ACTION:TOGGLE center bright Brightness",
    ] {
        flex_stub(&dir, action);
        let output = run_wrapper(&dir, &[]);
        assert!(output.status.success(), "{action} exits 0");
    }
    assert!(
        !dir.join("calls.log").exists(),
        "no side-effect stub ever ran"
    );
    // Malformed ACTION: lines fail (sibling-wrapper contract).
    flex_stub(&dir, "GARBAGE LINE");
    let output = run_wrapper(&dir, &[]);
    assert!(!output.status.success(), "malformed ACTION: must fail");
    let _ = std::fs::remove_dir_all(&dir);
}
