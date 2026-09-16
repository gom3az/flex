//! `flex power` cutover tests (M6): provider rows, danger key-seq replays
//! (the release-gate safety properties), the armed-danger golden,
//! `FLEX_TEST` determinism, and wrapper dry-run + dispatch tests.
//!
//! Row-set parity is against the deleted
//! `waybar/.config/waybar/power-menu.sh` (see `src/providers/power.rs`).
//! Danger confirm reuses the shared `keys` flow (proven by the center
//! danger tests); these replays lock the power surface onto it.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, ARM_CONFIRM_DELAY, ARM_EXPIRE, EXIT_CANCELLED};
use flex_core::{backend, run, width, Menu};
use flex_rice::providers::power;

fn power_menu() -> Menu {
    flex_rice::menu(power::PROVIDER, vec![power::power_tab()])
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn rune(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

fn down() -> KeyEvent {
    press(KeyCode::Down)
}

fn at(base: Instant, ms: u64) -> Instant {
    base + Duration::from_millis(ms)
}

// --- Provider rows ----------------------------------------------------------

#[test]
fn row_set_matches_bash_power_rows_exactly() {
    let tab = power::power_tab();
    assert_eq!(tab.name, power::TAB_NAME);
    assert!(tab.bare_rows, "power uses launch-style bare rows");
    assert!(!tab.filterable, "power has no search");
    assert!(!tab.deletable, "power rows are non-deletable");
    let rows: Vec<(&str, &str, Option<&str>, bool)> = tab
        .rows
        .iter()
        .map(|row| {
            (
                row.id.as_str(),
                row.label.as_str(),
                row.meta.as_deref(),
                row.confirmable,
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("lock", "Lock Screen", Some("hyprlock"), false),
            ("suspend", "Suspend", Some("systemctl suspend"), false),
            ("reboot", "Reboot", Some("systemctl reboot"), true),
            ("poweroff", "Power Off", Some("systemctl poweroff"), true),
            ("logout", "Logout", Some("pkill -SIGTERM Hyprland"), false),
        ]
    );
}

#[test]
fn default_focus_is_row_zero() {
    let menu = power_menu();
    assert_eq!(menu.app.active_tab().expect("tab").state.focus, 0);
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "lock");
}

// --- Key-seq replays (danger safety properties) ------------------------------

/// Release-gate safety property: a single `Enter` on a danger row arms
/// but NEVER confirms — for every danger row, from a fresh menu.
#[test]
fn single_enter_on_confirmable_arms_but_never_confirms() {
    for (index, id) in ["reboot", "poweroff"].iter().enumerate() {
        let mut menu = power_menu();
        let t0 = Instant::now();
        for _ in 0..=index + 1 {
            // Focus row `index + 2` (Down from Lock Screen).
            let _ = handle_key(&mut menu, down(), t0);
        }
        assert_eq!(
            menu.app.focused_row().expect("row").id.as_str(),
            *id,
            "focused confirmable row"
        );
        let out = handle_key(&mut menu, press(KeyCode::Enter), t0);
        assert_eq!(out, KeyOutcome::Consumed, "single Enter on {id}");
        assert!(menu.app.is_armed(), "first Enter arms {id}");
    }
}

#[test]
fn second_enter_before_delay_is_swallowed() {
    assert_eq!(ARM_CONFIRM_DELAY, Duration::from_millis(50));
    let mut menu = power_menu();
    let t0 = Instant::now();
    let _ = handle_key(&mut menu, down(), t0);
    let _ = handle_key(&mut menu, down(), t0);
    let _ = handle_key(&mut menu, press(KeyCode::Enter), t0);
    let out = handle_key(&mut menu, press(KeyCode::Enter), at(t0, 49));
    assert_eq!(out, KeyOutcome::Consumed, "49 ms Enter is a hold");
    assert!(menu.app.is_armed(), "early Enter stays armed");
}

#[test]
fn second_enter_at_delay_confirms_reboot() {
    let mut menu = power_menu();
    let t0 = Instant::now();
    let _ = handle_key(&mut menu, down(), t0);
    let _ = handle_key(&mut menu, down(), t0);
    let _ = handle_key(&mut menu, press(KeyCode::Enter), t0);
    let out = handle_key(&mut menu, press(KeyCode::Enter), at(t0, 50));
    assert_eq!(out, KeyOutcome::Select);
    assert!(!menu.app.is_armed(), "confirm disarms");
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "reboot");
}

#[test]
fn arm_expires_after_five_seconds_then_rearms() {
    assert_eq!(ARM_EXPIRE, Duration::from_secs(5));
    let mut menu = power_menu();
    let t0 = Instant::now();
    for _ in 0..3 {
        let _ = handle_key(&mut menu, down(), t0);
    }
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "poweroff");
    let _ = handle_key(&mut menu, press(KeyCode::Enter), t0);
    // Stale arm + Enter: expiry disarms first, then the key re-arms.
    let out = handle_key(&mut menu, press(KeyCode::Enter), at(t0, 5_000));
    assert_eq!(out, KeyOutcome::Consumed, "expired arm never confirms");
    assert!(menu.app.is_armed(), "confirmable Enter re-arms fresh");
    // Tick expiry path.
    assert!(menu.tick(at(t0, 10_000)));
    assert!(!menu.app.is_armed());
}

