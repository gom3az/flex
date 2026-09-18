//! `flex-theme` binary: the theme switcher.
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+execute through [`exec::theme`] (the port of the retired
//! `flex-theme.sh` wrapper). `--print-action` keeps the end-to-end probe of
//! the row→action mapping with no execution.
//!
//! [`runner`]: flex_rice::runner
//! [`exec::theme`]: flex_rice::exec::theme

use clap::{Parser, Subcommand};
use flex_core::Outcome;
use flex_rice::exec::theme;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Theme switcher: select a row and activate it; or run one of the
/// `list`/`current`/`activate`/`delete` verbs.
#[derive(Debug, Parser)]
#[command(name = "flex-theme", version, about = "Theme switcher")]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
    #[command(flatten)]
    style: GlobalStyle,

    /// Print the selected `ACTION:` line without executing it: an end-to-end
    /// probe of the real binary's row→action mapping with no pty.
    #[arg(long)]
    print_action: bool,

    /// Non-interactive verb; without one the interactive picker runs.
    #[command(subcommand)]
    op: Option<Op>,
}

/// The ported `theme-switcher.sh` entry points (`pick` is the default TUI).
#[derive(Debug, Subcommand)]
enum Op {
    /// List available themes (name, wallpaper, generated).
    List,
    /// Print the active theme and wallpaper.
    Current,
    /// Activate a theme by name.
    Activate {
        /// Theme (directory) name.
        name: String,
    },
    /// Delete an available theme by name.
    Delete {
        /// Theme (directory) name.
        name: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    runner::init_logging();
    // `flex: error:` is added once, in the runner, and nowhere else.
    if let Err(err) = run().await {
        runner::fail(&err);
    }
}

/// Parse args: a CLI verb runs directly (no popup, no TUI); otherwise guard
/// the popup and select+execute.
///
/// # Errors
///
/// Returns an error when a verb fails, the popup toggle or the select loop
/// fails, the action id is unknown, or the theme switcher cannot run. The
/// error carries no `flex:` prefix; `main` adds it via the runner.
async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(op) = cli.op {
        return match op {
            Op::List => theme::list(),
            Op::Current => theme::current(),
            Op::Activate { name } => theme::activate(&name, None),
            Op::Delete { name } => theme::delete(&name),
        };
    }
    let style = cli.style.options();
    runner::run_standard_cli(
        Provider::Theme,
        style,
        cli.print_action,
        |outcome| match outcome {
            Outcome::Chosen { action_id, .. } => {
                theme::execute(&action_id, None)?;
                Ok(())
            }
            Outcome::Delete { action_id, .. } => {
                anyhow::bail!("theme: unexpected delete outcome for '{action_id}'")
            }
            Outcome::Toggle { action_id, .. } => {
                anyhow::bail!("theme: unexpected toggle outcome for '{action_id}'")
            }
            Outcome::Target { row, .. } => {
                anyhow::bail!("theme: unexpected target outcome for '{row}'")
            }
            _ => unreachable!(),
        },
    )
    .await
}
