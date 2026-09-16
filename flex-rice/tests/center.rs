//! `flex center` cutover tests (M5): provider fixtures per tab, the
//! 125x30 golden (5-tab bar + gauge states), key-seq replays (tab/digit
//! navigation, 60-tick soak, toggle flow, danger-arm on power), and the
//! executor action matrix.
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

// --- Shared stub helpers ----------------------------------------------------------------

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

fn read_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

// --- Executor tests (`exec::center`) -------------------------------------------------
//
// These drive the Rust port with per-tool logging stubs, byte-compared call
// logs and scratch `HOME`. Process-env mutations ride under `EXEC_ENV_LOCK`
// with the shared [`EnvGuard`] save/restore.

use flex_rice::exec::center as exec_center;

/// Serialises the executor/parity tests below: each mutates process env
/// (`HOME`, `NMCLI`, `FLEX_CENTER_PASSWORD`, …) and the harness runs tests
/// in parallel, so the mutations are held under one lock with restores in
/// [`EnvGuard`] (same pattern as the launch port's `EXEC_ENV_LOCK`).
static EXEC_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `PATH` shadow for executor calls: stub dir first, ambient `PATH` after
/// (stubs are `bash` scripts, and the wrapper side needs real `grep`/`awk`).
fn exec_path_env(dir: &Path) -> String {
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// Scratch `HOME` with an applications dir (plain + `Terminal=true` apps)
/// and a logging theme-switcher at the wrapper-default path. The switcher
/// logs through `$STUB_LOG` like every other stub, so matrix and parity
/// tests compare one shared call log.
fn install_center_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("flex-center-exec-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let apps = home.join(".local/share/applications");
    std::fs::create_dir_all(&apps).expect("apps dir");
    std::fs::write(
        apps.join("firefox.desktop"),
        "[Desktop Entry]\nName=Firefox\nExec=firefox %U\nTerminal=false\n",
    )
    .expect("plain entry");
    std::fs::write(
        apps.join("termapp.desktop"),
        "[Desktop Entry]\nName=Htop\nExec=htop\nTerminal=true\n",
    )
    .expect("terminal entry");
    let switcher = home.join(".config/scripts/theme-switcher.sh");
    std::fs::create_dir_all(switcher.parent().expect("switcher parent")).expect("switcher dir");
    write_exe(
        &switcher,
        "#!/usr/bin/env bash\nprintf 'switcher %s\\n' \"$*\" >> \"$STUB_LOG\"\n",
    );
    home
}

/// Full action matrix through one shared call log: launch (plain + the
/// verbatim `kitty` branch), mute, bt connect/disconnect on fresh `info`
/// state, wifi open connect, all five power arms, theme, and the quiet
/// `bright`/`noop` rows. Byte-compared against the wrapper's shapes.
#[test]
fn executor_matrix_runs_every_action_through_stub_tools() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-matrix");
    for name in [
        "setsid",
        "wpctl",
        "systemctl",
        "hyprlock",
        "pkill",
        "notify-send",
    ] {
        log_stub(&dir, name, LOG_STUB);
    }
    write_exe(&dir.join("nmcli"), NMCLI_STUB);
    write_exe(&dir.join("bluetoothctl"), BT_STUB);
    let wifi_list = dir.join("wifi-list.txt");
    std::fs::write(&wifi_list, ":Coffee Shop:41:--\n").expect("wifi list");
    let info = dir.join("info.txt");
    let log = dir.join("calls.log");
    let home = install_center_home("matrix");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("NMCLI", dir.join("nmcli").to_str().expect("utf8")),
        (
            "BLUETOOTHCTL",
            dir.join("bluetoothctl").to_str().expect("utf8"),
        ),
        ("WPCTL", dir.join("wpctl").to_str().expect("utf8")),
        ("THEME_SWITCHER", ""),
        ("FLEX_CENTER_PASSWORD", ""),
        ("STUB_LOG", log.to_str().expect("utf8")),
        ("WIFI_LIST", wifi_list.to_str().expect("utf8")),
        ("BT_INFO", info.to_str().expect("utf8")),
    ]);
    let path_env = exec_path_env(&dir);
    let select = exec_center::CenterOp::Select;
    let run = |op: exec_center::CenterOp, id: &str, label: &str| {
        exec_center::execute(op, id, label, Some(&path_env)).expect("executor succeeds")
    };
    // Launcher rows: the hash resolves in-process (no `flex --resolve`).
    let firefox_id = format!("launch:{}", launch::entry_id("firefox.desktop"));
    let term_id = format!("launch:{}", launch::entry_id("termapp.desktop"));
    assert_eq!(
        run(select, &firefox_id, "Firefox").detail.as_deref(),
        Some("firefox.desktop")
    );
    assert_eq!(
        run(select, &term_id, "Htop").detail.as_deref(),
        Some("termapp.desktop")
    );
    run(select, "vol", "Volume  55%  [bar]");
    std::fs::write(&info, "Device X\n\tConnected: yes\n").expect("info");
    run(select, "bt:AA:BB:CC:DD:EE:FF", "Headphones");
    std::fs::write(&info, "Device X\n\tConnected: no\n").expect("info");
    run(
        exec_center::CenterOp::Toggle,
        "bt:11:22:33:44:55:66",
        "Speaker",
    );
    run(select, "wifi", "Coffee Shop");
    for (id, detail) in [
        ("pwlock", "pwlock"),
        ("pwsuspend", "pwsuspend"),
        ("pwreboot", "pwreboot"),
        ("pwoff", "pwoff"),
        ("pwlogout", "pwlogout"),
    ] {
        assert_eq!(run(select, id, id).detail.as_deref(), Some(detail));
    }
    assert_eq!(
        run(select, "theme", "Theme: tokyo-night").detail.as_deref(),
        Some("tokyo-night")
    );
    // Quiet rows: no steps, still `Ok`.
    for id in ["bright", "noop"] {
        let report = run(select, id, id);
        assert_eq!(report.detail, None, "{id} carries no detail");
    }
    assert_eq!(
        read_log(&dir).lines().collect::<Vec<_>>(),
        vec![
            "setsid -f firefox",
            "setsid -f kitty -e htop",
            "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle",
            "bt info AA:BB:CC:DD:EE:FF",
            "bt disconnect AA:BB:CC:DD:EE:FF",
            "bt info 11:22:33:44:55:66",
            "bt connect 11:22:33:44:55:66",
            "nmcli -t -f DEVICE,TYPE device",
            "nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname wlan0",
            "nmcli device wifi connect Coffee Shop ifname wlan0",
            "notify-send -a Control Center Connected Coffee Shop",
            // `hyprlock` takes no args; `LOG_STUB` joins `name + space +
            // args` unconditionally, so the zero-arg call keeps its space.
            "hyprlock ",
            "systemctl suspend",
            "systemctl reboot",
            "systemctl poweroff",
            "pkill -SIGTERM Hyprland",
            "switcher activate tokyo-night",
        ],
        "one tool sequence per action (describe: one line per step)",
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// Secure wifi uses the stored password (no `/dev/tty` read) and notifies.
#[test]
fn executor_wifi_secure_uses_the_stored_password() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-wifi-sec");
    log_stub(&dir, "notify-send", LOG_STUB);
    write_exe(&dir.join("nmcli"), NMCLI_STUB);
    let wifi_list = dir.join("wifi-list.txt");
    std::fs::write(&wifi_list, ":MyNet:70:WPA2\n").expect("wifi list");
    let log = dir.join("calls.log");
    let home = install_center_home("wifi-sec");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("NMCLI", dir.join("nmcli").to_str().expect("utf8")),
        ("FLEX_CENTER_PASSWORD", "s3cret"),
        ("STUB_LOG", log.to_str().expect("utf8")),
        ("WIFI_LIST", wifi_list.to_str().expect("utf8")),
    ]);
    let report = exec_center::execute(
        exec_center::CenterOp::Select,
        "wifi",
        "MyNet",
        Some(&exec_path_env(&dir)),
    )
    .expect("secure connect succeeds");
    assert_eq!(report.detail.as_deref(), Some("MyNet"));
    assert_eq!(
        read_log(&dir).lines().collect::<Vec<_>>(),
        vec![
            "nmcli -t -f DEVICE,TYPE device",
            "nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname wlan0",
            "nmcli device wifi connect MyNet password s3cret ifname wlan0",
            "notify-send -a Control Center Connected MyNet",
        ],
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// A connected row disconnects (fresh `IN-USE` state, unconditional notify).
#[test]
fn executor_wifi_connected_disconnects() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-wifi-disc");
    log_stub(&dir, "notify-send", LOG_STUB);
    write_exe(&dir.join("nmcli"), NMCLI_STUB);
    let wifi_list = dir.join("wifi-list.txt");
    std::fs::write(&wifi_list, "*:HomeNet:87:WPA2\n").expect("wifi list");
    let log = dir.join("calls.log");
    let home = install_center_home("wifi-disc");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("NMCLI", dir.join("nmcli").to_str().expect("utf8")),
        ("FLEX_CENTER_PASSWORD", ""),
        ("STUB_LOG", log.to_str().expect("utf8")),
        ("WIFI_LIST", wifi_list.to_str().expect("utf8")),
    ]);
    exec_center::execute(
        exec_center::CenterOp::Select,
        "wifi",
        "HomeNet",
        Some(&exec_path_env(&dir)),
    )
    .expect("disconnect succeeds");
    assert_eq!(
        read_log(&dir).lines().collect::<Vec<_>>(),
        vec![
            "nmcli -t -f DEVICE,TYPE device",
            "nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list ifname wlan0",
            "nmcli device disconnect wlan0",
            "notify-send -a Control Center Disconnected HomeNet",
        ],
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// No wifi interface (or a failing discovery) is a quiet no-op after the
/// single discovery call — the interface vanished since the snapshot.
#[test]
fn executor_wifi_without_interface_is_a_quiet_noop() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-wifi-noif");
    log_stub(&dir, "notify-send", LOG_STUB);
    write_exe(
        &dir.join("nmcli"),
        "#!/usr/bin/env bash\nprintf 'nmcli %s\\n' \"$*\" >> \"$STUB_LOG\"\nprintf 'eth0:ethernet\\n'\n",
    );
    let log = dir.join("calls.log");
    let home = install_center_home("wifi-noif");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("NMCLI", dir.join("nmcli").to_str().expect("utf8")),
        ("FLEX_CENTER_PASSWORD", ""),
        ("STUB_LOG", log.to_str().expect("utf8")),
    ]);
    let report = exec_center::execute(
        exec_center::CenterOp::Select,
        "wifi",
        "HomeNet",
        Some(&exec_path_env(&dir)),
    )
    .expect("vanished interface is Ok");
    assert_eq!(report.detail, None);
    assert_eq!(
        read_log(&dir).lines().collect::<Vec<_>>(),
        vec!["nmcli -t -f DEVICE,TYPE device"],
        "nothing past discovery",
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// A stale label (SSID gone from the scan) takes the open arm, like an
/// empty `$sec` in bash.
#[test]
fn executor_wifi_stale_label_treated_as_open() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-wifi-stale");
    log_stub(&dir, "notify-send", LOG_STUB);
    write_exe(&dir.join("nmcli"), NMCLI_STUB);
    let wifi_list = dir.join("wifi-list.txt");
    std::fs::write(&wifi_list, ":Other:50:WPA2\n").expect("wifi list");
    let log = dir.join("calls.log");
    let home = install_center_home("wifi-stale");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("NMCLI", dir.join("nmcli").to_str().expect("utf8")),
        ("FLEX_CENTER_PASSWORD", ""),
        ("STUB_LOG", log.to_str().expect("utf8")),
        ("WIFI_LIST", wifi_list.to_str().expect("utf8")),
    ]);
    exec_center::execute(
        exec_center::CenterOp::Select,
        "wifi",
        "Gone",
        Some(&exec_path_env(&dir)),
    )
    .expect("stale label is Ok");
    let logged = read_log(&dir);
    assert!(
        logged.contains("nmcli device wifi connect Gone ifname wlan0"),
        "stale label connects open: {logged:?}"
    );
    assert!(
        logged.contains("notify-send -a Control Center Connected Gone"),
        "success notifies: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// A failing open connect skips its notify (the wrapper's `if`, no `else`);
/// a failing secure connect notifies `Failed` instead of `Connected`.
#[test]
fn executor_wifi_failed_connect_gates_its_notify() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-wifi-fail");
    log_stub(&dir, "notify-send", LOG_STUB);
    write_exe(
        &dir.join("nmcli"),
        r#"#!/usr/bin/env bash
printf 'nmcli %s\n' "$*" >> "$STUB_LOG"
case "$1" in
    -t) case "$3" in
        DEVICE,TYPE) printf 'wlan0:wifi\n' ;;
        IN-USE,SSID,SIGNAL,SECURITY) cat "$WIFI_LIST" ;;
    esac ;;
    device) case "$2" in
        wifi) exit 3 ;;
        disconnect) exit 0 ;;
    esac ;;
