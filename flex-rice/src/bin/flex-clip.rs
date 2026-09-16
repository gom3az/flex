//! `flex-clip` binary: the clipboard history.
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+execute through [`exec::clip`] (the port of the retired
//! `flex-clip.sh` wrapper). `--print-action` keeps the end-to-end probe of
//! the row→action mapping with no execution.
//!
//! Clip rows are deletable, so unlike `shot`/`theme`/`wallpaper`/`launch`
//! (whose bins bail on `Delete`/`Toggle` as unexpected) this bin handles
//! `Outcome::Delete` (remove the entry) and `Outcome::Toggle` (flip the pin)
//! via [`exec::clip`]; only `Outcome::Target` bails (clip rows carry no
//! dropdown targets).
//!
//! [`runner`]: flex_rice::runner
//! [`exec::clip`]: flex_rice::exec::clip

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::clip::{self, ClipOp};
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Clipboard history: select a row and copy, delete, or (un)pin it.
#[derive(Debug, Parser)]
#[command(name = "flex-clip", version, about = "Clipboard history")]
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
/// action id is unknown, or the copy/delete/toggle cannot run. The error
/// carries no `flex:` prefix; `main` adds it via the runner.
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    runner::popup_guard(Provider::Clip)?;
    if cli.print_action {
        // Probe path: exercise the real row→action mapping, never execute.
        return runner::run_select(Provider::Clip, style);
    }
    let menu = runner::build_menu(Provider::Clip, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen { action_id, .. } => {
            clip::execute(ClipOp::Copy, &action_id, None)?;
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            clip::execute(ClipOp::Delete, &action_id, None)?;
            Ok(())
        }
        Outcome::Toggle { action_id, .. } => {
            clip::execute(ClipOp::Toggle, &action_id, None)?;
            Ok(())
        }
        Outcome::Target { row, .. } => {
            anyhow::bail!("clip: unexpected target outcome for '{row}'")
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
