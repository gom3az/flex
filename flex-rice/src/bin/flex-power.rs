//! `flex-power` binary: the power menu (shutdown/reboot/…).
//!
//! Thin shell over the shared [`runner`]: popup guard, menu
//! construction, then select+execute through [`exec::power`] (the
//! port of the retired `flex-power.sh` wrapper). `--print-action`
//! keeps the end-to-end probe of the row→action mapping with no
//! execution.
//!
//! Outcome mapping mirrors the wrapper's arms (`flex-power.sh:51-57`):
//! `Chosen` runs the `select` dispatch (`lock`, `suspend`,
//! `reboot`, `poweroff`, `logout`). `Delete`/`Toggle`/`Target` are
//! unreachable from the power surface (rows are non-deletable and
//! carry no toggle/target), so they bail like the theme/launch ports
//! — the wrapper's case pattern also accepts `ACTION:DELETE`, but no
//! power row can emit it.
//!
//! Danger rows (Reboot/Poweroff) are confirmed UI-side by the shared
//! double-Enter flow before SELECT ever arrives here — this bin
//! never re-prompts.
//!
//! [`runner`]: flex_rice::runner
//! [`exec::power`]: flex_rice::exec::power

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::power;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Power menu: select a row, run its power effect.
#[derive(Debug, Parser)]
#[command(name = "flex-power", version, about = "Power menu (shutdown/reboot/…)")]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
    #[command(flatten)]
    style: GlobalStyle,

    /// Print the selected `ACTION:` line without executing it: an
    /// end-to-end probe of the real binary's row→action mapping
    /// with no pty.
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
/// Returns an error when the popup toggle or the select loop
/// fails, the action id is unknown, or a loud effect (hyprlock,
/// systemctl) cannot run. The error carries no `flex:` prefix;
/// `main` adds it via the runner.
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let style = cli.style.options();
    runner::popup_guard(Provider::Power)?;
    if cli.print_action {
        // Probe path: exercise the real row→action mapping, never execute.
        return runner::run_select(Provider::Power, style);
    }
    let menu = runner::build_menu(Provider::Power, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen { action_id, .. } => {
            power::execute(&action_id, None)?;
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            anyhow::bail!("power: unexpected delete outcome for '{action_id}'")
        }
        Outcome::Toggle { action_id, .. } => {
            anyhow::bail!("power: unexpected toggle outcome for '{action_id}'")
        }
        Outcome::Target { row, .. } => {
            anyhow::bail!("power: unexpected target outcome for '{row}'")
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
