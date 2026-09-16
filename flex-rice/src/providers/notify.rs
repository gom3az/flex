//! Notification Center Drawer provider (`flex-notify`).
//!
//! Renders a native, search-free right-side drawer dashboard containing:
//! - Quick Controls shelf (DND toggle & presets, Clear All, Night Light, Caffeine, Mic Mute)
//! - MPRIS media player card with progress bar
//! - Sticky critical system alerts
//! - In-flight download/upgrade progress trackers
//! - Expandable app and channel thread stacks
//! - Smart entity extractors (1-click copy OTP, URLs, hex colors)
//! - Per-app mute rules and Focus/DND timers

use std::time::{SystemTime, UNIX_EPOCH};

use flex_core::{Menu, Row, RowId, Tab, Target};

use crate::exec::notify::{
    self, DndState, ExtractedEntity, NotificationItem, NotifyState, Urgency,
};

/// The provider name matching `flex notify` and `flex-notify`.
pub const PROVIDER: &str = "notify";

/// Tab titles
pub const TAB_FEED: &str = "Feed";
pub const TAB_CHANNELS: &str = "Channels";
pub const TAB_FOCUS: &str = "Focus/DND";
pub const TAB_HISTORY: &str = "History";

/// Action IDs for quick controls
pub const ACTION_QUICK_CONTROLS: &str = "quick:controls";
pub const ACTION_MPRIS_TRACK: &str = "mpris:track";
pub const ACTION_CLEAR_ALL: &str = "action:clear_all";
pub const ACTION_TOGGLE_DND: &str = "action:toggle_dnd";
pub const ACTION_TOGGLE_NIGHT: &str = "action:toggle_night";
pub const ACTION_TOGGLE_CAFFEINE: &str = "action:toggle_caffeine";
pub const ACTION_TOGGLE_MIC: &str = "action:toggle_mic";

