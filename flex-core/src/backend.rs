//! Terminal backend: `/dev/tty` alt-screen init/restore, the `ACTION:`
//! single-line stdout writer, and the `0/130/1` exit-code contract.

use std::io::Write as _;

use anyhow::{Context as _, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// Exit code: an action was chosen (`ACTION:` printed).
pub const EXIT_OK: i32 = 0;
/// Exit code: user cancelled (`Esc` / `q` in NORMAL + empty filter).
pub const EXIT_CANCELLED: i32 = 130;
/// Exit code: runtime error (diagnostics on stderr, no `ACTION:` line).
pub const EXIT_ERROR: i32 = 1;

/// Open the controlling terminal (`/dev/tty`) for reading and writing.
///
/// Rendering goes through this handle rather than stdout, and the image
/// preview pane opens a second one: both refer to the same terminal, so
/// bytes land in write order — safe because `Terminal::draw` flushes the
/// frame before the preview writes its graphics escapes.
///
/// # Errors
///
/// Returns the open error when there is no controlling terminal.
pub fn open_tty() -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
}

/// Enter the alternate screen on the controlling terminal (`/dev/tty`).
///
/// All rendering goes through `/dev/tty` — never stdout — so stdout carries
/// exactly one `ACTION:` line even when a wrapper captures it
/// (`out="$(flex …)"`, fzf-style). Key events still come from stdin, which
/// is the popup pty in production and the test pty under e2e. Must be
/// paired with [`restore`].
///
/// # Errors
///
/// Returns an error when `/dev/tty` cannot be opened (no controlling
/// terminal) or the terminal cannot be prepared.
pub fn init() -> Result<Terminal<CrosstermBackend<std::fs::File>>> {
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen};
    let tty = open_tty().context("flex: cannot open /dev/tty (needs a controlling terminal)")?;
    let mut tty_out = tty
        .try_clone()
        .context("flex: cannot clone /dev/tty for rendering")?;
    // Alt-screen first: if raw mode fails below, the shell is never left
    // in raw mode; [`restore`] undoes both.
    crossterm::execute!(tty_out, EnterAlternateScreen)
        .context("flex: cannot enter alternate screen")?;
    if let Err(err) = enable_raw_mode() {
        let _ = disable_raw_mode();
        return Err(err).context("flex: cannot enable raw mode");
    }
    Terminal::new(CrosstermBackend::new(tty_out)).context("flex: cannot create terminal")
}

/// Leave the alternate screen, restoring the user's terminal.
///
/// Best-effort: reopens `/dev/tty` to leave the alternate screen, then
/// disables raw mode. Safe to call even when [`init`] failed partway.
pub fn restore() {
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        let _ = crossterm::execute!(tty, LeaveAlternateScreen);
    }
    let _ = disable_raw_mode();
}

/// Print exactly one `ACTION:` line to stdout.
///
/// Format: `ACTION: <provider> <action_id> <escaped-label>`. Diagnostics must
/// never go through this function — use `eprintln!` instead.
///
/// # Errors
///
/// Returns an error if stdout cannot be written or flushed.
pub fn emit_action(provider: &str, action_id: &str, label: &str) -> Result<()> {
    let escaped = escape_label(label);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "ACTION: {provider} {action_id} {escaped}")
        .context("failed to write ACTION: line")?;
    stdout.flush().context("failed to flush stdout")?;
    Ok(())
}

/// Print exactly one `ACTION:DELETE` line to stdout.
///
/// Format: `ACTION:DELETE <provider> <action_id> <escaped-label>`. Used when
/// a deletable tab (e.g. `clip`) confirms deletion; the wrapper performs the
/// removal. Diagnostics must never go through this function.
///
/// # Errors
///
/// Returns an error if stdout cannot be written or flushed.
pub fn emit_delete(provider: &str, action_id: &str, label: &str) -> Result<()> {
    let escaped = escape_label(label);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "ACTION:DELETE {provider} {action_id} {escaped}")
        .context("failed to write ACTION:DELETE line")?;
    stdout.flush().context("failed to flush stdout")?;
    Ok(())
}

