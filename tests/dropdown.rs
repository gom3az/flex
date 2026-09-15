//! Target-dropdown replays: opening, moving, committing, cancelling, and the
//! `ChooseTarget` outcome that `run::run` turns into an `ACTION:TARGET` line.
//!
//! Upstream contract: `Enter`/`c` open the dropdown (or choose inside it),
//! `Esc` cancels, `j/k` + arrows move (`app.rs:548-608`,
//! `object_list.rs:56-72`); the highlight starts on the row's current target
//! (`view.rs:904-915`).

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use flex_core::keys::{handle_key, KeyOutcome};
use flex_core::{Menu, Mode, Row, RowId, Tab, Target};

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn rune(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

/// Two rows: one with targets (`Volume`), one without (`Plain`).
fn fixture() -> Menu {
    let targets = vec![
        Target::default_target(RowId::new("t-default"), "Default: Speakers"),
        Target::new(RowId::new("t-hdmi"), "HDMI"),
        Target::new(RowId::new("t-usb"), "USB Headset"),
    ];
    let tab = Tab::with_rows(
        "apps",
        vec![
            Row::with_targets(RowId::new("id-vol"), "Volume", targets, 0),
            Row::new(RowId::new("id-plain"), "Plain"),
        ],
    );
    Menu::new("test", vec![tab])
}

fn base() -> Instant {
    Instant::now()
}

#[test]
fn enter_opens_the_dropdown_on_a_row_with_targets() {
    let mut menu = fixture();
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Enter), base()),
        KeyOutcome::Consumed,
        "opening never selects the row"
    );
    let dropdown = menu.app.dropdown().expect("dropdown open");
    assert_eq!(dropdown.row, 0);
    assert_eq!(dropdown.selected, 0, "starts on the current target");
}

#[test]
fn enter_still_selects_rows_without_targets() {
    let mut menu = fixture();
    let _ = handle_key(&mut menu, press(KeyCode::Down), base());
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Enter), base()),
        KeyOutcome::Select,
        "rows without targets keep plain selection"
    );
    assert!(menu.app.dropdown().is_none());
}

#[test]
fn arrows_move_the_dropdown_highlight_not_the_focus() {
    let mut menu = fixture();
    let _ = handle_key(&mut menu, press(KeyCode::Enter), base());
    let _ = handle_key(&mut menu, press(KeyCode::Down), base());
    let _ = handle_key(&mut menu, press(KeyCode::Down), base());
    let dropdown = menu.app.dropdown().expect("dropdown open");
    assert_eq!(dropdown.selected, 2);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.focus,
        0,
        "row focus stays put while the dropdown is open"
    );

    // Clamped at the ends (no wrap).
    let _ = handle_key(&mut menu, press(KeyCode::Down), base());
    assert_eq!(menu.app.dropdown().expect("open").selected, 2);
    for _ in 0..5 {
        let _ = handle_key(&mut menu, press(KeyCode::Up), base());
    }
    assert_eq!(menu.app.dropdown().expect("open").selected, 0);
}

#[test]
fn enter_commits_the_highlighted_target() {
    let mut menu = fixture();
    let _ = handle_key(&mut menu, press(KeyCode::Enter), base());
    let _ = handle_key(&mut menu, press(KeyCode::Down), base());
    let outcome = handle_key(&mut menu, press(KeyCode::Enter), base());
    assert_eq!(
        outcome,
        KeyOutcome::ChooseTarget {
            row: RowId::new("id-vol"),
            target: RowId::new("t-hdmi"),
            title: "HDMI".to_string(),
        }
    );
    assert!(menu.app.dropdown().is_none(), "commit closes the dropdown");
    assert_eq!(
        menu.app.active_tab().expect("tab").rows[0].target_index,
        1,
        "the row remembers the chosen target"
    );
}

#[test]
fn esc_closes_the_dropdown_before_anything_else() {
    let mut menu = fixture();
    let _ = handle_key(&mut menu, press(KeyCode::Enter), base());
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Esc), base()),
        KeyOutcome::Consumed,
        "Esc cancels the dropdown instead of quitting"
    );
    assert!(menu.app.dropdown().is_none());
    assert_eq!(
        menu.app.active_tab().expect("tab").rows[0].target_index,
        0,
        "cancelling keeps the old target"
    );
}

#[test]
fn escaping_a_closed_dropdown_still_quits() {
    let mut menu = fixture();
    assert_eq!(
        handle_key(&mut menu, press(KeyCode::Esc), base()),
        KeyOutcome::Quit(flex_core::keys::EXIT_CANCELLED)
    );
}