/// Current unix timestamp in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Construct the `[Feed]` tab.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn feed_tab_from(state: &NotifyState, now: u64) -> Tab {
    let mut rows = Vec::new();

    // 1. Top Quick Controls Shelf
    let dnd_label = if state.controls.dnd.is_active(now) {
        if let Some(rem) = state.controls.dnd.remaining_secs(now) {
            format!("DND: On ({}m)", rem / 60)
        } else {
            String::from("DND: On")
        }
    } else {
        String::from("DND: Off")
    };

    let active_count = state
        .notifications
        .iter()
        .filter(|n| !n.is_dismissed && !n.is_snoozed)
        .count();

    let mut quick_targets = vec![
        Target::new(RowId::new("toggle_dnd"), format!("Toggle {dnd_label}")),
        Target::new(
            RowId::new("clear_all"),
            format!("Clear All ({active_count} Active)"),
        ),
        Target::new(
            RowId::new("toggle_night"),
            format!(
                "Night Light: {}",
                if state.controls.night_light {
                    "On"
                } else {
                    "Off"
                }
            ),
        ),
        Target::new(
            RowId::new("toggle_caffeine"),
            format!(
                "Caffeine: {}",
                if state.controls.caffeine { "On" } else { "Off" }
            ),
        ),
        Target::new(
            RowId::new("toggle_mic"),
            format!(
                "Microphone: {}",
                if state.controls.mic_muted {
                    "Muted"
                } else {
                    "Active"
                }
            ),
        ),
    ];
    if let Some(target) = quick_targets.first_mut() {
        target.is_default = true;
    }

    let quick_meta = format!("[󰂛 {dnd_label}]  [󰅖 Clear ({active_count})]  [󰖔 Night]  [󰤄 Caffe]");
    let mut quick_row = Row::with_targets(
        RowId::new(ACTION_QUICK_CONTROLS),
        "Quick Controls & System Shelf",
        quick_targets,
        0,
    );
    quick_row.meta = Some(quick_meta);
    rows.push(quick_row);

    // 2. MPRIS Media Player Card (if present)
    if let Some(mpris) = &state.controls.mpris {
        let title_label = if mpris.artist.is_empty() || mpris.artist == "Unknown Artist" {
            format!("󰝚 {}", mpris.title)
        } else {
            format!("󰝚 {} — {}", mpris.title, mpris.artist)
        };

        let time_meta = if mpris.is_live || mpris.length_secs == 0 {
            format!(
                "{} · Live · {}",
                notify::format_duration(mpris.position_secs),
                mpris.player
            )
        } else {
            format!(
                "{} / {} · {}",
                notify::format_duration(mpris.position_secs),
                notify::format_duration(mpris.length_secs),
                mpris.player
            )
        };

        let volume_frac = if !mpris.is_live && mpris.length_secs > 0 {
            #[allow(clippy::cast_precision_loss)]
            Some(((mpris.position_secs as f32) / (mpris.length_secs as f32)).clamp(0.0, 1.0))
        } else {
            None
        };

        let mut mpris_targets = vec![
            Target::new(
                RowId::new("play_pause"),
                if mpris.is_playing {
                    "Pause Playback"
                } else {
                    "Resume Playback"
                },
            ),
            Target::new(RowId::new("next_track"), "Next Track"),
            Target::new(RowId::new("prev_track"), "Previous Track"),
            Target::new(RowId::new("seek_forward_10"), "Seek Forward +10s"),
            Target::new(RowId::new("seek_backward_10"), "Seek Backward -10s"),
            Target::new(
                RowId::new(format!("focus_player:{}", mpris.player)),
                format!("Focus {}", mpris.player),
            ),
            Target::new(
                RowId::new(format!("copy_media:{} - {}", mpris.title, mpris.artist)),
                "Copy Media Info",
            ),
        ];
        if let Some(target) = mpris_targets.first_mut() {
            target.is_default = true;
        }

        let mut mpris_row = Row::with_targets(
            RowId::new(ACTION_MPRIS_TRACK),
            title_label,
            mpris_targets,
            0,
        );
        mpris_row.meta = Some(time_meta);
        mpris_row.volume = volume_frac;
        rows.push(mpris_row);
    }

    // 3. Notification Feed Rows
    let active_notifs: Vec<&NotificationItem> = state
        .notifications
        .iter()
        .filter(|n| {
            !n.is_dismissed && (!n.is_snoozed || n.snooze_until.is_some_and(|until| now >= until))
        })
        .collect();

    for item in &active_notifs {
        let rel_time = notify::format_relative_time(item.timestamp, now);
        let entities = notify::extract_entities(&format!("{} {}", item.summary, item.body));

        let mut targets = Vec::new();

        // Add extracted entity targets first
        for entity in &entities {
            match entity {
                ExtractedEntity::OtpCode(code) => {
                    targets.push(Target::new(
                        RowId::new(format!("copy_otp:{code}")),
                        format!("Copy OTP Code ({code})"),
                    ));
                }
                ExtractedEntity::Url(url) => {
                    targets.push(Target::new(
                        RowId::new(format!("open_url:{url}")),
                        format!("Open Link ({url})"),
                    ));
                    targets.push(Target::new(
                        RowId::new(format!("copy_url:{url}")),
                        format!("Copy Link ({url})"),
                    ));
                }
                ExtractedEntity::HexColor(hex) => {
                    targets.push(Target::new(
                        RowId::new(format!("copy_hex:{hex}")),
                        format!("Copy Color ({hex})"),
                    ));
                }
            }
        }

        targets.push(Target::new(
            RowId::new(format!("dismiss:{}", item.id)),
            "Dismiss Notification",
        ));
        targets.push(Target::new(
            RowId::new(format!("dismiss_app:{}", item.app_name)),
            format!("Dismiss All from {}", item.app_name),
        ));
        targets.push(Target::new(
            RowId::new(format!("snooze_15:{}", item.id)),
            "Snooze 15 Minutes",
        ));
        targets.push(Target::new(
            RowId::new(format!("snooze_60:{}", item.id)),
            "Snooze 1 Hour",
        ));
        targets.push(Target::new(
            RowId::new(format!("mute_app:{}", item.app_name)),
            format!("Silence {} for 1 Hour", item.app_name),
        ));

        if let Some(target) = targets.first_mut() {
            target.is_default = true;
        }

        let label = format!("{} · {}", item.app_name, item.summary);
        let meta = if item.urgency == Urgency::Critical {
            format!("{rel_time} · Critical")
        } else {
            rel_time
        };

        let mut row =
            Row::with_targets(RowId::new(format!("notif:{}", item.id)), label, targets, 0);
        row.meta = Some(meta);
        row.is_default = item.is_pinned || item.urgency == Urgency::Critical;

        if let Some(prog) = item.progress {
            row.volume = Some(prog.clamp(0.0, 1.0));
        }

        rows.push(row);
    }

    if rows.is_empty() {
        rows.push(Row::new(
            RowId::new("empty:notif"),
            "No Active Notifications",
        ));
    }

    let mut tab = Tab::with_rows(TAB_FEED, rows);
    tab.bare_rows = false;
    tab.filterable = false; // Search does not belong in notification center
    tab
}

