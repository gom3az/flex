//! `flex-notify` binary entrypoint.
//!
//! Dual mode operation:
//! - With `-m` / `--menu` (or `flex notify`): launches the interactive
//!   right-side drawer TUI inside the terminal popup.
//! - Default / without `-m`: outputs JSON status for Waybar polling with
//!   unread badge counts, active DND state, and tooltip previews.

#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(clippy::pedantic)]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use flex_core::backend::EXIT_CANCELLED;
use flex_core::Outcome;
use flex_rice::exec::notify::{self, DndState, Urgency};
use flex_rice::providers::notify as provider;
use flex_rice::runner::{self, GlobalStyle, Provider};

#[derive(Parser, Debug)]
#[allow(clippy::struct_excessive_bools)]
#[command(
    name = "flex-notify",
    version,
    about = "Native Right-Side Notification Center Drawer and Waybar Status Module"
)]
struct Cli {
    /// Global presentation flags.
    #[command(flatten)]
    style: GlobalStyle,

    /// Open the interactive Notification Center Drawer TUI
    #[arg(short = 'm', long = "menu")]
    menu: bool,

    /// Clear all active non-critical notifications
    #[arg(long = "clear-all")]
    clear_all: bool,

    /// Toggle Do Not Disturb mode
    #[arg(long = "toggle-dnd")]
    toggle_dnd: bool,

    /// Print the selected `ACTION:` line without executing it.
    #[arg(long)]
    print_action: bool,

    /// Output JSON status for Waybar polling (default when not in popup)
    #[arg(long = "status")]
    status: bool,

    /// State file override (for testing)
    #[arg(long = "state-file", hide = true)]
    state_file: Option<PathBuf>,

    /// Direct notification operations
    #[command(subcommand)]
    op: Option<NotifyOp>,
}

