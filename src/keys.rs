//! Key handling priority chain over `crossterm` key events.
//!
//! Order per keystroke (first match wins):
//!
//! 1. Non-`Press` event kinds (`Release`/`Repeat`) are swallowed — this is
//!    what discards Enter-hold repeats before timing logic runs.
//! 2. `Esc` chain: filter-clear → help-close → confirm-cancel → disarm →
//!    exit `130`. (Confirm-cancel extends the `UI_UX_doc.md` chain for the
//!    M1 `Delete` flow.)
//! 3. Pending `Delete` confirm: `Delete`/`Enter` deletes, anything else
//!    cancels the pending state (`Esc` already returned above) and the key
//!    is processed normally.
//! 4. Armed danger flow: `Enter` confirms iff at least [`ARM_CONFIRM_DELAY`]
//!    elapsed since arming (earlier presses are swallowed, staying armed);
//!    a stale arm (≥ [`ARM_EXPIRE`]) expires; any other key disarms first
//!    and is then processed normally.
//! 5. `Ctrl-c` quits (`130`); `Alt-digit` always switches tabs.
//! 6. `Ctrl` combos: `n`/`p` navigate, `u` clears the filter, `w` kills a
//!    word, `o` flips NORMAL ↔ NAVIGATE.
//! 7. Special keys: `Enter` select (danger rows arm instead), `Delete`
//!    arms the delete confirm on deletable tabs only (see `Tab::deletable`;
//!    launch/power leave it `false`), `Tab`/`Shift-Tab` cycle tabs, arrows/`Home`/
//!    `End` move focus (steps wrap, `PgUp`/`PgDn` clamp), `Backspace` edits
//!    the filter, `F1` toggles gauge mute when a gauge is present.
//! 8. Runes (`Char` with no `Ctrl`/`Alt`): `?` toggles help; `1`–`9` switch
//!    tabs iff the filter is empty (else they type); `q` quits iff
//!    NORMAL + empty filter (else types in NORMAL, ignored in NAVIGATE);
//!    `j`/`k`/`h`/`l` navigate and `m` marks in NAVIGATE (they type in
//!    NORMAL); all other runes edit the filter in NORMAL and are ignored
//!    in NAVIGATE. Exception: NAVIGATE `m` on deletable tabs (`clip`)
//!    returns [`KeyOutcome::Toggle`] so the wrapper can pin/unpin.
//!
//! Timestamps are injected (`now: Instant`); the engine never sleeps, so
//! danger timing is fully deterministic under test.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{Menu, Mode};

/// Minimum age of an arm for a second `Enter` to confirm (hold = swallow).
pub const ARM_CONFIRM_DELAY: Duration = Duration::from_millis(50);
/// Arm lifetime: auto-disarm at or past this age (checked on keys + tick).
pub const ARM_EXPIRE: Duration = Duration::from_secs(5);
/// Rows moved by `PageUp`/`PageDown`.
pub const PAGE_STEP: isize = 10;
/// Cancelled exit code (matches `backend::EXIT_CANCELLED`).
pub const EXIT_CANCELLED: i32 = 130;

/// What a keystroke decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Consumed with no further action (includes swallowed holds/repeats).
    Consumed,
    /// Select the focused row (caller builds `Outcome::Chosen`).
    Select,
    /// Delete the focused row (delete-confirm was armed and confirmed).
    Delete,
    /// Toggle the pin on the focused row (clip NAVIGATE `m`; the wrapper
    /// flips the pin in the store after the TUI exits).
    Toggle,
    /// A dropdown target was chosen: `(row id, target id, target title)`.
    ///
    /// The caller reports it as an `ACTION:TARGET` line (upstream
    /// `Action::SetTarget`).
    ChooseTarget {
        /// Action id of the row the dropdown belongs to.
        row: crate::RowId,
        /// Id of the chosen target.
        target: crate::RowId,
        /// Human title of the chosen target.
        title: String,
    },
    /// Quit the menu with an exit code (`130` = cancelled).
    Quit(i32),
}