/// Construct the `[Channels]` tab.
#[must_use]
pub fn channels_tab_from(state: &NotifyState, now: u64) -> Tab {
    let mut rows = Vec::new();

    let mut apps: Vec<&str> = state
        .notifications
        .iter()
        .map(|n| n.app_name.as_str())
        .collect();
    apps.sort_unstable();
    apps.dedup();

    for app in apps {
        let count = state
            .notifications
            .iter()
            .filter(|n| n.app_name == app && !n.is_dismissed)
            .count();
        let latest = state
            .notifications
            .iter()
            .filter(|n| n.app_name == app && !n.is_dismissed)
            .max_by_key(|n| n.timestamp);

        let meta = latest.map_or_else(
            || String::from("idle"),
            |l| notify::format_relative_time(l.timestamp, now),
        );

        let targets = vec![
            Target::new(
                RowId::new(format!("dismiss_app:{app}")),
                format!("Dismiss All ({count})"),
            ),
            Target::new(
                RowId::new(format!("mute_app:{app}")),
                format!("Mute {app} for 1h"),
            ),
            Target::new(
                RowId::new(format!("priority_app:{app}")),
                format!("Toggle Priority for {app}"),
            ),
        ];

        let mut row = Row::with_targets(
            RowId::new(format!("channel:{app}")),
            format!("{app} ({count} active)"),
            targets,
            0,
        );
        row.meta = Some(meta);
        rows.push(row);
    }

    if rows.is_empty() {
        rows.push(Row::new(RowId::new("empty:channels"), "No App Channels"));
    }

    let mut tab = Tab::with_rows(TAB_CHANNELS, rows);
    tab.bare_rows = false;
    tab.filterable = false;
    tab
}

/// Construct the `[Focus/DND]` tab.
#[must_use]
pub fn focus_tab_from(state: &NotifyState, now: u64) -> Tab {
    let mut rows = Vec::new();

    let dnd_status_label = match state.controls.dnd {
        DndState::Off => "Do Not Disturb: Inactive",
        DndState::Indefinite => "Do Not Disturb: Active (Indefinite)",
        DndState::Timed { until, .. } => {
            let rem = until.saturating_sub(now);
            if rem > 0 {
                "Do Not Disturb: Active (Timer Running)"
            } else {
                "Do Not Disturb: Timer Expired"
            }
        }
    };

    let targets = vec![
        Target::new(RowId::new("dnd:off"), "Turn DND Off"),
        Target::new(RowId::new("dnd:25m"), "Pomodoro Sprint (25 Mins)"),
        Target::new(RowId::new("dnd:60m"), "Deep Work (1 Hour)"),
        Target::new(RowId::new("dnd:120m"), "Focus Session (2 Hours)"),
        Target::new(RowId::new("dnd:indefinite"), "Silence Until Turned Off"),
    ];

    let mut status_row =
        Row::with_targets(RowId::new("focus:status"), dnd_status_label, targets, 0);
    if let Some(frac) = state.controls.dnd.fraction_remaining(now) {
        status_row.volume = Some(frac);
    }
    rows.push(status_row);

    // Presets
    let mut p1 = Row::with_meta(
        RowId::new("preset:pomodoro"),
        "Pomodoro Sprint (25m)",
        "25 Mins",
    );
    p1.volume = Some(1.0);
    rows.push(p1);

    let mut p2 = Row::with_meta(
        RowId::new("preset:deepwork"),
        "Deep Work Session (1h)",
        "60 Mins",
    );
    p2.volume = Some(1.0);
    rows.push(p2);

    let p3 = Row::with_meta(
        RowId::new("preset:gaming"),
        "Gaming Mode (Mute All Except Critical)",
        "Auto",
    );
    rows.push(p3);

    let mut tab = Tab::with_rows(TAB_FOCUS, rows);
    tab.bare_rows = false;
    tab.filterable = false;
    tab
}

