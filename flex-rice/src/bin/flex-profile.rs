//! `flex-profile` binary: power profile menu.

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::profile;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Profile menu: select a power profile and set it.
#[derive(Debug, Parser)]
#[command(
    name = "flex-profile",
    version,
    about = "Power profile menu (performance/balanced/power-saver)"
)]
struct Cli {
    /// Global presentation flags.
    #[command(flatten)]
    style: GlobalStyle,

    /// Print the selected `ACTION:` line without executing it.
    #[arg(long)]
    print_action: bool,
}

fn main() {
    if let Err(err) = run() {
        runner::fail(&err);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    runner::popup_guard(Provider::Profile)?;
    if cli.print_action {
        return runner::run_select(Provider::Profile, style);
    }
    let menu = runner::build_menu(Provider::Profile, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen { action_id, .. } => {
            profile::execute(&action_id, None)?;
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            anyhow::bail!("profile: unexpected delete outcome for '{action_id}'")
        }
        Outcome::Toggle { action_id, .. } => {
            anyhow::bail!("profile: unexpected toggle outcome for '{action_id}'")
        }
        Outcome::Target { row, .. } => {
            anyhow::bail!("profile: unexpected target outcome for '{row}'")
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