#[test]
fn other_key_disarms_and_applies() {
    let mut menu = power_menu();
    let t0 = Instant::now();
    let _ = handle_key(&mut menu, down(), t0);
    let _ = handle_key(&mut menu, down(), t0);
    let _ = handle_key(&mut menu, press(KeyCode::Enter), t0);
    let out = handle_key(&mut menu, press(KeyCode::Enter), at(t0, 10));
    assert_eq!(out, KeyOutcome::Consumed, "hold swallowed");
    let out = handle_key(&mut menu, down(), at(t0, 20));
    assert_eq!(out, KeyOutcome::Consumed);
    assert!(!menu.app.is_armed(), "Down disarms");
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "poweroff");
}

#[test]
fn plain_rows_select_on_first_enter() {
    for (downs, id) in [(0, "lock"), (1, "suspend"), (4, "logout")] {
        let mut menu = power_menu();
        let base = run::test_base();
        let mut keys = vec![down(); downs];
        keys.push(press(KeyCode::Enter));
        let outcome = run::replay_keys(&mut menu, &keys, base);
        assert_eq!(outcome, KeyOutcome::Select, "{id} selects at once");
        assert_eq!(menu.app.focused_row().expect("row").id.as_str(), id);
    }
}

#[test]
fn typing_is_ignored_enter_selects_focused() {
    // Power has no search: typing never filters, so Enter selects the
    // focused row (lock) instead of arming a filtered danger row.
    let mut menu = power_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[rune('r'), rune('e'), rune('b'), press(KeyCode::Enter)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select, "no filter: Enter selects");
    assert!(!menu.app.is_armed(), "nothing arms without navigation");
    assert_eq!(menu.app.focused_row().expect("row").id.as_str(), "lock");
    assert!(
        menu.app.active_state().expect("state").filter.is_empty(),
        "filter stays empty"
    );
}

#[test]
fn keyseq_esc_cancels_with_no_action() {
    let mut menu = power_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], base);
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
}

#[test]
fn delete_never_fires_on_power_rows() {
    let mut menu = power_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Delete), press(KeyCode::Delete)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Consumed, "Delete is dead on power");
    assert!(
        !menu.app.active_state().expect("state").confirm_pending,
        "no confirm arms on non-deletable tabs"
    );
}

// --- Power golden -------------------------------------------------------------

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

#[test]
fn power_default_view_golden_at_80x24() {
    let mut menu = power_menu();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    terminal
        .draw(|frame| flex_core::render::render(frame, &mut menu))
        .expect("render frame");
    let buf = terminal.backend().buffer().clone();
    for y in 0..24 {
        let mut x = 0_u16;
        let mut total = 0_usize;
        while x < 80 {
            let cell = buf.cell((x, y)).expect("cell in frame");
            assert!(!cell.skip, "dangling skip cell at ({x}, {y})");
            let symbol_w = width::str_width(cell.symbol());
            total += symbol_w;
            x += u16::try_from(symbol_w.max(1)).expect("row width fits u16");
        }
        assert_eq!(total, 80, "row {y} must be exactly 80 cells");
    }
    // List rows sit below the reserved `•••` indicator line.
    let first_y = flex_core::render::LIST_INDICATOR_ROWS / 2;
    assert!(
        row_text(&buf, first_y, 80).starts_with('░'),
        "row 0 is the first list row (no tab bar)"
    );
    let first = row_text(&buf, first_y, 80);
    assert!(first.contains("Lock Screen"), "row 0: {first:?}");
    assert!(
        !first.contains("hyprlock"),
        "bare rows hide metas: {first:?}"
    );
    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(!all.contains('›'), "no filter line");
    assert!(!all.contains("navigate"), "no hints line");
    assert!(all.contains("Reboot"), "Reboot row renders");
}

#[test]
fn armed_confirmable_row_golden_at_80x24() {
    let mut menu = power_menu();
    let t0 = Instant::now();
    // Focus Reboot (Down, Down) and arm with a single Enter.
    let _ = handle_key(&mut menu, down(), t0);
    let _ = handle_key(&mut menu, down(), t0);
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Enter), t0),
        KeyOutcome::Consumed
    );
    assert!(menu.app.is_armed());
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    terminal
        .draw(|frame| flex_core::render::render(frame, &mut menu))
        .expect("render frame");
    let buf = terminal.backend().buffer().clone();
    // Armed danger rows show `-- confirm` text. Power rows carry no volume,
    // peaks or config data, so the tab renders flex's compact nodes: one line
    // per entry plus one blank gap row.
    let metrics = flex_core::render::node_metrics(&menu);
    assert_eq!(metrics, flex_core::render::NodeMetrics::COMPACT);
    let header = flex_core::render::LIST_INDICATOR_ROWS / 2 + 2 * metrics.pitch();
    let armed = row_text(&buf, header, 80);
    assert!(armed.contains("confirm"), "armed context: {armed:?}");
    // Compact nodes show only `selector_top` on their single line.
    let selector = buf.cell((0, header)).expect("selector cell");
    assert_eq!(selector.symbol(), "░");
    assert_eq!(
        selector.fg,
        flex_core::theme::Theme::DEFAULT
            .selector
            .fg
            .expect("selector sets a fg")
    );
    let gap = buf.cell((0, header + 1)).expect("gap cell");
    assert_eq!(gap.symbol(), " ", "the gap row carries nothing");
}