esac
"#,
    );
    let wifi_list = dir.join("wifi-list.txt");
    std::fs::write(&wifi_list, ":Open:41:--\n:MyNet:70:WPA2\n").expect("wifi list");
    let log = dir.join("calls.log");
    let home = install_center_home("wifi-fail");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("NMCLI", dir.join("nmcli").to_str().expect("utf8")),
        ("FLEX_CENTER_PASSWORD", "s3cret"),
        ("STUB_LOG", log.to_str().expect("utf8")),
        ("WIFI_LIST", wifi_list.to_str().expect("utf8")),
    ]);
    let path_env = exec_path_env(&dir);
    // Open failure: connect logged, no notify at all (still `Ok` — quiet).
    exec_center::execute(
        exec_center::CenterOp::Select,
        "wifi",
        "Open",
        Some(&path_env),
    )
    .expect("failed open connect is quiet");
    let logged = read_log(&dir);
    assert!(
        logged.contains("nmcli device wifi connect Open ifname wlan0"),
        "connect attempted: {logged:?}"
    );
    assert!(
        !logged.contains("notify-send"),
        "no notify on failed open connect: {logged:?}"
    );
    // Secure failure: `Failed` notify, never `Connected`.
    std::fs::remove_file(&log).expect("reset log");
    exec_center::execute(
        exec_center::CenterOp::Select,
        "wifi",
        "MyNet",
        Some(&path_env),
    )
    .expect("failed secure connect is quiet");
    let logged = read_log(&dir);
    assert!(
        logged.contains("notify-send -a Control Center Failed MyNet"),
        "failure notifies: {logged:?}"
    );
    assert!(
        !logged.contains("Connected MyNet"),
        "no success notify on failure: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// Toggle dispatch: `vol` mutes, `bt:*` flips on fresh state, everything
/// else (bright/theme/launch/wifi/power/noop) is a quiet no-op.
#[test]
fn executor_toggle_matrix() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-toggle");
    for name in ["setsid", "wpctl", "systemctl", "notify-send"] {
        log_stub(&dir, name, LOG_STUB);
    }
    write_exe(&dir.join("nmcli"), NMCLI_STUB);
    write_exe(&dir.join("bluetoothctl"), BT_STUB);
    let wifi_list = dir.join("wifi-list.txt");
    std::fs::write(&wifi_list, ":MyNet:70:WPA2\n").expect("wifi list");
    let info = dir.join("info.txt");
    std::fs::write(&info, "Device X\n\tConnected: yes\n").expect("info");
    let log = dir.join("calls.log");
    let home = install_center_home("toggle");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("NMCLI", dir.join("nmcli").to_str().expect("utf8")),
        (
            "BLUETOOTHCTL",
            dir.join("bluetoothctl").to_str().expect("utf8"),
        ),
        ("WPCTL", dir.join("wpctl").to_str().expect("utf8")),
        ("FLEX_CENTER_PASSWORD", "s3cret"),
        ("STUB_LOG", log.to_str().expect("utf8")),
        ("WIFI_LIST", wifi_list.to_str().expect("utf8")),
        ("BT_INFO", info.to_str().expect("utf8")),
    ]);
    let path_env = exec_path_env(&dir);
    let toggle = exec_center::CenterOp::Toggle;
    exec_center::execute(toggle, "vol", "Volume", Some(&path_env)).expect("toggle mute");
    exec_center::execute(toggle, "bt:AA:BB:CC:DD:EE:FF", "HP", Some(&path_env)).expect("toggle bt");
    // Inert toggles: still `Ok`, no tool calls.
    for id in [
        "bright",
        "theme",
        "wifi",
        "pwreboot",
        "noop",
        &format!("launch:{}", launch::entry_id("firefox.desktop")),
    ] {
        exec_center::execute(toggle, id, id, Some(&path_env)).expect("inert toggle is Ok");
    }
    assert_eq!(
        read_log(&dir).lines().collect::<Vec<_>>(),
        vec![
            "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle",
            "bt info AA:BB:CC:DD:EE:FF",
            "bt disconnect AA:BB:CC:DD:EE:FF",
        ],
        "only vol/bt toggle arms act (wrapper `toggle)` verbatim)",
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// Malformed ids (`bad id`) and well-formed-but-unresolvable rows are
/// errors with no `flex:` prefix of their own — the runner adds the single
/// prefix at the binary boundary.
#[test]
fn executor_rejects_bad_and_unknown_ids_without_its_own_prefix() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-bad");
    for name in [
        "setsid",
        "wpctl",
        "systemctl",
        "hyprlock",
        "pkill",
        "notify-send",
    ] {
        log_stub(&dir, name, LOG_STUB);
    }
    let home = install_center_home("bad");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("THEME_SWITCHER", ""),
        ("FLEX_CENTER_PASSWORD", ""),
    ]);
    let path_env = exec_path_env(&dir);
    let ghost_hash = launch::entry_id("ghost.desktop");
    let ghost_launch = format!("launch:{ghost_hash}");
    let cases = [
        ("", "center: bad id ''"),
        ("a/b", "center: bad id 'a/b'"),
        ("a\nb", "center: bad id 'a\nb'"),
        (ghost_launch.as_str(), "center: unknown launch id"),
        ("frobnicate", "center: unknown action: frobnicate"),
        ("bt:", "center: empty MAC"),
    ];
    for (id, expected) in cases {
        let err = exec_center::execute(exec_center::CenterOp::Select, id, id, Some(&path_env))
            .expect_err("bad/unknown id must fail");
        let message = format!("{err:#}");
        assert!(
            message.starts_with(expected),
            "unexpected message for {id:?}: {message:?}"
        );
        assert!(
            !message.contains("flex:"),
            "no runner prefix below the runner: {message:?}"
        );
    }
    // Theme with an empty name (label exactly `Theme: `) errors too.
    let err = exec_center::execute(
        exec_center::CenterOp::Select,
        "theme",
        "Theme: ",
        Some(&path_env),
    )
    .expect_err("empty theme name must fail");
    assert_eq!(format!("{err:#}"), "center: empty theme name");
    assert!(
        !dir.join("calls.log").exists(),
        "no id failure launches anything"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// Loud arms stay loud: a failing `setsid`/power tool/switcher is an error
/// (never a quiet cancel — center has no `slurp`-like cancellable step).
#[test]
fn executor_loud_tool_failure_is_an_error_not_a_quiet_cancel() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = stub_dir("exec-loud-fail");
    write_exe(&dir.join("setsid"), "#!/usr/bin/env bash\nexit 3\n");
    write_exe(&dir.join("systemctl"), "#!/usr/bin/env bash\nexit 3\n");
    let switcher = dir.join("switcher.sh");
    write_exe(&switcher, "#!/usr/bin/env bash\nexit 3\n");
    let home = install_center_home("loud-fail");
    let _guard = EnvGuard::set(&[
        ("HOME", home.to_str().expect("utf8")),
        ("THEME_SWITCHER", switcher.to_str().expect("utf8")),
        ("FLEX_CENTER_PASSWORD", ""),
    ]);
    let path_env = exec_path_env(&dir);
    let id = format!("launch:{}", launch::entry_id("firefox.desktop"));
    let err = exec_center::execute(
        exec_center::CenterOp::Select,
        &id,
        "Firefox",
        Some(&path_env),
    )
    .expect_err("a failing setsid must fail");
    assert_eq!(
        format!("{err:#}"),
        "center: failed to launch firefox.desktop"
    );
    let err = exec_center::execute(
        exec_center::CenterOp::Select,
        "pwreboot",
        "Reboot",
        Some(&path_env),
    )
    .expect_err("a failing systemctl must fail");
    assert_eq!(format!("{err:#}"), "center: systemctl reboot failed");
    let err = exec_center::execute(
        exec_center::CenterOp::Select,
        "theme",
        "Theme: mocha",
        Some(&path_env),
    )
    .expect_err("a failing switcher must fail");
    assert_eq!(format!("{err:#}"), "center: activate mocha failed");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// Binary level without a pty: the menu cannot start, so the binary exits 1
/// with exactly one `flex: error:` prefix (the runner owns it; executor
/// errors carry none).
#[test]
fn binary_errors_carry_a_single_prefix() {
    let output = std::process::Command::new("setsid")
        .arg(env!("CARGO_BIN_EXE_flex-center"))
        .env("POPUP_KITTY", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-center without a pty");
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

/// Outside a popup the binary re-execs into the `menu-wide` popup (the
/// shared runner guard); the kitty spawn is asserted against a stub `PATH`.
/// The stub `PATH` rides on the child env only — no process-env mutation,
/// hence no lock.
#[test]
fn binary_outside_a_popup_reexecs_into_the_menu_wide_popup() {
    let dir = stub_dir("exec-guard");
    std::fs::create_dir_all(&dir).expect("stub dir");
    let kitty_log = dir.join("kitty.log");
    write_exe(&dir.join("pgrep"), "#!/usr/bin/env bash\nexit 1\n");
    write_exe(
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
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex-center"))
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
        .expect("run flex-center outside a popup");
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
            && logged.contains(&String::from("flex-menu-wide"))
            && logged.contains(&String::from("POPUP_KITTY=1")),
        "re-exec uses the menu-wide popup template: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
