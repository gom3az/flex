//! Notification center engine, persistence, entity extractors, and quick control probes.
//!
//! Provides safe, high-performance state management for `flex-notify`, including:
//! - In-flight progress updates and notification threading
//! - Regex-free / robust entity extraction (2FA/OTP codes, URLs, hex colors)
//! - Quick system toggles (DND presets, Night Light, Caffeine, Mic mute)
//! - MPRIS track info and playback commands
//! - Persistent runtime store at `$XDG_RUNTIME_DIR/flex-notify.json`

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Urgency level matching the `FreeDesktop` notification specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "low" => Self::Low,
            "critical" | "crit" => Self::Critical,
            _ => Self::Normal,
        }
    }
}

/// Action button attached to a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    pub id: String,
    pub title: String,
}

/// A rich notification item stored in the feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationItem {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub urgency: Urgency,
    pub timestamp: u64,
    pub progress: Option<f32>,
    pub app_icon: Option<String>,
    pub image_path: Option<String>,
    pub actions: Vec<NotificationAction>,
    pub is_pinned: bool,
    pub is_snoozed: bool,
    pub snooze_until: Option<u64>,
    pub is_dismissed: bool,
}

impl NotificationItem {
    /// Create a new notification with default flags.
    #[must_use]
    pub fn new(
        id: u32,
        app_name: impl Into<String>,
        summary: impl Into<String>,
        body: impl Into<String>,
        urgency: Urgency,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self {
            id,
            app_name: app_name.into(),
            summary: summary.into(),
            body: body.into(),
            urgency,
            timestamp,
            progress: None,
            app_icon: None,
            image_path: None,
            actions: Vec::new(),
            is_pinned: urgency == Urgency::Critical,
            is_snoozed: false,
            snooze_until: None,
            is_dismissed: false,
        }
    }
}

/// Extracted actionable entity from notification text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractedEntity {
    OtpCode(String),
    Url(String),
    HexColor(String),
}

/// Extract actionable entities (OTP 2FA codes, URLs, hex color codes) from text.
#[must_use]
pub fn extract_entities(text: &str) -> Vec<ExtractedEntity> {
    let mut entities = Vec::new();

    // 1. Scan for URLs (https:// or http://)
    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| {
            c == '('
                || c == ')'
                || c == '<'
                || c == '>'
                || c == '"'
                || c == '\''
                || c == ','
                || c == '.'
        });
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            entities.push(ExtractedEntity::Url(trimmed.to_string()));
        }
    }

    // 2. Scan for Hex color codes (#RRGGBB)
    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#');
        if trimmed.starts_with('#') && (trimmed.len() == 7 || trimmed.len() == 4) {
            let hex_part = &trimmed[1..];
            if hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
                entities.push(ExtractedEntity::HexColor(trimmed.to_string()));
            }
        }
    }

    // 3. Scan for OTP / 2FA verification codes (4 to 8 consecutive digits)
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, &word) in words.iter().enumerate() {
        let clean = word.trim_matches(|c: char| !c.is_ascii_digit());
        if (4..=8).contains(&clean.len()) && clean.chars().all(|c| c.is_ascii_digit()) {
            // Check context if available or if standalone
            let is_contextual = if i > 0 {
                let prev = words[i - 1].to_lowercase();
                prev.contains("code")
                    || prev.contains("otp")
                    || prev.contains("pin")
                    || prev.contains("is")
                    || prev.contains("verification")
            } else {
                false
            };
            if (is_contextual || clean.len() == 6)
                && !entities
                    .iter()
                    .any(|e| matches!(e, ExtractedEntity::OtpCode(c) if c == clean))
            {
                entities.push(ExtractedEntity::OtpCode(clean.to_string()));
            }
        }
    }

    entities
}

/// Do Not Disturb state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DndState {
    Off,
    Indefinite,
    Timed { until: u64, total_secs: u64 },
}

impl DndState {
    #[must_use]
    pub fn is_active(self, now: u64) -> bool {
        match self {
            Self::Off => false,
            Self::Indefinite => true,
            Self::Timed { until, .. } => now < until,
        }
    }

    #[must_use]
    pub fn remaining_secs(self, now: u64) -> Option<u64> {
        match self {
            Self::Timed { until, .. } if now < until => Some(until - now),
            _ => None,
        }
    }

