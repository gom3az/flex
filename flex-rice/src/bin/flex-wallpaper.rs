//! `flex-wallpaper` binary: the wallpaper picker (image previews).
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+execute through [`exec::wallpaper`] (the port of
//! `wrappers/flex-wallpaper.sh`, which stays live until cutover). `--print-action`
//! keeps the end-to-end probe of the row→action mapping with no execution.
//!
//! [`runner`]: flex_rice::runner
//! [`exec::wallpaper`]: flex_rice::exec::wallpaper

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::wallpaper;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Wallpaper picker: select a row and set it.
#[derive(Debug, Parser)]
#[command(name = "flex-wallpaper", version, about = "Wallpaper picker")]
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
/// action id is unknown, or the wallpaper setter cannot run. The error
/// carries no `flex:` prefix; `main` adds it via the runner.
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    runner::popup_guard(Provider::Wallpaper)?;
    if cli.print_action {
        // Probe path: exercise the real row→action mapping, never execute.
        return runner::run_select(Provider::Wallpaper, style);
    }
    let menu = runner::build_menu(Provider::Wallpaper, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen { action_id, .. } => {
            wallpaper::execute(&action_id, None)?;
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            anyhow::bail!("wallpaper: unexpected delete outcome for '{action_id}'")
        }
        Outcome::Toggle { action_id, .. } => {
            anyhow::bail!("wallpaper: unexpected toggle outcome for '{action_id}'")
        }
        Outcome::Target { row, .. } => {
            anyhow::bail!("wallpaper: unexpected target outcome for '{row}'")
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