#[test]
fn navigate_mode_uses_j_k_and_c() {
    let mut menu = fixture();
    menu.app.mode = Mode::Navigate;
    // `c` opens (upstream's dropdown key).
    let _ = handle_key(&mut menu, rune('c'), base());
    assert!(menu.app.dropdown().is_some(), "c opens the dropdown");
    let _ = handle_key(&mut menu, rune('j'), base());
    let _ = handle_key(&mut menu, rune('j'), base());
    assert_eq!(menu.app.dropdown().expect("open").selected, 2);
    let _ = handle_key(&mut menu, rune('k'), base());
    assert_eq!(menu.app.dropdown().expect("open").selected, 1);
    // j,j,k leaves the highlight on the second target (HDMI).
    let outcome = handle_key(&mut menu, rune('c'), base());
    assert_eq!(
        outcome,
        KeyOutcome::ChooseTarget {
            row: RowId::new("id-vol"),
            target: RowId::new("t-hdmi"),
            title: "HDMI".to_string(),
        }
    );
}

#[test]
fn normal_mode_typing_still_edits_the_filter() {
    let mut menu = fixture();
    // NORMAL mode: `c` is filter text, not a dropdown key (flex's filter is an
    // extension upstream does not have).
    let _ = handle_key(&mut menu, rune('c'), base());
    assert!(menu.app.dropdown().is_none());
    assert_eq!(
        menu.app.active_tab().expect("tab").state.filter,
        "c",
        "NORMAL mode keeps type-to-filter"
    );
}

#[test]
fn switching_tabs_keeps_per_tab_dropdown_state() {
    let targets = vec![Target::new(RowId::new("t0"), "One")];
    let first = Tab::with_rows(
        "apps",
        vec![Row::with_targets(RowId::new("id-a"), "Alpha", targets, 0)],
    );
    let second = Tab::with_rows("power", vec![Row::new(RowId::new("id-b"), "Beta")]);
    let mut menu = Menu::new("test", vec![first, second]);
    let _ = handle_key(&mut menu, press(KeyCode::Enter), base());
    assert!(menu.app.dropdown().is_some());
    let _ = handle_key(&mut menu, press(KeyCode::Tab), base());
    assert!(
        menu.app.dropdown().is_none(),
        "the other tab has no dropdown open"
    );
    let _ = handle_key(&mut menu, press(KeyCode::BackTab), base());
    assert!(
        menu.app.dropdown().is_some(),
        "returning restores the open dropdown"
    );
}

#[test]
fn help_overlay_takes_precedence_over_the_dropdown() {
    let mut menu = fixture();
    let _ = handle_key(&mut menu, press(KeyCode::Enter), base());
    let _ = handle_key(&mut menu, rune('?'), base());
    assert!(menu.app.help_open);
    // While help is open, Down scrolls the help instead of the dropdown.
    let _ = handle_key(&mut menu, press(KeyCode::Down), base());
    assert_eq!(menu.app.help_scroll, 1);
    assert_eq!(menu.app.dropdown().expect("open").selected, 0);
    // Enter closes the help overlay.
    let _ = handle_key(&mut menu, press(KeyCode::Enter), base());
    assert!(!menu.app.help_open);
}

#[test]
fn dropdown_state_helpers_clamp_to_the_target_list() {
    let mut menu = fixture();
    assert!(menu.app.open_dropdown());
    assert!(menu.app.move_dropdown(99));
    assert_eq!(menu.app.dropdown().expect("open").selected, 2);
    let (index, target) = menu.app.highlighted_target().expect("highlight");
    assert_eq!(index, 2);
    assert_eq!(target.id, RowId::new("t-usb"));
    let chosen = menu.app.commit_dropdown().expect("commit");
    assert_eq!(chosen.0, RowId::new("id-vol"));
    assert_eq!(chosen.2, "USB Headset".to_string());
    // No dropdown left to commit.
    assert!(menu.app.commit_dropdown().is_none());
}

#[test]
fn rows_without_targets_cannot_open_a_dropdown() {
    let mut menu = fixture();
    menu.app.active_tab_mut().expect("tab").state.focus = 1;
    assert!(!menu.app.open_dropdown());
    assert!(menu.app.dropdown().is_none());
    assert!(!menu.app.move_dropdown(1));
    assert!(menu.app.highlighted_target().is_none());
}