    #[must_use]
    pub fn fraction_remaining(self, now: u64) -> Option<f32> {
        match self {
            Self::Timed { until, total_secs } if total_secs > 0 && now < until => {
                let remaining = until.saturating_sub(now);
                #[allow(clippy::cast_precision_loss)]
                Some(((remaining as f32) / (total_secs as f32)).clamp(0.0, 1.0))
            }
            _ => None,
        }
    }
}

/// MPRIS Now Playing track status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MprisTrack {
    pub player: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub position_secs: u64,
    pub length_secs: u64,
    pub is_playing: bool,
    pub is_live: bool,
    pub art_url: Option<String>,
}

/// Status of connected peripheral batteries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeripheralDevice {
    pub name: String,
    pub battery: u8,
    pub icon: String,
}

/// Quick controls shelf snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickControls {
    pub dnd: DndState,
    pub night_light: bool,
    pub caffeine: bool,
    pub mic_muted: bool,
    pub wifi_ssid: Option<String>,
    pub bluetooth_status: Option<String>,
    pub peripherals: Vec<PeripheralDevice>,
    pub mpris: Option<MprisTrack>,
}

impl Default for QuickControls {
    fn default() -> Self {
        Self {
            dnd: DndState::Off,
            night_light: false,
            caffeine: false,
            mic_muted: false,
            wifi_ssid: None,
            bluetooth_status: None,
            peripherals: Vec::new(),
            mpris: None,
        }
    }
}

/// Parse a raw metadata line formatted by `playerctl`.
#[must_use]
pub fn parse_mpris_line(line: &str) -> Option<MprisTrack> {
    let parts: Vec<&str> = line.split(";;").collect();
    if parts.len() < 7 {
        return None;
    }

    let player = parts[0].trim().to_string();
    let status = parts[1].trim();
    let artist = parts[2].trim().to_string();
    let title = parts[3].trim().to_string();
    let album_raw = parts[4].trim();
    let album = if album_raw.is_empty() {
        None
    } else {
        Some(album_raw.to_string())
    };

    let pos_micro: u64 = parts[5].trim().parse().unwrap_or(0);
    let len_micro: u64 = parts[6].trim().parse().unwrap_or(0);

    let art_url = if parts.len() > 7 {
        let raw_art = parts[7].trim();
        if raw_art.is_empty() {
            None
        } else {
            Some(
                raw_art
                    .strip_prefix("file://")
                    .unwrap_or(raw_art)
                    .to_string(),
            )
        }
    } else {
        None
    };

    let position_secs = pos_micro / 1_000_000;
    // Chromium and Twitch send i64::MAX (9223372036854775807) for live streams.
    let is_live = len_micro > (86400 * 7 * 1_000_000) || len_micro == 0;
    let length_secs = if is_live { 0 } else { len_micro / 1_000_000 };

    if title.is_empty() && artist.is_empty() {
        return None;
    }

    Some(MprisTrack {
        player,
        title: if title.is_empty() {
            "Unknown Title".to_string()
        } else {
            title
        },
        artist: if artist.is_empty() {
            "Unknown Artist".to_string()
        } else {
            artist
        },
        album,
        position_secs,
        length_secs,
        is_playing: status.eq_ignore_ascii_case("Playing"),
        is_live,
        art_url,
    })
}

/// Query `playerctl` for the active media player metadata.
#[must_use]
pub fn probe_mpris() -> Option<MprisTrack> {
    let output = std::process::Command::new("playerctl")
        .args([
            "metadata",
            "--format",
            "{{playerName}};;{{status}};;{{artist}};;{{title}};;{{album}};;{{position}};;{{mpris:length}};;{{mpris:artUrl}}",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?.trim();
    if first_line.is_empty() {
        return None;
    }

    parse_mpris_line(first_line)
}

/// Query `wpctl` to determine if the default microphone is muted.
#[must_use]
pub fn probe_mic_muted() -> bool {
    std::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
        .output()
        .ok()
        .is_some_and(|out| String::from_utf8_lossy(&out.stdout).contains("[MUTED]"))
}

/// Update hardware & MPRIS statuses in `QuickControls` while preserving active DND state.
pub fn probe_quick_controls(controls: &mut QuickControls) {
    controls.mpris = probe_mpris();
    controls.mic_muted = probe_mic_muted();
}

/// Complete runtime state stored in `$XDG_RUNTIME_DIR/flex-notify.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NotifyState {
    pub notifications: Vec<NotificationItem>,
    pub controls: QuickControls,
    pub muted_apps: Vec<String>,
    pub priority_apps: Vec<String>,
}

/// Resolve the path to the runtime state JSON file.
#[must_use]
pub fn state_file_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("flex-notify.json")
    } else {
        std::env::temp_dir().join("flex-notify.json")
    }
}