/// Escape backslashes and newlines in a label for the `ACTION:` line.
#[must_use]
pub fn escape_label(label: &str) -> String {
    label.replace('\\', "\\\\").replace('\n', "\\n")
}

/// Print exactly one `ACTION:TOGGLE` line to stdout.
///
/// Format: `ACTION:TOGGLE <provider> <action_id> <escaped-label>`. Used when
/// a deletable tab (e.g. `clip`) toggles a pin via NAVIGATE `m`; the wrapper
/// flips the pin in the store. Diagnostics must never go through this
/// function.
///
/// # Errors
///
/// Returns an error if stdout cannot be written or flushed.
pub fn emit_toggle(provider: &str, action_id: &str, label: &str) -> Result<()> {
    let escaped = escape_label(label);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "ACTION:TOGGLE {provider} {action_id} {escaped}")
        .context("failed to write ACTION:TOGGLE line")?;
    stdout.flush().context("failed to flush stdout")?;
    Ok(())
}

/// Print exactly one `ACTION:TARGET` line to stdout.
///
/// Format: `ACTION:TARGET <provider> <action_id> <target_id> <escaped-target-title>`.
/// Used when a row's target dropdown chooses a target (upstream
/// `Action::SetTarget`); the wrapper applies the routing change. Diagnostics
/// must never go through this function.
///
/// # Errors
///
/// Returns an error if stdout cannot be written or flushed.
pub fn emit_target(
    provider: &str,
    action_id: &str,
    target_id: &str,
    target_title: &str,
) -> Result<()> {
    let line = target_line(provider, action_id, target_id, target_title);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}").context("failed to write ACTION:TARGET line")?;
    stdout.flush().context("failed to flush stdout")?;
    Ok(())
}

/// Build the `ACTION:TARGET` line (see [`emit_target`] for the format).
///
/// Split out so the format and escaping are unit-testable without touching
/// the process's stdout.
#[must_use]
pub fn target_line(provider: &str, action_id: &str, target_id: &str, target_title: &str) -> String {
    format!(
        "ACTION:TARGET {provider} {action_id} {target_id} {}",
        escape_label(target_title)
    )
}

/// Whether deterministic test mode is on (`FLEX_TEST=1`).
///
/// In test mode the event loop uses a fixed poll timeout and seeded
/// timestamps so golden/key replays are reproducible. Production paths are
/// identical apart from timing sources.
#[must_use]
pub fn is_flex_test() -> bool {
    std::env::var("FLEX_TEST").is_ok_and(|value| value == "1")
}

/// Fixed RNG seed for deterministic fixtures in `FLEX_TEST` mode.
///
/// Remainder of the seed-case matrix (full key-space coverage) is
/// `TODO(M6)`; see `tests/keys.rs`.
pub const FLEX_TEST_SEED: u64 = 0xF1E07u64;

/// Event-poll timeout: the 1 s gauge tick cadence (Q7).
///
/// When a gauge is visible the loop rerenders on this tick; otherwise ticks
/// never touch focus or selection (see `App::tick`, `tests/keys.rs`).
#[must_use]
pub const fn poll_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_label_round_trips_backslash_and_newline() {
        assert_eq!(escape_label("a\\b\nc"), "a\\\\b\\nc");
    }
}

#[cfg(test)]
mod target_line_tests {
    use super::*;

    #[test]
    fn target_line_format() {
        assert_eq!(
            target_line("center", "vol", "t-hdmi", "HDMI"),
            "ACTION:TARGET center vol t-hdmi HDMI"
        );
    }

    #[test]
    fn target_line_escapes_the_title() {
        assert_eq!(
            target_line("center", "vol", "t0", "Multi\nline\\title"),
            "ACTION:TARGET center vol t0 Multi\\nline\\\\title"
        );
    }
}
