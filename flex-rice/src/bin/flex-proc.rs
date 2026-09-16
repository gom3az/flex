//! `flex-proc` binary: the native process manager.
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+signal through [`exec::proc`]. Replaces the htop-based
//! `kill-menu.sh` with a filterable `/proc` list.
//!
//! Outcome mapping (rows are `confirmable` and the tab `deletable`):
//! `Chosen` (Enter, armed) → SIGTERM; `Delete` (armed) → SIGKILL; `Toggle`
//! (NAVIGATE `m`) → SIGSTOP/SIGCONT. `Target` bails (no dropdown targets).
//! `--print-action` keeps the end-to-end probe of the row→action mapping.
//!
//! [`runner`]: flex_rice::runner
//! [`exec::proc`]: flex_rice::exec::proc

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::proc::{self, Signal};
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Process manager: filter processes and signal the selection.
#[derive(Debug, Parser)]
#[command(name = "flex-proc", version, about = "Process manager")]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
    #[command(flatten)]
    style: GlobalStyle,

    /// Sort order (mem, cpu, pid, name). Defaults to memory consumption.
    #[arg(short = 'o', long, default_value = "mem", value_name = "FIELD")]
    sort: String,

    /// Expand all services by default.
    #[arg(long)]
    expand_services: bool,

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

/// Parse args, guard the popup, then select+signal.
///
/// # Errors
///
/// Returns an error when the popup toggle or the select loop fails, or the
/// signal cannot run. The error carries no `flex:` prefix; `main` adds it via
/// the runner.
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.expand_services {
        std::env::set_var("FLEX_PROC_EXPAND", "all");
    }
    std::env::set_var("FLEX_PROC_SORT", &cli.sort);
    let style = cli.style.options();
    runner::popup_guard(Provider::Proc)?;
    if cli.print_action {
        // Probe path: exercise the real row→action mapping, never execute.
        return runner::run_select(Provider::Proc, style);
    }
    let menu = runner::build_menu(Provider::Proc, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen { action_id, .. } => {
            proc::signal(&action_id, Signal::Term, None)?;
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            proc::signal(&action_id, Signal::Kill, None)?;
            Ok(())
        }
        Outcome::Toggle { action_id, .. } => {
            proc::toggle(&action_id, None)?;
            Ok(())
        }
        Outcome::Target { target, .. } => {
            proc::signal(&target, Signal::Term, None)?;
            Ok(())
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