#[derive(Debug, Subcommand)]
enum NotifyOp {
    /// Post a new notification into the notification feed
    Send {
        /// Notification summary / title
        summary: String,

        /// Notification body
        #[arg(default_value = "")]
        body: String,

        /// Application name
        #[arg(short = 'a', long = "app", default_value = "System")]
        app: String,

        /// Urgency level (low, normal, critical)
        #[arg(short = 'u', long = "urgency", default_value = "normal")]
        urgency: String,

        /// In-flight progress fraction (0.0 - 1.0)
        #[arg(short = 'p', long = "progress")]
        progress: Option<f32>,
    },
    /// Clear all non-critical notifications
    ClearAll,
    /// Toggle Do Not Disturb mode
    ToggleDnd,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn main() {
    if let Err(err) = run() {
        runner::fail(&err);
    }
}

fn run_waybar_status(state_path: Option<&Path>) {
    let mut state = notify::load_state(state_path);
    notify::probe_quick_controls(&mut state.controls);
    let now = now_secs();

    let active: Vec<_> = state
        .notifications
        .iter()
        .filter(|n| {
            !n.is_dismissed && (!n.is_snoozed || n.snooze_until.is_some_and(|until| now >= until))
        })
        .collect();

    let unread_count = active.len();
    let is_dnd = state.controls.dnd.is_active(now);
    let has_critical = active.iter().any(|n| n.urgency == Urgency::Critical);

    let (text, class) = if is_dnd {
        if let Some(rem) = state.controls.dnd.remaining_secs(now) {
            (format!(" 󰂛 {}m", rem / 60), "dnd")
        } else {
            (String::from(" 󰂛 DND"), "dnd")
        }
    } else if has_critical {
        (format!(" 󰀦 {unread_count}"), "critical")
    } else if unread_count > 0 {
        (format!(" 󰂚 {unread_count}"), "has-unread")
    } else {
        (String::from(" 󰂚 0"), "empty")
    };

    let tooltip = if unread_count == 0 {
        if let Some(mpris) = &state.controls.mpris {
            let status_icon = if mpris.is_playing {
                "Playing"
            } else {
                "Paused"
            };
            format!(
                "No unread notifications\n\n󰝚 {}: {} — {}\nStatus: {status_icon} ({})\nClick to open Notification Center",
                mpris.player,
                mpris.title,
                mpris.artist,
                notify::format_duration(mpris.position_secs)
            )
        } else if is_dnd {
            String::from("Do Not Disturb Active\nNo new notifications")
        } else {
            String::from("No unread notifications\nClick to open Notification Center")
        }
    } else {
        let mut lines = vec![format!("{unread_count} unread notification(s):")];
        for item in active.iter().take(5) {
            let rel = notify::format_relative_time(item.timestamp, now);
            lines.push(format!("• {} ({rel}): {}", item.app_name, item.summary));
        }
        if unread_count > 5 {
            lines.push(format!("... and {} more", unread_count - 5));
        }
        if let Some(mpris) = &state.controls.mpris {
            lines.push(format!(
                "\n󰝚 Now Playing: {} — {} ({})",
                mpris.title, mpris.artist, mpris.player
            ));
        }
        lines.join("\n")
    };

    let json = serde_json::json!({
        "text": text,
        "tooltip": tooltip,
        "class": class,
        "count": unread_count,
    });

    println!("{json}");
}

fn handle_op(op: NotifyOp, state_path: Option<&Path>) -> anyhow::Result<()> {
    match op {
        NotifyOp::Send {
            summary,
            body,
            app,
            urgency,
            progress,
        } => {
            let urg = Urgency::parse(&urgency);
            let mut item = notify::NotificationItem::new(0, app, summary, body, urg);
            item.progress = progress;
            let id = notify::post_notification(item, state_path)?;
            println!("Notification {id} posted");
            Ok(())
        }
        NotifyOp::ClearAll => {
            let mut state = notify::load_state(state_path);
            for n in &mut state.notifications {
                if !n.is_pinned && n.urgency != Urgency::Critical {
                    n.is_dismissed = true;
                }
            }
            notify::save_state(&state, state_path)?;
            println!("Cleared active notifications");
            Ok(())
        }
        NotifyOp::ToggleDnd => {
            let mut state = notify::load_state(state_path);
            let now = now_secs();
            state.controls.dnd = if state.controls.dnd.is_active(now) {
                DndState::Off
            } else {
                DndState::Indefinite
            };
            notify::save_state(&state, state_path)?;
            println!("DND toggled");
            Ok(())
        }
    }
}

#[allow(clippy::too_many_lines)]
fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let state_path = cli.state_file.as_deref();
    let style = cli.style.options();

    if let Some(op) = cli.op {
        return handle_op(op, state_path);
    }

    if cli.clear_all {
        let mut state = notify::load_state(state_path);
        for n in &mut state.notifications {
            if !n.is_pinned && n.urgency != Urgency::Critical {
                n.is_dismissed = true;
            }
        }
        notify::save_state(&state, state_path)?;
        println!("Cleared active notifications");
        return Ok(());
    }

    if cli.toggle_dnd {
        let mut state = notify::load_state(state_path);
        let now = now_secs();
        state.controls.dnd = if state.controls.dnd.is_active(now) {
            DndState::Off
        } else {
            DndState::Indefinite
        };
        notify::save_state(&state, state_path)?;
        println!("DND toggled");
        return Ok(());
    }

    if cli.menu {
        // Interactive popup drawer mode
        runner::popup_guard(Provider::Notify)?;

        if cli.print_action {
            return runner::run_select(Provider::Notify, style);
        }

        let menu = runner::build_menu(Provider::Notify, style)?;
        match flex_core::run::run_capture(menu)? {
            Outcome::Chosen {
                action_id, label, ..
            }
            | Outcome::Toggle {
                action_id, label, ..
            } => {
                provider::execute(&action_id, &label, state_path)?;
                Ok(())
            }
            Outcome::Target { target, title, .. } => {
                provider::execute(&target, &title, state_path)?;
                Ok(())
            }
            Outcome::Delete { action_id, .. } => {
                let action = if action_id.starts_with("notif:") {
                    format!("dismiss:{}", action_id.trim_start_matches("notif:"))
                } else {
                    format!("dismiss:{action_id}")
                };
                provider::execute(&action, "Dismiss", state_path)?;
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
        run_waybar_status(state_path);
        Ok(())
    }
}
