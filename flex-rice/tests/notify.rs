//! Integration and golden tests for `flex-notify` provider and binary.

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use flex_core::Menu;
use flex_rice::exec::notify::{
    self, DndState, ExtractedEntity, MprisTrack, NotificationItem, NotifyState, Urgency,
};
use flex_rice::providers::notify as provider;

fn draw(menu: &mut Menu, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal creates");
    terminal
        .draw(|f| flex_core::render::render(f, menu))
        .expect("renders cleanly");
    terminal.backend().buffer().clone()
}

fn buffer_line(buf: &Buffer, y: u16, width: u16) -> String {
    let mut s = String::new();
    for x in 0..width {
        s.push_str(buf.cell((x, y)).map_or(" ", |c| c.symbol()));
    }
    s
}

fn mock_state() -> NotifyState {
    let mut state = NotifyState::default();

    // Controls
    state.controls.dnd = DndState::Timed {
        until: 2500,
        total_secs: 1500,
    };
    state.controls.night_light = true;
    state.controls.caffeine = false;
    state.controls.mic_muted = false;
    state.controls.mpris = Some(MprisTrack {
        player: "spotify".to_string(),
        title: "Get Lucky".to_string(),
        artist: "Daft Punk ft. Pharrell Williams".to_string(),
        album: Some("Random Access Memories".to_string()),
        position_secs: 134,
        length_secs: 248,
        is_playing: true,
        is_live: false,
        art_url: None,
    });

    // 1. Critical low battery alert
    let mut n1 = NotificationItem::new(
        1,
        "System",
        "Low Battery Warning (8%)",
        "Connect AC charger immediately",
        Urgency::Critical,
    );
    n1.timestamp = 990;
    state.notifications.push(n1);

    // 2. In-flight download notification
    let mut n2 = NotificationItem::new(
        2,
        "PackageKit",
        "System Upgrade (mesa, linux-firmware)",
        "Downloading packages: 14.2 MB/s",
        Urgency::Normal,
    );
    n2.timestamp = 980;
    n2.progress = Some(0.64);
    state.notifications.push(n2);

    // 3. Discord message with URL and OTP
    let mut n3 = NotificationItem::new(
        3,
        "Discord",
        "#dev-team",
        "Alex: Check PR #142 at https://github.com/gom3az/flex and use 2FA code 849201",
        Urgency::Normal,
    );
    n3.timestamp = 880;
    state.notifications.push(n3);

    // 4. Dismissed item in history
    let mut n4 =
        NotificationItem::new(4, "Slack", "#general", "Standup in 5 minutes", Urgency::Low);
    n4.timestamp = 500;
    n4.is_dismissed = true;
    state.notifications.push(n4);

    state
}

#[test]
fn entity_extractor_robustness() {
    let text = "Your verification code is 492018. Visit https://auth.example.com/verify or check color #ff5555.";
    let entities = notify::extract_entities(text);

    assert!(entities
        .iter()
        .any(|e| matches!(e, ExtractedEntity::OtpCode(c) if c == "492018")));
    assert!(entities
        .iter()
        .any(|e| matches!(e, ExtractedEntity::Url(u) if u == "https://auth.example.com/verify")));
    assert!(entities
        .iter()
        .any(|e| matches!(e, ExtractedEntity::HexColor(h) if h == "#ff5555")));
}

#[test]
fn feed_tab_contract_and_wiremix_rows() {
    let state = mock_state();
    let tab = provider::feed_tab_from(&state, 1000);

    assert_eq!(tab.name, provider::TAB_FEED);
    assert!(!tab.bare_rows);
    assert!(
        !tab.filterable,
        "Notification Center must not have a search bar"
    );
    assert_eq!(tab.rows.len(), 5);

    // Row 0: Quick Controls Shelf
    assert_eq!(tab.rows[0].id.as_str(), provider::ACTION_QUICK_CONTROLS);
    assert_eq!(tab.rows[0].label, "Quick Controls & System Shelf");

    // Row 1: MPRIS Track Card with volume bar
    assert_eq!(tab.rows[1].id.as_str(), provider::ACTION_MPRIS_TRACK);
    assert!(tab.rows[1].label.contains("Get Lucky"));
    assert!(tab.rows[1].volume.is_some());

    // Row 2: Critical alert pinned with marker
    let r2 = &tab.rows[2];
    assert_eq!(r2.id.as_str(), "notif:1");
    assert!(
        r2.is_default,
        "Critical alerts must have the default marker ◇"
    );
    assert_eq!(r2.label, "System · Low Battery Warning (8%)");

    // Row 3: In-flight progress notification with volume bar
    let r3 = &tab.rows[3];
    assert_eq!(r3.id.as_str(), "notif:2");
    assert_eq!(r3.volume, Some(0.64));

    // Row 4: Discord with extracted targets
    let r4 = &tab.rows[4];
    assert_eq!(r4.id.as_str(), "notif:3");
    assert!(r4.targets.iter().any(|t| t.title.contains("849201")));
    assert!(r4
        .targets
        .iter()
        .any(|t| t.title.contains("https://github.com")));
}

