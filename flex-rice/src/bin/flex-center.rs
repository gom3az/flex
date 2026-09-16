//! `flex-center` binary: the control center (volume/brightness/network).
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+execute through [`exec::center`] (the port of
//! `wrappers/flex-center.sh`, which stays live until cutover). `--print-action`
//! keeps the end-to-end probe of the row→action mapping with no execution.
//!
//! Outcome mapping mirrors the wrapper's arms (`flex-center.sh:32-44,168-200`):
//! `Chosen` runs the `select` dispatch, `Toggle` runs the `toggle` dispatch
//! (`vol` mutes, `bt:*` flips, everything else no-ops), `Delete` exits `0`
//! with no effect (center rows are never data — the wrapper exits before id
//! validation), and only `Target` bails (no `ACTION:TARGET` arm, and no
//! center row carries dropdown targets).
//!
//! [`runner`]: flex_rice::runner
//! [`exec::center`]: flex_rice::exec::center

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::center::{self, CenterOp};
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Control center: select a row and run its volume/network/power effect.
#[derive(Debug, Parser)]
#[command(name = "flex-center", version, about = "Control center")]
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
/// action id is unknown, or a loud effect (launch/power/theme) cannot run.
/// The error carries no `flex:` prefix; `main` adds it via the runner.
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    runner::popup_guard(Provider::Center)?;
    if cli.print_action {
        // Probe path: exercise the real row→action mapping, never execute.
        return runner::run_select(Provider::Center, style);
    }
    let menu = runner::build_menu(Provider::Center, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen {
            action_id, label, ..
        } => {
            center::execute(CenterOp::Select, &action_id, &label, None)?;
            Ok(())
        }
        Outcome::Delete { .. } => {
            // The wrapper no-ops every DELETE before id validation.
            Ok(())
        }
        Outcome::Toggle {
            action_id, label, ..
        } => {
            center::execute(CenterOp::Toggle, &action_id, &label, None)?;
            Ok(())
        }
        Outcome::Target { row, .. } => {
            anyhow::bail!("center: unexpected target outcome for '{row}'")
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
