//! `flex-mixer` binary: floating `PipeWire` TUI mixer toggle.
//!
//! Ports `audio-mixer-toggle.sh` for Hyprland (`SUPER+A`) and Waybar
//! (`pulseaudio` module on-click). Toggles `wiremix` in a floating window.
//!
//! Sync-only helper (OPT-11): plain `fn main`, no tokio runtime —
//! `mixer::toggle` never awaits, so spawning a thread-pool would only raise
//! the link floor (same rationale as `flex-record`).

use clap::Parser;
use flex_rice::exec::mixer;
use flex_rice::runner;

/// Toggle wiremix (`PipeWire` TUI mixer) in a floating window.
#[derive(Debug, Parser)]
#[command(
    name = "flex-mixer",
    version,
    about = "Toggle floating PipeWire TUI mixer (wiremix)"
)]
struct Cli {}

fn main() {
    runner::init_logging();
    let _cli = Cli::parse();
    if let Err(err) = mixer::toggle(None) {
        runner::fail(&err);
    }
}
