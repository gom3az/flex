//! `flex-bt` binary: native Bluetooth manager Wiremix popup.
//!
//! Provides an interactive Bluetooth manager with device connection, battery level,
//! audio profile switching, adapter toggles, and background status refreshing.

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::bt;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Native Bluetooth manager Wiremix popup.
#[derive(Debug, Parser)]
#[command(name = "flex-bt", version, about = "Native Bluetooth manager")]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
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

    runner::popup_guard(Provider::Bt)?;

    if cli.print_action {
        return runner::run_select(Provider::Bt, style);
    }

    let menu = runner::build_menu(Provider::Bt, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen {
            action_id, label, ..
        }
        | Outcome::Toggle {
            action_id, label, ..
        } => {
            bt::execute(&action_id, &label, None)?;
            Ok(())
        }
        Outcome::Target { target, title, .. } => {
            bt::execute(&target, &title, None)?;
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            if let Some(mac) = action_id.strip_prefix("device:") {
                bt::remove(mac, None)?;
                Ok(())
            } else {
                anyhow::bail!("bt: unexpected delete outcome for '{action_id}'")
            }
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