// --- FLEX_TEST determinism ----------------------------------------------------

#[test]
fn flex_test_replays_are_deterministic_across_bases() {
    assert_ne!(backend::FLEX_TEST_SEED, 0);
    let script = [down(), down(), press(KeyCode::Enter)];
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
    let mut first = power_menu();
    let mut second = power_menu();
    assert_eq!(run_script(&mut first), run_script(&mut second));
    assert_eq!(
        first.app.focused_row().expect("row").id,
        second.app.focused_row().expect("row").id
    );
}

// --- Wrapper tests ------------------------------------------------------------

fn wrapper_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-power.sh")
}

fn stub_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("flex-power-test-{}-{name}", std::process::id()))
}

/// Install a stub `flex` emitting `action_line` plus logging stubs for the
/// real power commands, then run the wrapper. `dry_run` selects `DRY_RUN=1`.
fn run_wrapper_with_stubs(
    name: &str,
    action_line: &str,
    dry_run: bool,
) -> (std::path::PathBuf, std::process::Output) {
    let dir = stub_dir(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    let log = dir.join("calls.log");
    for tool in ["hyprlock", "systemctl", "pkill"] {
        let path = dir.join(tool);
        std::fs::write(
            &path,
            format!(
                "#!/usr/bin/env bash\necho \"{tool} $@\" >> \"{}\"\n",
                log.display()
            ),
        )
        .expect("stub tool");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    let flex = dir.join("flex");
    std::fs::write(
        &flex,
        format!("#!/usr/bin/env bash\necho '{action_line}'\n"),
    )
    .expect("flex stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&flex, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
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
    if dry_run {
        cmd.env("DRY_RUN", "1");
    }
    let output = cmd.output().expect("run wrapper");
    (dir, output)
}

fn dry_run_output(name: &str, action_line: &str) -> (std::path::PathBuf, String) {
    let (dir, output) = run_wrapper_with_stubs(name, action_line, true);
    assert!(
        output.status.success(),
        "dry-run exits 0: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    (dir, stdout)
}

/// Blast-radius gate: every action id dry-runs to its exact bash command
/// and executes nothing (the stub log stays empty / unwritten).
#[test]
fn wrapper_dry_run_maps_every_action_to_its_bash_command() {
    let cases = [
        (
            "lock",
            "ACTION: power lock Lock Screen",
            "would run: hyprlock",
        ),
        (
            "suspend",
            "ACTION: power suspend Suspend",
            "would run: systemctl suspend",
        ),
        (
            "reboot",
            "ACTION: power reboot Reboot",
            "would run: systemctl reboot",
        ),
        (
            "poweroff",
            "ACTION: power poweroff Power Off",
            "would run: systemctl poweroff",
        ),
        (
            "logout",
            "ACTION: power logout Logout",
            "would run: pkill -SIGTERM Hyprland",
        ),
    ];
    for (name, action, expected) in cases {
        let (dir, stdout) = dry_run_output(name, action);
        assert!(
            stdout.trim_end() == expected,
            "dry-run for {name}: {stdout:?} != {expected:?}"
        );
        assert!(
            !dir.join("calls.log").exists(),
            "dry-run executes nothing for {name}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn wrapper_dispatches_real_commands_with_stubbed_path() {
    let cases = [
        ("lock-live", "ACTION: power lock Lock Screen", "hyprlock"),
        (
            "suspend-live",
            "ACTION: power suspend Suspend",
            "systemctl suspend",
        ),
        (
            "reboot-live",
            "ACTION: power reboot Reboot",
            "systemctl reboot",
        ),
        (
            "poweroff-live",
            "ACTION: power poweroff Power Off",
            "systemctl poweroff",
        ),
        (
            "logout-live",
            "ACTION: power logout Logout",
            "pkill -SIGTERM Hyprland",
        ),
    ];
    for (name, action, expected) in cases {
        let (dir, output) = run_wrapper_with_stubs(name, action, false);
        assert!(
            output.status.success(),
            "stderr: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        let log = std::fs::read_to_string(dir.join("calls.log")).expect("call log");
        assert!(
            log.trim_end() == expected,
            "dispatch for {name}: {log:?} != {expected:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn wrapper_rejects_malformed_action_lines() {
    let (dir, output) = run_wrapper_with_stubs("bad", "GARBAGE LINE", true);
    assert!(!output.status.success(), "malformed ACTION: must fail");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_rejects_unknown_action_ids() {
    let (dir, output) = run_wrapper_with_stubs("unknown", "ACTION: power format Format", true);
    assert!(!output.status.success(), "unknown id must fail");
    let _ = std::fs::remove_dir_all(&dir);
}
