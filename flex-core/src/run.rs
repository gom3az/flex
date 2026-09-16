//! Event loop: `ratatui::init` on `/dev/tty`, poll → dispatch → render.
//!
//! Frame cadence mirrors `Docs/UI_UX_doc.md`: every frame sizes exactly once
//! (inside [`render::render`]), crossterm events dispatch through
//! [`keys::handle_key`] with an injected `Instant`, and the 1 s
//! [`backend::poll_timeout`] expiry drives [`Menu::tick`] (gauge cadence +
//! danger-arm expiry). Resizes need no explicit handling: the next frame
//! re-sizes and re-renders.
//!
//! Outcomes: `Select` prints one `ACTION:` line and exits `0`; `Delete`
//! prints one `ACTION:DELETE` line and exits `0` (launch/power/shot/theme
//! tabs leave `Tab::deletable` false so this never fires there); `Toggle`
//! prints one `ACTION:TOGGLE` line and exits `0` (clip NAVIGATE `m`, the
//! wrapper flips the pin); `Quit(code)` exits with `code` and prints
//! nothing. Diagnostics go to stderr.
//!
//! `FLEX_TEST=1` replays: the loop uses a fixed base plus [`FLEX_TEST_STEP`]
//! per event instead of the wall clock, so scripted runs are deterministic.
//! [`replay_keys`] is the test-facing form of the same dispatch (no TTY).
//!
//! Image previews (`menu.preview`): after every frame the reserved pane is
//! handed to [`preview::Preview`], which paints the focused row's image with
//! kitty graphics escapes and re-parks the caret. The pane is the only part
//! of the frame that is drawn outside the ratatui buffer.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use crossterm::event::{Event, KeyEvent};

use crate::backend;
use crate::keys::{self, KeyOutcome};
use crate::preview;
use crate::render;
use crate::Menu;

/// Deterministic per-event step for `FLEX_TEST` replays.
///
/// Derived from [`backend::FLEX_TEST_SEED`] so a seed change visibly retimes
/// replays; the value stays above [`keys::ARM_CONFIRM_DELAY`] so a scripted
/// double-`Enter` still confirms danger rows.
pub const FLEX_TEST_STEP: Duration = Duration::from_millis(backend::FLEX_TEST_SEED % 100 + 50);

/// Fixed-base clock for deterministic tests: one [`Instant`] per replay,
/// event `i` stamped at `base + i * FLEX_TEST_STEP`.
#[must_use]
pub fn test_base() -> Instant {
    Instant::now()
}

/// Feed synthetic keys through [`keys::handle_key`] deterministically.
///
/// Stamps are `base + i * FLEX_TEST_STEP` (no sleeps, no clock reads).
/// Returns the first terminal outcome (`Select`/`Delete`/`Toggle`/`Quit`);
/// returns `Consumed` when the script never terminates (caller asserts
/// state).
#[must_use]
pub fn replay_keys(menu: &mut Menu, keys: &[KeyEvent], base: Instant) -> KeyOutcome {
    for (index, key) in keys.iter().enumerate() {
        // Scripts are short; the index never approaches `u32::MAX`.
        #[allow(clippy::cast_possible_truncation)]
        let step = index as u32;
        let outcome = keys::handle_key(menu, *key, base + FLEX_TEST_STEP * step);
        if outcome != KeyOutcome::Consumed {
            return outcome;
        }
    }
    KeyOutcome::Consumed
}