/// Handle one key event for `menu` at time `now`.
///
/// Pure state machine apart from the `&mut` borrow: no I/O, no sleeping,
/// no clock reads — pass the event-loop timestamp for danger timing.
///
/// # Panics
///
/// Never panics through public flows: the single internal `expect` covers
/// the `is_armed() ⟹ armed_at.is_some()` invariant, which `arm`/`disarm`
/// maintain by construction.
#[must_use]
pub fn handle_key(menu: &mut Menu, key: KeyEvent, now: Instant) -> KeyOutcome {
    // 1. Swallow holds and releases (Enter-hold never confirms).
    if key.kind != KeyEventKind::Press {
        return KeyOutcome::Consumed;
    }

    // 2. Esc chain.
    if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
        return handle_esc(&mut menu.app);
    }

    // 3. Pending delete confirm (Esc already returned above).
    if menu.app.active_state().is_some_and(|s| s.confirm_pending)
        && handle_pending_delete(&mut menu.app, &key)
    {
        return KeyOutcome::Delete;
    }
    // Fall through: the key still applies (e.g. typing cancels + types).

    // 4. Armed danger flow (expiry/disarm fall through to normal handling).
    if let Some(outcome) = handle_armed(&mut menu.app, &key, now) {
        return outcome;
    }

    // 5-6. Ctrl-c, Alt-digit, Ctrl combos.
    if let Some(outcome) = handle_chords(&mut menu.app, &key) {
        return outcome;
    }

    // 7. Special keys (plain, no modifiers unless noted).
    if let Some(outcome) = handle_special(menu, &key, now) {
        return outcome;
    }

    // 8. Runes (Shift accepted so `?`/`Q`/upper digits work as typed).
    if let KeyCode::Char(c) = key.code {
        if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT {
            return handle_rune(&mut menu.app, c);
        }
    }
    KeyOutcome::Consumed
}

/// Esc chain: dropdown-close → filter-clear → help-close → confirm-cancel →
/// disarm → exit.
///
/// The dropdown closes first: upstream's `Esc` cancels the open dropdown
/// before anything else (`Action::CloseDropdown`).
fn handle_esc(app: &mut crate::App) -> KeyOutcome {
    if app.active_tab_mut().is_none() {
        return KeyOutcome::Quit(EXIT_CANCELLED);
    }
    if app.close_dropdown() {
        return KeyOutcome::Consumed;
    }
    let filter_was_empty = app.active_state().is_some_and(|s| s.filter.is_empty());
    if !filter_was_empty {
        if let Some(tab) = app.active_tab_mut() {
            tab.state.filter.clear();
        }
        app.clamp_focus();
        return KeyOutcome::Consumed;
    }
    if app.help_open {
        app.help_open = false;
        app.help_scroll = 0;
        return KeyOutcome::Consumed;
    }
    let had_pending = app
        .active_tab_mut()
        .is_some_and(|tab| std::mem::replace(&mut tab.state.confirm_pending, false));
    if had_pending {
        return KeyOutcome::Consumed;
    }
    if app.disarm() {
        return KeyOutcome::Consumed;
    }
    KeyOutcome::Quit(EXIT_CANCELLED)
}

/// Pending `Delete` confirm: `Delete`/`Enter` confirms (clearing the flag),
/// anything else clears the flag and falls through (`false`).
fn handle_pending_delete(app: &mut crate::App, key: &KeyEvent) -> bool {
    let is_delete = key.code == KeyCode::Delete && key.modifiers == KeyModifiers::NONE;
    let is_enter = key.code == KeyCode::Enter && key.modifiers == KeyModifiers::NONE;
    if let Some(tab) = app.active_tab_mut() {
        tab.state.confirm_pending = false;
    }
    is_delete || is_enter
}

/// Armed danger flow. `Some` = decided (`Select` on a mature `Enter`,
/// `Consumed` on an early `Enter`); `None` = expired/disarm/other key, fall
/// through to normal handling.
fn handle_armed(app: &mut crate::App, key: &KeyEvent, now: Instant) -> Option<KeyOutcome> {
    if !app.is_armed() {
        return None;
    }
    let since = app
        .active_state()
        .and_then(|s| s.armed_at)
        .expect("armed means armed_at is set");
    let age = now.checked_duration_since(since).unwrap_or(Duration::ZERO);
    if age >= ARM_EXPIRE {
        app.disarm();
        return None;
    }
    if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::NONE {
        if age >= ARM_CONFIRM_DELAY {
            app.disarm();
            return Some(KeyOutcome::Select);
        }
        return Some(KeyOutcome::Consumed);
    }
    app.disarm();
    None
}