/// Construct the `[History]` tab.
#[must_use]
pub fn history_tab_from(state: &NotifyState, now: u64) -> Tab {
    let mut rows = Vec::new();

    let dismissed_items: Vec<&NotificationItem> = state
        .notifications
        .iter()
        .filter(|n| n.is_dismissed)
        .collect();

    for item in dismissed_items.iter().rev().take(30) {
        let rel_time = notify::format_relative_time(item.timestamp, now);
        let targets = vec![
            Target::new(
                RowId::new(format!("restore:{}", item.id)),
                "Restore Notification",
            ),
            Target::new(
                RowId::new(format!("copy_body:{}", item.id)),
                "Copy Message Text",
            ),
        ];

        let mut row = Row::with_targets(
            RowId::new(format!("hist:{}", item.id)),
            format!("{} · {}", item.app_name, item.summary),
            targets,
            0,
        );
        row.meta = Some(format!("{rel_time} · Dismissed"));
        row.offline = true;
        rows.push(row);
    }

    if rows.is_empty() {
        rows.push(Row::new(RowId::new("empty:history"), "History is Empty"));
    }

    let mut tab = Tab::with_rows(TAB_HISTORY, rows);
    tab.bare_rows = false;
    tab.filterable = false;
    tab
}

/// Build the full 4-tab Notification Center menu.
#[must_use]
pub fn menu_from(state: &NotifyState, now: u64) -> Menu {
    let tabs = vec![
        feed_tab_from(state, now),
        channels_tab_from(state, now),
        focus_tab_from(state, now),
        history_tab_from(state, now),
    ];
    crate::menu(PROVIDER, tabs)
}

/// Construct menu using live disk state and live hardware/MPRIS probes.
#[must_use]
pub fn menu() -> Menu {
    let mut state = notify::load_state(None);
    notify::probe_quick_controls(&mut state.controls);
    menu_from(&state, now_secs())
}

/// In-place tick hook for background updates.
pub fn refresh(menu: &mut Menu) {
    let mut state = notify::load_state(None);
    notify::probe_quick_controls(&mut state.controls);
    let now = now_secs();

    let new_menu = menu_from(&state, now);
    for (i, new_tab) in new_menu.app.tabs.into_iter().enumerate() {
        if let Some(tab) = menu.app.tabs.get_mut(i) {
            let old_focus = tab.state.focus;
            tab.rows = new_tab.rows;
            tab.state.focus = old_focus.min(tab.rows.len().saturating_sub(1));
        }
    }
}

/// Report on execution outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteReport {
    pub action_id: String,
    pub detail: Option<String>,
}