/// Run the interactive menu until it selects, deletes, quits, or errors.
///
/// See the module docs for the frame/outcome contract. Diverges via
/// [`std::process::exit`] on terminal outcomes (matching `main.rs` stub
/// behavior); returns `Ok` only on TTY errors handled by the caller.
///
/// # Errors
///
/// Returns an error when the terminal cannot initialize, frames cannot
/// draw, or events cannot be polled/read.
pub fn run(mut menu: Menu) -> Result<()> {
    let provider = menu.provider.clone();
    let mut terminal = backend::init()?;
    let base = Instant::now();
    let seeded = backend::is_flex_test();
    let mut step: u32 = 0;
    let mut images = menu.preview.then(preview::Preview::new);
    // Second handle on the same terminal for the out-of-band image writes;
    // without it the menu still runs, just without previews.
    let mut preview_tty = if menu.preview {
        backend::open_tty().ok()
    } else {
        None
    };
    loop {
        let mut area = ratatui::layout::Rect::default();
        terminal
            .draw(|frame| {
                area = frame.area();
                render::render(frame, &mut menu);
            })
            .context("failed to draw frame")?;
        if let (Some(images), Some(tty)) = (images.as_mut(), preview_tty.as_mut()) {
            // Best effort: a preview that cannot be drawn (no kitty graphics,
            // unreadable image, converter missing) leaves the pane blank and
            // must never take the menu down with it.
            let pane = render::preview_area(area, &menu);
            let source = menu
                .app
                .focused_row()
                .and_then(|row| row.preview_image.clone());
            let park = render::cursor_position(area, &menu);
            let _ = images.sync(tty, pane, source.as_deref().map(std::path::Path::new), park);
        }
        if crossterm::event::poll(backend::poll_timeout()).context("event poll failed")? {
            match crossterm::event::read().context("event read failed")? {
                Event::Key(key) => {
                    let now = loop_now(base, &mut step, seeded);
                    match keys::handle_key(&mut menu, key, now) {
                        KeyOutcome::Consumed => {}
                        KeyOutcome::Select => {
                            let Some(row) = menu.app.focused_row() else {
                                continue;
                            };
                            let (id, label) = (row.id.as_str().to_string(), row.label.clone());
                            backend::restore();
                            backend::emit_action(&provider, &id, &label)?;
                            std::process::exit(backend::EXIT_OK);
                        }
                        KeyOutcome::Delete => {
                            let Some(row) = menu.app.focused_row() else {
                                continue;
                            };
                            let (id, label) = (row.id.as_str().to_string(), row.label.clone());
                            backend::restore();
                            backend::emit_delete(&provider, &id, &label)?;
                            std::process::exit(backend::EXIT_OK);
                        }
                        KeyOutcome::Toggle => {
                            let Some(row) = menu.app.focused_row() else {
                                continue;
                            };
                            let (id, label) = (row.id.as_str().to_string(), row.label.clone());
                            backend::restore();
                            backend::emit_toggle(&provider, &id, &label)?;
                            std::process::exit(backend::EXIT_OK);
                        }
                        KeyOutcome::ChooseTarget { row, target, title } => {
                            backend::restore();
                            backend::emit_target(&provider, row.as_str(), target.as_str(), &title)?;
                            std::process::exit(backend::EXIT_OK);
                        }
                        KeyOutcome::Quit(code) => {
                            backend::restore();
                            std::process::exit(code);
                        }
                    }
                }
                // Resize: the next frame re-sizes once (see `render`), and
                // focus/mouse/paste events never affect the menu.
                Event::Resize(_, _)
                | Event::FocusGained
                | Event::FocusLost
                | Event::Mouse(_)
                | Event::Paste(_) => {}
            }
        } else {
            menu.tick(loop_now(base, &mut step, seeded));
        }
    }
}

/// Loop clock: seeded fixed-step in `FLEX_TEST` mode, wall clock otherwise.
fn loop_now(base: Instant, step: &mut u32, seeded: bool) -> Instant {
    if seeded {
        let now = base + FLEX_TEST_STEP * *step;
        *step = step.saturating_add(1);
        now
    } else {
        Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flex_test_step_confirms_confirmable_in_replays() {
        assert!(FLEX_TEST_STEP >= keys::ARM_CONFIRM_DELAY);
        assert!(FLEX_TEST_STEP < keys::ARM_EXPIRE);
    }
}