/// `Ctrl-c` quit, `Alt-digit` tab jump, `Ctrl` combos. `None` = no chord
/// keys involved; the keystroke continues to special/runes handling.
fn handle_chords(app: &mut crate::App, key: &KeyEvent) -> Option<KeyOutcome> {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(KeyOutcome::Quit(EXIT_CANCELLED));
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(digit @ '1'..='9') = key.code {
            let index = digit as usize - '0' as usize - 1;
            if index < app.tabs.len() {
                app.switch_tab(index);
            }
        }
        return Some(KeyOutcome::Consumed);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::SHIFT)
    {
        return Some(match key.code {
            KeyCode::Char('n' | 'N') => {
                app.move_focus(1);
                KeyOutcome::Consumed
            }
            KeyCode::Char('p' | 'P') => {
                app.move_focus(-1);
                KeyOutcome::Consumed
            }
            KeyCode::Char('u' | 'U') => {
                if let Some(tab) = app.active_tab_mut() {
                    tab.state.filter.clear();
                    tab.state.confirm_pending = false;
                }
                app.clamp_focus();
                KeyOutcome::Consumed
            }
            KeyCode::Char('w' | 'W') => {
                if let Some(tab) = app.active_tab_mut() {
                    kill_word(&mut tab.state.filter);
                }
                app.clamp_focus();
                KeyOutcome::Consumed
            }
            KeyCode::Char('o' | 'O') => {
                app.mode = match app.mode {
                    Mode::Normal => Mode::Navigate,
                    Mode::Navigate => Mode::Normal,
                };
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        });
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(KeyOutcome::Consumed);
    }
    None
}
/// Movement/dismiss keys while the help overlay is open (upstream
/// `Action::MoveUp`/`MoveDown`/`Action::Help`, `app.rs:548-560`).
fn handle_help_overlay(app: &mut crate::App, key: &KeyEvent) -> Option<KeyOutcome> {
    if !app.help_open {
        return None;
    }
    let step = usize::try_from(PAGE_STEP).unwrap_or(1);
    match key.code {
        KeyCode::Up if key.modifiers == KeyModifiers::NONE => {
            app.help_scroll = app.help_scroll.saturating_sub(1);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::PageUp if key.modifiers == KeyModifiers::NONE => {
            app.help_scroll = app.help_scroll.saturating_sub(step);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Down if key.modifiers == KeyModifiers::NONE => {
            app.help_scroll = app.help_scroll.saturating_add(1);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::PageDown if key.modifiers == KeyModifiers::NONE => {
            app.help_scroll = app.help_scroll.saturating_add(step);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Enter if key.modifiers == KeyModifiers::NONE => {
            app.help_open = false;
            Some(KeyOutcome::Consumed)
        }
        _ => None,
    }
}

/// Movement + commit while a target dropdown is open (upstream
/// `Action::MoveUp`/`MoveDown` then `Action::SetTarget`).
fn handle_open_dropdown(app: &mut crate::App, key: &KeyEvent) -> Option<KeyOutcome> {
    if app.dropdown().is_none() || key.modifiers != KeyModifiers::NONE {
        return None;
    }
    match key.code {
        KeyCode::Up => {
            app.move_dropdown(-1);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Down => {
            app.move_dropdown(1);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Enter => Some(match app.commit_dropdown() {
            Some((row, target, title)) => KeyOutcome::ChooseTarget { row, target, title },
            None => KeyOutcome::Consumed,
        }),
        _ => None,
    }
}

/// Special keys (plain, no modifiers unless noted). `None` = not a
/// special key; the keystroke continues to runes handling.
fn handle_special(menu: &mut Menu, key: &KeyEvent, now: Instant) -> Option<KeyOutcome> {
    let app = &mut menu.app;
    if let Some(outcome) = handle_help_overlay(app, key) {
        return Some(outcome);
    }
    if let Some(outcome) = handle_open_dropdown(app, key) {
        return Some(outcome);
    }
    match key.code {
        KeyCode::Enter if key.modifiers == KeyModifiers::NONE => Some(enter_select(app, now)),
        KeyCode::Delete if key.modifiers == KeyModifiers::NONE => {
            // Only deletable tabs (e.g. `clip`) arm the confirm flow;
            // launch/power rows are never deletable.
            let armed = app.active_tab().is_some_and(|tab| tab.deletable)
                && app.focused_original_index().is_some();
            if armed {
                if let Some(tab) = app.active_tab_mut() {
                    tab.state.confirm_pending = true;
                }
            }
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Tab if key.modifiers == KeyModifiers::NONE => {
            app.cycle_tab(1);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::BackTab => {
            app.cycle_tab(-1);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Up if key.modifiers == KeyModifiers::NONE => {
            app.move_focus(-1);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Down if key.modifiers == KeyModifiers::NONE => {
            app.move_focus(1);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Left | KeyCode::Right if key.modifiers == KeyModifiers::NONE => {
            app.cycle_tab(if key.code == KeyCode::Right { 1 } else { -1 });
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Home if key.modifiers == KeyModifiers::NONE => {
            if let Some(tab) = app.active_tab_mut() {
                tab.state.focus = 0;
            }
            Some(KeyOutcome::Consumed)
        }
        KeyCode::End if key.modifiers == KeyModifiers::NONE => {
            let len = app.visible_len();
            if let Some(tab) = app.active_tab_mut() {
                tab.state.focus = len.saturating_sub(1);
            }
            Some(KeyOutcome::Consumed)
        }
        KeyCode::PageUp if key.modifiers == KeyModifiers::NONE => {
            app.move_focus_clamped(-PAGE_STEP);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::PageDown if key.modifiers == KeyModifiers::NONE => {
            app.move_focus_clamped(PAGE_STEP);
            Some(KeyOutcome::Consumed)
        }
        KeyCode::Backspace if key.modifiers == KeyModifiers::NONE => {
            if let Some(tab) = app.active_tab_mut() {
                tab.state.filter.pop();
            }
            app.clamp_focus();
            Some(KeyOutcome::Consumed)
        }
        KeyCode::F(1) if key.modifiers == KeyModifiers::NONE => {
            if let Some(gauge) = menu.gauge.as_mut() {
                gauge.toggle_mute();
            }
            Some(KeyOutcome::Consumed)
        }
        _ => None,
    }
}

/// `Enter`: dropdown rows open their target list (upstream `Enter/c open
/// dropdown or choose`); danger rows arm on first press (caller sees
/// `Consumed`); plain rows select immediately.
fn enter_select(app: &mut crate::App, now: Instant) -> KeyOutcome {
    if app.focused_row().is_some_and(|row| !row.targets.is_empty()) {
        app.open_dropdown();
        return KeyOutcome::Consumed;
    }
    let confirmable = app.focused_row().is_some_and(|row| row.confirmable);
    if !confirmable {
        return if app.focused_row().is_some() {
            KeyOutcome::Select
        } else {
            KeyOutcome::Consumed
        };
    }
    if let Some(tab) = app.active_tab_mut() {
        tab.state.arm(now);
    }
    KeyOutcome::Consumed
}

/// Rune routing: `?` help, digits tab-switch-or-type, `q` quit-or-type,
/// NAVIGATE nav/mark/dropdown, otherwise NORMAL filter edit.
fn handle_rune(app: &mut crate::App, c: char) -> KeyOutcome {
    if c == '?' {
        // Opening the help overlay resets its scroll (upstream sets
        // `help_position = Some(0)`, `app.rs:641`).
        app.help_open = !app.help_open;
        if app.help_open {
            app.help_scroll = 0;
        }
        return KeyOutcome::Consumed;
    }
    if app.help_open && app.mode == Mode::Navigate {
        match c {
            'j' | 'J' => {
                app.help_scroll = app.help_scroll.saturating_add(1);
                return KeyOutcome::Consumed;
            }
            'k' | 'K' => {
                app.help_scroll = app.help_scroll.saturating_sub(1);
                return KeyOutcome::Consumed;
            }
            _ => {}
        }
    }
    if app.dropdown().is_some() && app.mode == Mode::Navigate {
        match c {
            'j' | 'J' => {
                app.move_dropdown(1);
                return KeyOutcome::Consumed;
            }
            'k' | 'K' => {
                app.move_dropdown(-1);
                return KeyOutcome::Consumed;
            }
            'c' | 'C' => {
                return match app.commit_dropdown() {
                    Some((row, target, title)) => KeyOutcome::ChooseTarget { row, target, title },
                    None => KeyOutcome::Consumed,
                };
            }
            _ => {}
        }
    }
    if let '1'..='9' = c {
        let index = c as usize - '0' as usize - 1;
        let filter_empty = app.active_state().is_some_and(|s| s.filter.is_empty());
        if filter_empty {
            if index < app.tabs.len() {
                app.switch_tab(index);
            }
            return KeyOutcome::Consumed;
        }
        // Else: falls through to filter insert below.
    } else if c == 'q' || c == 'Q' {
        let filter_empty = app.active_state().is_some_and(|s| s.filter.is_empty());
        if app.mode == Mode::Normal && filter_empty {
            return KeyOutcome::Quit(EXIT_CANCELLED);
        }
        if app.mode == Mode::Navigate {
            return KeyOutcome::Consumed;
        }
        // Else (NORMAL + text): falls through to filter insert below.
    } else if app.mode == Mode::Navigate {
        return handle_navigate_rune(app, c);
    }
    // NORMAL-mode filter insert (skipped on non-filterable tabs: the
    // rune is consumed so fixed-choice menus ignore typing entirely).
    if app.mode == Mode::Normal {
        if let Some(tab) = app.active_tab_mut() {
            if tab.filterable {
                tab.state.filter.push(c);
            }
        }
        app.clamp_focus();
    }
    KeyOutcome::Consumed
}

/// NAVIGATE-mode runes (wiremix-style `hjkl` navigation, `m` mark/pin,
/// `c` dropdown). Every rune is consumed: NAVIGATE never edits the filter.
fn handle_navigate_rune(app: &mut crate::App, c: char) -> KeyOutcome {
    match c {
        'j' | 'J' => app.move_focus(1),
        'k' | 'K' => app.move_focus(-1),
        'h' | 'H' => app.cycle_tab(-1),
        'l' | 'L' => app.cycle_tab(1),
        'm' | 'M' => {
            // History tabs (`clip`, the only deletable provider) use `m` for
            // pin/unpin: the keystroke exits the TUI with `ACTION:TOGGLE` and
            // the wrapper flips the pin. Every other tab keeps the legacy
            // in-place mark toggle.
            if app.active_tab().is_some_and(|tab| tab.deletable)
                && app.focused_original_index().is_some()
            {
                return KeyOutcome::Toggle;
            }
            toggle_mark(app);
        }
        'c' | 'C' => {
            // Upstream `c` opens the target dropdown; rows without targets
            // keep the keystroke as a no-op.
            app.open_dropdown();
        }
        _ => {}
    }
    KeyOutcome::Consumed
}

/// Toggle the mark on the focused provider row (NAVIGATE `m`).
fn toggle_mark(app: &mut crate::App) {
    if let Some(index) = app.focused_original_index() {
        if let Some(tab) = app.active_tab_mut() {
            if !tab.state.marked.remove(&index) {
                tab.state.marked.insert(index);
            }
        }
    }
}

/// Delete the last whitespace-delimited word (`Ctrl-w`).
fn kill_word(filter: &mut String) {
    while filter.ends_with(' ') {
        filter.pop();
    }
    while !filter.is_empty() && !filter.ends_with(' ') {
        filter.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_quits() {
        let mut menu = crate::Menu::new("power", vec![crate::Tab::empty("a")]);
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            handle_key(&mut menu, key, Instant::now()),
            KeyOutcome::Quit(EXIT_CANCELLED)
        );
    }
}
