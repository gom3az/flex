//! `flex-launch` binary: the application launcher.
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+execute through [`exec::launch`] (the port of the retired
//! `flex-launch.sh` wrapper). `--print-action` keeps the end-to-end probe of
//! the row→action mapping with no execution.
//!
//! [`runner`]: flex_rice::runner
//! [`exec::launch`]: flex_rice::exec::launch

use clap::Parser;
use flex_core::Outcome;
use flex_rice::exec::launch;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Application launcher: select a row and launch it.
#[derive(Debug, Parser)]
#[command(name = "flex-launch", version, about = "Application launcher")]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
    #[command(flatten)]
    style: GlobalStyle,

    /// Print the selected `ACTION:` line without executing it: an end-to-end
    /// probe of the real binary's row→action mapping with no pty.
    #[arg(long)]
    print_action: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    runner::init_logging();
    // `flex: error:` is added once, in the runner, and nowhere else.
    if let Err(err) = run().await {
        runner::fail(&err);
    }
}

/// Parse args, guard the popup, then select+execute.
///
/// # Errors
///
/// Returns an error when the popup toggle or the select loop fails, the
/// action id is unknown, or the launch cannot run. The error carries no
/// `flex:` prefix; `main` adds it via the runner.
async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    runner::run_standard_cli(
        Provider::Launch,
        style,
        cli.print_action,
        |outcome| match outcome {
            Outcome::Chosen { action_id, .. } => {
                launch::execute(&action_id, None)?;
                Ok(())
            }
            Outcome::Delete { action_id, .. } => {
                anyhow::bail!("launch: unexpected delete outcome for '{action_id}'")
            }
            Outcome::Toggle { action_id, .. } => {
                anyhow::bail!("launch: unexpected toggle outcome for '{action_id}'")
            }
            Outcome::Target { row, .. } => {
                anyhow::bail!("launch: unexpected target outcome for '{row}'")
            }
            _ => unreachable!(),
        },
    )
    .await
}
