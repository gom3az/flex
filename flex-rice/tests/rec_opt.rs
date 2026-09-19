//! Recording-options submenu tests: options tab rows, confirm-first focus,
//! and key-seq replays through the drill-in tabs.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::{run, width, Menu};
use flex_rice::providers::rec_opt;
use flex_rice::providers::rec_opt::{Quality, RecOptions};

fn options_menu(opts: RecOptions) -> Menu {
    flex_rice::menu(rec_opt::PROVIDER, vec![rec_opt::options_tab(&opts)])
}

fn dimension_menu(tab: flex_core::Tab) -> Menu {
    flex_rice::menu(rec_opt::PROVIDER, vec![tab])
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn options_tab_confirms_first_with_defaults_summary() {
    let menu = options_menu(RecOptions::defaults());
    assert_eq!(menu.provider, "rec-opt");
    let rows: Vec<(&str, &str)> = menu
        .app
        .active_tab()
        .expect("tab")
        .rows
        .iter()
        .map(|row| (row.id.as_str(), row.label.as_str()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("rec-confirm", "Record (Audio · Balanced · 30fps)"),
            ("rec-audio", "Audio: On"),
            ("rec-quality", "Quality: Balanced"),
            ("rec-fps", "Framerate: 30fps"),
        ]
    );
}

#[test]
fn enter_on_row_zero_confirms() {
    let mut menu = options_menu(RecOptions::defaults());
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Enter)], run::test_base());
    assert_eq!(outcome, KeyOutcome::Select);
    let row = menu.app.focused_row().expect("focused row");
    assert_eq!(row.id.as_str(), "rec-confirm");
}

#[test]
fn quality_tab_selects_high_on_down_down_enter() {
    let mut menu = dimension_menu(rec_opt::quality_tab());
    let outcome = run::replay_keys(
        &mut menu,
        &[
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Enter),
        ],
        run::test_base(),
    );
    assert_eq!(outcome, KeyOutcome::Select);
    assert_eq!(
        menu.app.focused_row().expect("focused row").id.as_str(),
        "rec-quality-high"
    );
}

#[test]
fn audio_tab_selects_mute_on_down_enter() {
    let mut menu = dimension_menu(rec_opt::audio_tab());
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Down), press(KeyCode::Enter)],
        run::test_base(),
    );
    assert_eq!(outcome, KeyOutcome::Select);
    assert_eq!(
        menu.app.focused_row().expect("focused row").id.as_str(),
        "rec-audio-off"
    );
}

#[test]
fn esc_cancels_with_no_action() {
    let mut menu = options_menu(RecOptions::defaults());
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], run::test_base());
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
}

#[test]
fn submenu_round_trip_updates_the_summary() {
    // Enter on Quality (row 2), pick High, rebuild: the confirm label
    // carries the new settings.
    let mut opts = RecOptions::defaults();
    let mut menu = options_menu(opts);
    let outcome = run::replay_keys(
        &mut menu,
        &[
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Enter),
        ],
        run::test_base(),
    );
    assert_eq!(outcome, KeyOutcome::Select);
    let picked = menu
        .app
        .focused_row()
        .expect("focused row")
        .id
        .as_str()
        .to_owned();
    assert_eq!(picked, "rec-quality");
    assert_eq!(
        rec_opt::apply_choice(&mut opts, &picked),
        rec_opt::Choice::Open(rec_opt::Dimension::Quality)
    );

    let mut menu = dimension_menu(rec_opt::quality_tab());
    let outcome = run::replay_keys(
        &mut menu,
        &[
            press(KeyCode::Down),
            press(KeyCode::Down),
            press(KeyCode::Enter),
        ],
        run::test_base(),
    );
    assert_eq!(outcome, KeyOutcome::Select);
    let picked = menu
        .app
        .focused_row()
        .expect("focused row")
        .id
        .as_str()
        .to_owned();
    assert_eq!(
        rec_opt::apply_choice(&mut opts, &picked),
        rec_opt::Choice::Updated
    );
    assert_eq!(opts.quality, Quality::High);

    let menu = options_menu(opts);
    assert_eq!(
        menu.app.active_tab().expect("tab").rows[0].label.as_str(),
        "Record (Audio · High · 30fps)"
    );
}

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
fn options_golden_at_80x24() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    let mut menu = options_menu(RecOptions::defaults());
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
    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(
        all.contains("Record (Audio · Balanced · 30fps)"),
        "confirm row renders with the live summary"
    );
    assert!(all.contains("Quality: Balanced"), "dimension rows render");
    let _ = handle_key;
}