#[test]
fn channels_tab_groups_by_app() {
    let state = mock_state();
    let tab = provider::channels_tab_from(&state, 1000);

    assert_eq!(tab.name, provider::TAB_CHANNELS);
    assert!(!tab.filterable);
    // 4 channel headers (Discord, PackageKit, Slack, System) + 3 active child notification rows = 7 rows
    assert_eq!(tab.rows.len(), 7);
    assert!(tab
        .rows
        .iter()
        .any(|r| r.label.contains("Discord (1 active)")));
    assert!(tab
        .rows
        .iter()
        .any(|r| r.label.contains("PackageKit (1 active)")));
    assert!(tab
        .rows
        .iter()
        .any(|r| r.label.contains("System (1 active)")));
    assert!(tab.rows.iter().any(|r| r.label.contains("#dev-team")));
}

#[test]
fn feed_tab_groups_multi_notification_apps() {
    let mut state = mock_state();
    // Add a second Discord notification to trigger group header stack
    let mut n5 = NotificationItem::new(
        5,
        "Discord",
        "#general",
        "Lunch time everyone!",
        Urgency::Normal,
    );
    n5.timestamp = 900;
    state.notifications.push(n5);

    let tab = provider::feed_tab_from(&state, 1000);
    // Row 0: Quick Controls
    // Row 1: MPRIS
    // Row 2: Critical Low Battery
    // Row 3: Discord Group Header (2 notifications)
    // Row 4: Discord #general
    // Row 5: Discord #dev-team
    // Row 6: PackageKit
    assert_eq!(tab.rows.len(), 7);

    let group_row = tab
        .rows
        .iter()
        .find(|r| r.id.as_str() == "group:Discord")
        .expect("group header found");
    assert!(group_row.label.contains("Discord (2 notifications)"));
    assert!(group_row
        .targets
        .iter()
        .any(|t| t.title.contains("Dismiss All (2)")));
}

#[test]
fn notification_preview_carries_body_and_image() {
    let mut state = NotifyState::default();
    let mut notif = NotificationItem::new(
        10,
        "Flameshot",
        "Screenshot Captured",
        "Saved to /tmp/screenshot.png",
        Urgency::Normal,
    );
    notif.image_path = Some("/tmp/screenshot.png".to_string());
    state.notifications.push(notif);

    let tab = provider::feed_tab_from(&state, 1000);
    let notif_row = tab
        .rows
        .iter()
        .find(|r| r.id.as_str() == "notif:10")
        .expect("row found");

    // Text preview in config (detail line)
    assert_eq!(
        notif_row.config.as_deref(),
        Some("Saved to /tmp/screenshot.png")
    );
    // Image preview in preview_image
    assert_eq!(
        notif_row.preview_image.as_deref(),
        Some("/tmp/screenshot.png")
    );
}

#[test]
fn focus_tab_has_presets_and_status() {
    let state = mock_state();
    let tab = provider::focus_tab_from(&state, 1000);

    assert_eq!(tab.name, provider::TAB_FOCUS);
    assert!(!tab.filterable);
    assert_eq!(tab.rows.len(), 4);
    assert_eq!(tab.rows[0].label, "Do Not Disturb: Active (Timer Running)");
    assert!(tab.rows[0].volume.is_some());
    assert_eq!(tab.rows[1].label, "Pomodoro Sprint (25m)");
}

#[test]
fn history_tab_renders_dismissed_archive() {
    let state = mock_state();
    let tab = provider::history_tab_from(&state, 1000);

    assert_eq!(tab.name, provider::TAB_HISTORY);
    assert!(!tab.filterable);
    assert_eq!(tab.rows.len(), 1);
    assert_eq!(tab.rows[0].label, "Slack · #general");
    assert!(tab.rows[0].offline);
}

#[test]
fn golden_80x24_rendering_drawer_wiremix_detail() {
    let state = mock_state();
    let mut menu = provider::menu_from(&state, 1000);

    let buf = draw(&mut menu, 80, 24);

    // Tab bar on the bottom line (y = 23)
    let tab_bar = buffer_line(&buf, 23, 80);
    assert!(tab_bar.contains("[Feed]"));
    assert!(tab_bar.contains("Channels"));
    assert!(tab_bar.contains("Focus/DND"));
    assert!(tab_bar.contains("History"));

    // Entry 0 header (y = 1): Quick Controls Shelf
    let e0_header = buffer_line(&buf, 1, 80);
    assert!(e0_header.contains("░"));
    assert!(e0_header.contains("Quick Controls & System Shelf"));

    // Entry 1 header (y = 6): MPRIS Media Card
    let e1_header = buffer_line(&buf, 6, 80);
    assert!(e1_header.contains("Get Lucky"));

    // Entry 1 detail (y = 8): MPRIS volume/progress bar
    let e1_detail = buffer_line(&buf, 8, 80);
    assert!(e1_detail.contains("━"));

    // Entry 2 header (y = 11): Critical Alert with marker ◇
    let e2_header = buffer_line(&buf, 11, 80);
    assert!(e2_header.contains("◇"));
    assert!(e2_header.contains("Low Battery Warning"));
}

