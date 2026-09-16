//! `flex shot` cutover tests (M3): provider rows, key-seq replays, shot
//! golden, `FLEX_TEST` determinism, and wrapper pipeline stubs.
//!
//! Row-set parity is against the deleted
//! `rofi/.config/rofi/scripts/screenshot.sh` (see `src/providers/shot.rs`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::{backend, run, width, Menu};
use flex_rice::providers::shot;

fn shot_menu() -> Menu {
    flex_rice::menu(shot::PROVIDER, vec![shot::shot_tab()])
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn down() -> KeyEvent {
    press(KeyCode::Down)
}

// --- Provider rows ----------------------------------------------------------

#[test]
fn row_set_matches_bash_cap_rows_exactly() {
    let tab = shot::shot_tab();
    assert_eq!(tab.name, shot::TAB_NAME);
    assert!(tab.bare_rows, "capture uses bare rows");
    assert!(!tab.deletable, "capture rows are non-deletable");
    let rows: Vec<(&str, &str, Option<&str>)> = tab
        .rows
        .iter()
        .map(|row| (row.id.as_str(), row.label.as_str(), row.meta.as_deref()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("area-shot", "  Area Screenshot", Some("PNG")),
            ("full-shot", "🖥  Full Screenshot", Some("PNG")),
            ("win-shot", "  Window Screenshot", Some("PNG")),
            ("area-rec", "  Area Recording", Some("MP4")),
            ("area-rec-audio", "  Area Recording + Audio", Some("MP4")),
            ("full-rec", "🖥  Full Recording", Some("MP4")),
            ("full-rec-audio", "🖥  Full Recording + Audio", Some("MP4")),
        ]
    );
}

#[test]
fn default_focus_is_row_zero() {
    let menu = shot_menu();
    assert_eq!(menu.app.active_tab().expect("tab").state.focus, 0);
}

// --- Key-seq replays ----------------------------------------------------------

#[test]
fn keyseq_down_down_enter_selects_window_shot() {
    let mut menu = shot_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(&mut menu, &[down(), down(), press(KeyCode::Enter)], base);
    assert_eq!(outcome, KeyOutcome::Select);
    let row = menu.app.focused_row().expect("focused row");
    assert_eq!(row.id.as_str(), "win-shot");
    assert_eq!(row.label, "  Window Screenshot");
    // The wrapper's ACTION: line for this selection:
    // `ACTION: shot win-shot   Window Screenshot` (exit 0).
    assert_eq!(menu.provider, "shot");
}

#[test]
fn keyseq_down_to_area_recording_audio() {
    // No filter in shot mode: use Down x4 to reach area-rec-audio (index 4).
    let mut menu = shot_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Enter),
        ],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select);
    let row = menu.app.focused_row().expect("focused row");
    assert_eq!(row.id.as_str(), "area-rec-audio");
}

#[test]
fn keyseq_esc_cancels_with_no_action() {
    let mut menu = shot_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], base);
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
}

#[test]
fn delete_never_fires_on_capture_rows() {
    let mut menu = shot_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Delete), press(KeyCode::Delete)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Consumed, "Delete is dead on shot");
    assert!(
        !menu.app.active_state().expect("state").confirm_pending,
        "no confirm arms on non-deletable tabs"
    );
}

// --- Shot golden --------------------------------------------------------------

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
fn shot_default_view_golden_at_80x24() {
    let mut menu = shot_menu();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    terminal
        .draw(|frame| flex_core::render::render(frame, &mut menu))
        .expect("render frame");
    let buf = terminal.backend().buffer().clone();
    // Every row is exactly the frame width in display cells.
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
    // Bare mode: list rows start below the reserved `•••` indicator line
    // (`LIST_INDICATOR_ROWS`), no tab bar, no filter, no meta.
    let first_y = flex_core::render::LIST_INDICATOR_ROWS / 2;
    assert!(
        row_text(&buf, first_y, 80).starts_with('░'),
        "row 0 is the first list row"
    );
    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(!all.contains('›'), "no filter line");
    assert!(!all.contains("PNG"), "bare rows hide meta");
    assert!(all.contains("Area Screenshot"), "area shot row renders");
}

// --- FLEX_TEST seed extension -----------------------------------------------------

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
    let mut first = shot_menu();
    let mut second = shot_menu();
    assert_eq!(run_script(&mut first), run_script(&mut second));
    assert_eq!(
        first.app.focused_row().expect("row").id,
        second.app.focused_row().expect("row").id
    );
}

// --- Wrapper stub tests -----------------------------------------------------------

fn wrapper_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-shot.sh")
}

fn stub_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("flex-shot-test-{}-{name}", std::process::id()))
}

/// Install stub `flex` + capture-pipeline tools logging to `log`, then run
/// the wrapper with `action_line` as the canned `flex shot` output.
fn run_wrapper_with_stubs(
    name: &str,
    action_line: &str,
    extra_envs: &[(&str, &str)],
) -> (std::path::PathBuf, std::process::Output) {
    let dir = stub_dir(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    let log = dir.join("calls.log");
    for tool in ["slurp", "grim", "wl-copy", "notify-send"] {
        let path = dir.join(tool);
        let body = if tool == "grim" {
            // The real grim writes the screenshot file; the stub touches
            // it so the later `wl-copy < file` redirect succeeds.
            format!(
                "#!/usr/bin/env bash\necho \"{tool} $@\" >> \"{}\"\ntouch \"${{@: -1}}\"\n",
                log.display()
            )
        } else {
            format!(
                "#!/usr/bin/env bash\necho \"{tool} $@\" >> \"{}\"\n",
                log.display()
            )
        };
        std::fs::write(&path, body).expect("stub tool");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    std::fs::write(dir.join("geometry"), "0,0 10x10").expect("geometry stub");
    std::fs::write(
        dir.join("slurp"),
        format!(
            "#!/usr/bin/env bash\necho \"slurp $@\" >> \"{}\"\ncat \"{}/geometry\"\n",
            log.display(),
            dir.display()
        ),
    )
    .expect("slurp stub");
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
    let save_dir = dir.join("shots");
    let mut cmd = std::process::Command::new("bash");
    cmd.arg(wrapper_path())
        .env("PATH", path_env)
        .env("SCREENSHOT_DIR", &save_dir);
    // The popup re-exec is bind-path behavior; wrapper tests emulate the
    // in-popup half (stubbed HOME has no popup.sh).
    cmd.env("POPUP_KITTY", "1");
    for (key, value) in extra_envs {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("run wrapper");
    (dir, output)
}

#[test]
fn wrapper_rejects_malformed_action_lines() {
    let (dir, output) = run_wrapper_with_stubs("bad", "GARBAGE LINE", &[]);
    assert!(!output.status.success(), "malformed ACTION: must fail");
    let _ = std::fs::remove_dir_all(&dir);
}