/// Load state from JSON file, returning default state if missing or corrupted.
#[must_use]
pub fn load_state(path: Option<&Path>) -> NotifyState {
    let default_path = state_file_path();
    let file = path.unwrap_or(&default_path);

    if let Ok(content) = std::fs::read_to_string(file) {
        if let Ok(state) = serde_json::from_str::<NotifyState>(&content) {
            return state;
        }
    }
    NotifyState::default()
}

/// Save state to JSON file with atomic rename.
///
/// # Errors
/// Returns error if file cannot be created or serialized.
pub fn save_state(state: &NotifyState, path: Option<&Path>) -> anyhow::Result<()> {
    let default_path = state_file_path();
    let file = path.unwrap_or(&default_path);

    let json = serde_json::to_string_pretty(state)?;
    let tmp_file = file.with_extension("tmp");
    std::fs::write(&tmp_file, json)?;
    std::fs::rename(tmp_file, file)?;
    Ok(())
}

/// Append a new notification to the active state store.
///
/// # Errors
/// Returns error if state cannot be loaded or saved.
pub fn post_notification(item: NotificationItem, path: Option<&Path>) -> anyhow::Result<u32> {
    let mut state = load_state(path);
    let next_id = state.notifications.iter().map(|n| n.id).max().unwrap_or(0) + 1;
    let mut final_item = item;
    if final_item.id == 0 {
        final_item.id = next_id;
    }
    let assigned_id = final_item.id;
    state.notifications.push(final_item);
    save_state(&state, path)?;
    Ok(assigned_id)
}

/// Format relative time (e.g. "Just Now", "2m ago", "1h ago", "2d ago").
#[must_use]
pub fn format_relative_time(timestamp: u64, now: u64) -> String {
    if now <= timestamp || now - timestamp < 45 {
        return String::from("Just Now");
    }
    let diff = now - timestamp;
    if diff < 3600 {
        let mins = diff / 60;
        format!("{mins}m ago")
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{hours}h ago")
    } else {
        let days = diff / 86400;
        format!("{days}d ago")
    }
}

/// Format duration in mm:ss.
#[must_use]
pub fn format_duration(seconds: u64) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{mins:02}:{secs:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_extractor_detects_otp_urls_and_colors() {
        let text = "Your GitHub verification code is 849201. Please visit https://github.com/login and use theme #7aa2f7.";
        let entities = extract_entities(text);

        assert_eq!(
            entities,
            vec![
                ExtractedEntity::Url("https://github.com/login".to_string()),
                ExtractedEntity::HexColor("#7aa2f7".to_string()),
                ExtractedEntity::OtpCode("849201".to_string()),
            ]
        );
    }

    #[test]
    fn relative_time_formatting() {
        assert_eq!(format_relative_time(1000, 1020), "Just Now");
        assert_eq!(format_relative_time(1000, 1120), "2m ago");
        assert_eq!(format_relative_time(1000, 4600), "1h ago");
        assert_eq!(format_relative_time(1000, 90000), "1d ago");
    }

    #[test]
    fn dnd_state_calculations() {
        let dnd = DndState::Timed {
            until: 2000,
            total_secs: 1000,
        };
        assert!(dnd.is_active(1500));
        assert!(!dnd.is_active(2500));
        assert_eq!(dnd.remaining_secs(1500), Some(500));
        assert_eq!(dnd.fraction_remaining(1500), Some(0.5));
    }

    #[test]
    fn state_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("flex-notify-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-state.json");

        let mut state = NotifyState::default();
        state.notifications.push(NotificationItem::new(
            1,
            "Discord",
            "#dev-team",
            "Message body",
            Urgency::Normal,
        ));
        state.controls.night_light = true;

        save_state(&state, Some(&path)).expect("saved");
        let loaded = load_state(Some(&path));

        assert_eq!(loaded.notifications.len(), 1);
        assert_eq!(loaded.notifications[0].app_name, "Discord");
        assert!(loaded.controls.night_light);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