#[test]
fn executor_dismiss_and_dnd_dispatch() {
    let dir = std::env::temp_dir().join(format!("flex-notify-test-exec-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("state.json");

    let state = mock_state();
    notify::save_state(&state, Some(&path)).expect("saved");

    // Dismiss single item (notif 3: Discord)
    let rep = provider::execute("dismiss:3", "Dismiss", Some(&path)).expect("exec");
    assert_eq!(rep.action_id, "dismiss:3");
    let s1 = notify::load_state(Some(&path));
    assert!(
        s1.notifications
            .iter()
            .find(|n| n.id == 3)
            .unwrap()
            .is_dismissed
    );

    // Clear all (non-critical)
    let _ = provider::execute(provider::ACTION_CLEAR_ALL, "Clear All", Some(&path)).expect("exec");
    let s2 = notify::load_state(Some(&path));
    assert!(
        s2.notifications
            .iter()
            .find(|n| n.id == 2)
            .unwrap()
            .is_dismissed
    );
    // Critical alert protected
    assert!(
        !s2.notifications
            .iter()
            .find(|n| n.id == 1)
            .unwrap()
            .is_dismissed
    );

    // Set Pomodoro 25m DND
    let _ = provider::execute("dnd:25m", "Pomodoro 25m", Some(&path)).expect("exec");
    let s3 = notify::load_state(Some(&path));
    assert!(matches!(
        s3.controls.dnd,
        DndState::Timed {
            total_secs: 1500,
            ..
        }
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mpris_parser_handles_twitch_livestreams_and_youtube() {
    // 1. Twitch stream
    let twitch_line = "brave;;Playing;;Caliathlol;;CoC Comet Oracle - Grand Expeditions;;;;14798847;;9223372036854775807;;file:///tmp/.org.chromium.Chromium.Ir95Tl";
    let twitch = notify::parse_mpris_line(twitch_line).expect("parses twitch");
    assert_eq!(twitch.player, "brave");
    assert_eq!(twitch.artist, "Caliathlol");
    assert_eq!(twitch.title, "CoC Comet Oracle - Grand Expeditions");
    assert!(twitch.is_playing);
    assert!(twitch.is_live);
    assert_eq!(twitch.position_secs, 14);
    assert_eq!(twitch.length_secs, 0);
    assert_eq!(
        twitch.art_url.as_deref(),
        Some("/tmp/.org.chromium.Chromium.Ir95Tl")
    );

    // 2. Standard Spotify track
    let spotify_line = "spotify;;Paused;;Daft Punk;;Get Lucky;;RAM;;134000000;;248000000;;";
    let spotify = notify::parse_mpris_line(spotify_line).expect("parses spotify");
    assert_eq!(spotify.player, "spotify");
    assert_eq!(spotify.artist, "Daft Punk");
    assert!(!spotify.is_playing);
    assert!(!spotify.is_live);
    assert_eq!(spotify.position_secs, 134);
    assert_eq!(spotify.length_secs, 248);
}

#[test]
fn sound_playback_safety_with_dnd_and_urgency() {
    // Suppressed when DND is active
    notify::play_notification_sound(Urgency::Critical, true);
    notify::play_notification_sound(Urgency::Normal, true);
    notify::play_notification_sound(Urgency::Low, true);

    // Safe execution (non-blocking, tolerates missing sound players or headless test env)
    notify::play_notification_sound(Urgency::Normal, false);
    notify::play_notification_sound(Urgency::Critical, false);
}

#[test]
fn notify_daemon_dbus_server_contract() {
    let dir = std::env::temp_dir().join(format!("flex-notify-dbus-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("dbus-state.json");

    let server = notify::NotificationServer::new(Some(path.clone()));
    let caps = server.get_capabilities();
    assert!(caps.contains(&"actions".to_string()));
    assert!(caps.contains(&"body".to_string()));
    assert!(caps.contains(&"sound".to_string()));

    let (name, vendor, ver, spec) = server.get_server_information();
    assert_eq!(name, "flex-notify");
    assert_eq!(vendor, "flex");
    assert_eq!(ver, env!("CARGO_PKG_VERSION"));
    assert_eq!(spec, "1.2");

    let mut hints = std::collections::HashMap::new();
    hints.insert("urgency".to_string(), zbus::zvariant::Value::U8(1));
    let notif_id = server.notify(
        "Firefox".to_string(),
        0,
        "firefox".to_string(),
        "Download Finished".to_string(),
        "ISO downloaded".to_string(),
        vec!["open".to_string(), "Open".to_string()],
        hints,
        5000,
    );
    assert_eq!(notif_id, 1);

    let state = notify::load_state(Some(&path));
    assert_eq!(state.notifications.len(), 1);
    assert_eq!(state.notifications[0].app_name, "Firefox");
    assert_eq!(state.notifications[0].summary, "Download Finished");

    let _ = std::fs::remove_dir_all(&dir);
}
