//! `flex-record` binary: the recording helper (`start`/`status`/`stop`).
//!
//! Ports `recording-start.sh`, `recording-status.sh`, and
//! `recording-stop.sh` so `flex-shot` and Waybar no longer shell out to bash.
//! `flex-record [-a] [-g GEOM] FILE` is the drop-in `RECORDING_START` shape
//! (start), and the `start`/`status`/`stop` words select the verb explicitly.
//!
//! Manual argv dispatch (not clap): the bare-args form and the subcommands
//! share one argv space, which clap cannot express unambiguously.

use flex_rice::exec::record;
use flex_rice::runner;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    runner::init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(err) = dispatch(&args) {
        runner::fail(&err);
    }
}

/// Route `args` to the matching recording action.
///
/// # Errors
///
/// When the selected action fails. Messages carry no `flex:` prefix; `main`
/// adds it via the runner.
fn dispatch(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => {
            print!(
                "Recording helper\n\n\
                 Usage:\n  \
                 flex-record [-a] [-g GEOM] FILE   start a recording\n  \
                 flex-record start …               same as above\n  \
                 flex-record status                print the REC badge (exit 1 when idle)\n  \
                 flex-record stop                  stop the active recording\n"
            );
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("flex-record {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("status") => record::status(None),
        Some("stop") => record::stop(None),
        Some("start") => record::start(args.get(1..).unwrap_or_default(), None),
        _ => record::start(args, None),
    }
}
