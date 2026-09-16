//! `flex-net` binary: network throughput monitor & process bandwidth manager.
//!
//! Provides both headless Waybar reporting (`--json`, `--stream`) and an interactive
//! Wiremix TUI (`flex-net`) showing Top Bandwidth Consumers and Network Interfaces.

use std::time::Duration;

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::net::{self, format_speed, scan_top_talkers};
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Network throughput monitor and bandwidth manager.
#[derive(Debug, Parser)]
#[command(
    name = "flex-net",
    version,
    about = "Network throughput monitor and process bandwidth manager"
)]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
    #[command(flatten)]
    style: GlobalStyle,

    /// Emit single Waybar JSON payload and exit.
    #[arg(short = 'j', long)]
    json: bool,

    /// Print top network consuming processes to stdout.
    #[arg(short = 'T', long)]
    top: bool,

    /// Run continuously, streaming JSON lines at the specified interval in seconds.
    #[arg(short = 'S', long, value_name = "SECS")]
    stream: Option<u64>,

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

    if cli.top {
        let talkers = scan_top_talkers();
        if talkers.is_empty() {
            println!("No active network consuming processes found.");
        } else {
            println!(
                "{:<8} {:<20} {:<15} {:<15} {:<8}",
                "PID", "COMMAND", "RX RATE", "TX RATE", "SHARE"
            );
            for p in &talkers {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let share_pct = (p.share * 100.0).round() as u32;
                println!(
                    "{:<8} {:<20} {:<15} {:<15} {:>3}%",
                    p.pid,
                    p.comm,
                    format_speed(p.rx_rate),
                    format_speed(p.tx_rate),
                    share_pct
                );
            }
        }
        return Ok(());
    }

    if let Some(secs) = cli.stream {
        let interval = Duration::from_secs(secs.max(1));
        return net::stream(interval);
    }

    if cli.json {
        println!("{}", net::sample_json());
        return Ok(());
    }

    let style = cli.style.options();
    runner::popup_guard(Provider::Net)?;

    if cli.print_action {
        return runner::run_select(Provider::Net, style);
    }

    let menu = runner::build_menu(Provider::Net, style)?;
    match flex_core::run::run_capture(menu)? {
        Outcome::Chosen { action_id, .. } => {
            if let Some(pid) = action_id.strip_prefix("proc:") {
                net::execute(&format!("signal:{pid}:SIGTERM"))?;
            } else {
                net::execute(&action_id)?;
            }
            Ok(())
        }
        Outcome::Delete { action_id, .. } => {
            if let Some(pid) = action_id.strip_prefix("proc:") {
                net::execute(&format!("signal:{pid}:SIGKILL"))?;
            }
            Ok(())
        }
        Outcome::Toggle { action_id, .. } => {
            if let Some(pid) = action_id.strip_prefix("proc:") {
                net::execute(&format!("signal:{pid}:SIGSTOP"))?;
            }
            Ok(())
        }
        Outcome::Target { target, .. } => {
            net::execute(&target)?;
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
