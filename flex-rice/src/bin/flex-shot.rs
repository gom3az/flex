//! `flex-shot` binary: the screenshot flow.
//!
//! Thin shell over the shared [`runner`]: popup guard, menu construction,
//! then select+execute through [`exec::shot`] (the port of
//! `wrappers/flex-shot.sh`, which stays live until cutover). `--print-action`
//! keeps the end-to-end probe of the row→action mapping with no execution;
//! `--capture` is the hidden detached worker the parent spawns via `setsid`
//! so the capture survives the popup kill.
//!
//! [`runner`]: flex_rice::runner
//! [`exec::shot`]: flex_rice::exec::shot

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::shot;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Screenshot flow: select a row, capture, let the popup die.
#[derive(Debug, Parser)]
#[command(name = "flex-shot", version, about = "Screenshot flow")]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
    #[command(flatten)]
    style: GlobalStyle,

    /// Print the selected `ACTION:` line without executing it: an end-to-end
    /// probe of the real binary's row→action mapping with no pty.
    #[arg(long)]
    print_action: bool,

    /// Hidden detached capture worker: run one capture synchronously with no
    /// menu and no popup guard (spawned by the parent via `setsid -f`, never
    /// typed by hand).
    #[arg(long, hide = true, num_args = 3, value_names = ["ID", "FILEPATH", "REC_START"])]
    capture: Option<Vec<String>>,
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
/// Returns an error when the worker args are malformed, the popup toggle or
/// the select loop fails, the action id is unknown, or the capture cannot be
/// staged/detached. The error carries no `flex:` prefix; `main` adds it via
/// the runner.
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(parts) = cli.capture {
        return run_capture_worker(parts);
    }
    let style = cli.style.options();
    runner::popup_guard(Provider::Shot)?;
    if cli.print_action {
        // Probe path: exercise the real row→action mapping, never execute.
        return runner::run_select(Provider::Shot, style);
    }
    let menu = runner::build_menu(Provider::Shot, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen { action_id, .. } => {
            shot::execute(&action_id, None)?;
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            anyhow::bail!("shot: unexpected delete outcome for '{action_id}'")
        }
        Outcome::Toggle { action_id, .. } => {
            anyhow::bail!("shot: unexpected toggle outcome for '{action_id}'")
        }
        Outcome::Target { row, .. } => {
            anyhow::bail!("shot: unexpected target outcome for '{row}'")
        }
        Outcome::Quit { code } => {
            std::process::exit(code);
        }
        Outcome::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}

/// Run one capture synchronously for the detached `--capture` worker.
///
/// # Errors
///
/// When the worker args are malformed, the id is unknown, or the capture
/// fails. A user-cancelled `slurp` pick exits 130 with no output instead.
fn run_capture_worker(parts: Vec<String>) -> anyhow::Result<()> {
    let mut parts = parts.into_iter();
    let (Some(id), Some(file), Some(rec)) = (parts.next(), parts.next(), parts.next()) else {
        anyhow::bail!("shot: --capture needs ID FILEPATH REC_START");
    };
    let Some(parsed) = shot::ShotId::parse(&id) else {
        anyhow::bail!("shot: unknown id '{id}'");
    };
    match shot::run_worker(
        parsed,
        std::path::Path::new(&file),
        std::path::Path::new(&rec),
        None,
    )? {
        shot::CaptureEnd::Done => Ok(()),
        shot::CaptureEnd::Cancelled => {
            std::process::exit(EXIT_CANCELLED);
        }
    }
}