/// Execute a selected action or target.
///
/// # Errors
/// Returns error if state file cannot be updated.
pub fn execute(
    action_id: &str,
    target_title: &str,
    state_path: Option<&std::path::Path>,
) -> anyhow::Result<ExecuteReport> {
    let mut state = notify::load_state(state_path);
    let now = now_secs();

    if action_id == ACTION_CLEAR_ALL || target_title.contains("Clear All") {
        for n in &mut state.notifications {
            if !n.is_pinned && n.urgency != Urgency::Critical {
                n.is_dismissed = true;
            }
        }
    } else if action_id == "play_pause"
        || action_id == ACTION_MPRIS_TRACK
        || target_title.contains("Playback")
    {
        let _ = std::process::Command::new("playerctl")
            .arg("play-pause")
            .status();
    } else if action_id == "next_track" || target_title.contains("Next Track") {
        let _ = std::process::Command::new("playerctl").arg("next").status();
    } else if action_id == "prev_track" || target_title.contains("Previous Track") {
        let _ = std::process::Command::new("playerctl")
            .arg("previous")
            .status();
    } else if action_id == "seek_forward_10" || target_title.contains("Forward +10s") {
        let _ = std::process::Command::new("playerctl")
            .args(["position", "10+"])
            .status();
    } else if action_id == "seek_backward_10" || target_title.contains("Backward -10s") {
        let _ = std::process::Command::new("playerctl")
            .args(["position", "10-"])
            .status();
    } else if let Some(player) = action_id.strip_prefix("focus_player:") {
        let class_name = if player.starts_with("brave") {
            "brave-browser"
        } else {
            player
        };
        let _ = std::process::Command::new("hyprctl")
            .args([
                "dispatch",
                "focuswindow",
                &format!("class:^({class_name})$"),
            ])
            .status();
    } else if let Some(info) = action_id.strip_prefix("copy_media:") {
        let _ = std::process::Command::new("wl-copy").arg(info).status();
    } else if action_id == "toggle_mic" || action_id == ACTION_TOGGLE_MIC {
        let _ = std::process::Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"])
            .status();
    } else if action_id == "toggle_night" || action_id == ACTION_TOGGLE_NIGHT {
        let _ = std::process::Command::new("pkill")
            .args(["-SIGUSR1", "hyprsunset"])
            .status();
    } else if let Some(id_str) = action_id.strip_prefix("dismiss:") {
        if let Ok(id) = id_str.parse::<u32>() {
            if let Some(n) = state.notifications.iter_mut().find(|n| n.id == id) {
                n.is_dismissed = true;
            }
        }
    } else if let Some(app) = action_id.strip_prefix("dismiss_app:") {
        for n in &mut state.notifications {
            if n.app_name == app {
                n.is_dismissed = true;
            }
        }
    } else if let Some(code) = action_id.strip_prefix("copy_otp:") {
        let _ = std::process::Command::new("wl-copy").arg(code).status();
    } else if let Some(url) = action_id.strip_prefix("open_url:") {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    } else if let Some(url) = action_id.strip_prefix("copy_url:") {
        let _ = std::process::Command::new("wl-copy").arg(url).status();
    } else if action_id == "dnd:25m" || target_title.contains("Pomodoro") {
        state.controls.dnd = DndState::Timed {
            until: now + 25 * 60,
            total_secs: 25 * 60,
        };
    } else if action_id == "dnd:60m" || target_title.contains("Deep Work") {
        state.controls.dnd = DndState::Timed {
            until: now + 60 * 60,
            total_secs: 60 * 60,
        };
    } else if action_id == "dnd:off" || target_title.contains("DND Off") {
        state.controls.dnd = DndState::Off;
    } else if action_id == "dnd:indefinite" {
        state.controls.dnd = DndState::Indefinite;
    } else if action_id == "toggle_dnd" || action_id == ACTION_TOGGLE_DND {
        state.controls.dnd = if state.controls.dnd.is_active(now) {
            DndState::Off
        } else {
            DndState::Indefinite
        };
    }

    notify::save_state(&state, state_path)?;

    Ok(ExecuteReport {
        action_id: action_id.to_string(),
        detail: Some(target_title.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_tab_builds_expected_wiremix_rows_and_targets() {
        let mut state = NotifyState::default();
        state.notifications.push(NotificationItem::new(
            1,
            "Discord",
            "#dev-team",
            "Hey please check PR #142 at https://github.com/gom3az/flex with code 849201",
            Urgency::Normal,
        ));
        state.notifications.push(NotificationItem::new(
            2,
            "System",
            "Low Battery Warning",
            "Plug in AC adapter soon",
            Urgency::Critical,
        ));

        let tab = feed_tab_from(&state, 1000);
        assert_eq!(tab.name, TAB_FEED);
        assert!(!tab.bare_rows);
        assert!(!tab.filterable, "No search prompt in notification drawer");

        // Row 0: Quick Controls Shelf
        assert_eq!(tab.rows[0].id.as_str(), ACTION_QUICK_CONTROLS);
        assert_eq!(tab.rows[0].label, "Quick Controls & System Shelf");

        // Row 1: Discord notification with OTP & URL targets
        let r1 = &tab.rows[1];
        assert_eq!(r1.label, "Discord · #dev-team");
        assert!(r1.targets.iter().any(|t| t.title.contains("849201")));
        assert!(r1
            .targets
            .iter()
            .any(|t| t.title.contains("https://github.com")));

        // Row 2: Critical Low Battery alert (pinned)
        let r2 = &tab.rows[2];
        assert_eq!(r2.label, "System · Low Battery Warning");
        assert!(r2.is_default); // Pinned marker ◇
    }

    #[test]
    fn execution_clears_non_critical_notifications() {
        let dir = std::env::temp_dir().join(format!("flex-notify-exec-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state.json");

        let mut state = NotifyState::default();
        state.notifications.push(NotificationItem::new(
            1,
            "Discord",
            "Chat",
            "Text",
            Urgency::Normal,
        ));
        state.notifications.push(NotificationItem::new(
            2,
            "System",
            "Battery",
            "Text",
            Urgency::Critical,
        ));
        notify::save_state(&state, Some(&path)).unwrap();

        execute(ACTION_CLEAR_ALL, "Clear All", Some(&path)).unwrap();

        let updated = notify::load_state(Some(&path));
        assert!(updated.notifications[0].is_dismissed);
        assert!(
            !updated.notifications[1].is_dismissed,
            "Critical notifications protected from Clear All"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
