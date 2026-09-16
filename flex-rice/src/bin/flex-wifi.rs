//! `flex-wifi` binary: the Wi-Fi picker (connect/disconnect, radio on/off).
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+print. The execute phase is still a stub (see the
//! `TODO(executor)` below); the shell wrapper owns the side effects.
//!
//! [`runner`]: flex_rice::runner

use clap::Parser;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Wi-Fi picker: select a row, print `ACTION:`, let the wrapper act.
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

/// Parse args, guard the popup, then select.
///
/// # Errors
///
/// Returns an error when the popup toggle or the select loop fails. The
/// error carries no `flex:` prefix; `main` adds it via the runner.
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    runner::popup_guard(Provider::Wifi)?;
    if cli.print_action {
        // Probe path: exercise the real row→action mapping, never execute.
        return runner::run_select(Provider::Wifi, style);
    }
    // TODO(executor): the execute phase is a stub — selection prints the
    // ACTION: line and exits 0; the real wifi executor lands later.
    runner::run_select(Provider::Wifi, style)
}
