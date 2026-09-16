//! `flex-net` binary: network throughput monitor & process bandwidth manager.
//!
//! Provides both headless Waybar reporting (default, `--stream`), an interactive
//! Wiremix TUI (`flex-net -m` or `flex net`) showing Top Bandwidth Consumers, Network Interfaces,
//! and Speedtest Benchmark, and a fast terminal benchmark (`flex-net -B` / `flex-net --speedtest`).

use std::time::Duration;

use clap::Parser;
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::net::{self, format_speed, scan_top_talkers};
use flex_rice::exec::speedtest;
use flex_rice::runner::{self, GlobalStyle, Provider};

/// Network throughput monitor and bandwidth manager.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[command(
    name = "flex-net",
    version,
    about = "Network throughput monitor and process bandwidth manager"
)]
struct Cli {
    /// Global presentation flags (upstream `-s/-t/-p/--filter-mode`).
    #[command(flatten)]
    style: GlobalStyle,

    /// Open interactive Wiremix TUI popup menu.
    #[arg(short = 'm', long)]
    menu: bool,

    /// Print top network consuming processes to stdout.
    #[arg(short = 'T', long)]
    top: bool,

    /// Run speedtest benchmark and print summary to stdout.
    #[arg(short = 'B', long)]
    speedtest: bool,

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

    if cli.speedtest {
        return speedtest::run_cli_benchmark();
    }

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

    if cli.menu || cli.print_action {
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
    } else {
        // Default (zero arguments / headless): emit single Waybar JSON payload
        println!("{}", net::sample_json());
        Ok(())
    }
}
