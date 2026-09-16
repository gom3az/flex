//! `flex-wifi` binary: the Wi-Fi picker (connect/disconnect, radio on/off).
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+execute through [`exec::wifi`] (the port of the retired
//! `flex-wifi.sh` wrapper). `--print-action` keeps the end-to-end probe of
//! the row→action mapping with no execution.
//!
//! Outcome mapping mirrors the wrapper's arms (`flex-wifi.sh:30-38,174-185`):
//! `Chosen` runs the `select` dispatch — the wrapper accepts exactly one
//! line shape, `ACTION: wifi …`, and exits `1` on anything else. `Delete`,
//! `Toggle` and `Target` therefore bail: the wrapper has no `ACTION:DELETE`
//! / `ACTION:TOGGLE` / `ACTION:TARGET` arm anywhere in its 186 lines (wifi
//! rows are never deletable, carry no pin flow, and have no dropdown
//! targets), so all three are genuinely unreachable.
//!
//! [`runner`]: flex_rice::runner
//! [`exec::wifi`]: flex_rice::exec::wifi

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::wifi;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Wi-Fi picker: select a row and run its radio/connect effect.
#[derive(Debug, Parser)]
#[command(name = "flex-wifi", version, about = "Wi-Fi picker")]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
    #[command(flatten)]
    style: GlobalStyle,

    /// Print the selected `ACTION:` line without executing it: an end-to-end
    /// probe of the real binary's row→action mapping with no pty.
    #[arg(long)]
    print_action: bool,
}

fn main() {
    // `flex: error:` is added once, in the runner, and nowhere else.
    if let Err(err) = run() {
        runner::fail(&err);
    }
}

/// Parse args, guard the popup, then select+execute.
///
/// # Errors
///
/// Returns an error when the popup toggle or the select loop fails, the
/// action id is unknown, or the outcome is unreachable for wifi rows
/// (`Delete`/`Toggle`/`Target` — the wrapper has no arm for any of them).
/// The error carries no `flex:` prefix; `main` adds it via the runner.
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    runner::popup_guard(Provider::Wifi)?;
    if cli.print_action {
        // Probe path: exercise the real row→action mapping, never execute.
        return runner::run_select(Provider::Wifi, style);
    }
    let menu = runner::build_menu(Provider::Wifi, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen {
            action_id, label, ..
        } => {
            wifi::execute(&action_id, &label, None)?;
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            anyhow::bail!("wifi: unexpected delete outcome for '{action_id}'")
        }
        Outcome::Toggle { action_id, .. } => {
            anyhow::bail!("wifi: unexpected toggle outcome for '{action_id}'")
        }
        Outcome::Target { row, .. } => {
            anyhow::bail!("wifi: unexpected target outcome for '{row}'")
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
