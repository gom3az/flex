//! `flex-net` binary: network throughput monitor for Waybar.
//!
//! Ports `network-speed.sh` for Waybar's `custom/net-speed` module.
//! Emits JSON: `{"text":" <down>   <up>","class":"up"|"idle"}`.

use std::time::Duration;

use clap::Parser;
use flex_rice::exec::net;
use flex_rice::runner;

/// Network throughput monitor for Waybar.
#[derive(Debug, Parser)]
#[command(
    name = "flex-net",
    version,
    about = "Network throughput monitor for Waybar"
)]
struct Cli {
    /// Run continuously, streaming JSON lines at the specified interval in seconds.
    #[arg(short, long, value_name = "SECS")]
    stream: Option<u64>,
}

fn main() {
    let cli = Cli::parse();
    if let Some(secs) = cli.stream {
        let interval = Duration::from_secs(secs.max(1));
        if let Err(err) = net::stream(interval) {
            runner::fail(&err);
        }
    } else {
        println!("{}", net::sample_json());
    }
}
