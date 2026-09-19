//! Recording-options provider: the drill-in submenu behind the two
//! recording rows (`area-rec`, `full-rec`).
//!
//! One options tab plus one tab per dimension (Audio, Quality, Framerate).
//! The options tab's first row confirms with the live effective settings in
//! its label, so the defaults path is Enter-Enter; the dimension rows reopen
//! their tab for a one-pick override, then return to the options tab. The
//! runner matches on these ids after the TUI exits, same as shot.
//!
//! Effective settings resolve from the environment ([`RecOptions::from_env`])
//! and ride to the detached worker the same way (the parent exports them
//! before the detach; `setsid` preserves env). [`Quality`] presets map to
//! `wf-recorder` bitrate params; the codec (`av1_vaapi`) and container
//! (`mp4`) stay fixed.
//!
//! The library never executes captures; it only selects rows.

use flex_core::{Row, RowId, Tab};

/// Provider name for the `ACTION:` line and the usage store.
pub const PROVIDER: &str = "rec-opt";
/// Options tab title.
pub const TAB_NAME: &str = "Recording";

/// Env override for recording audio (`0`/`false`/`no`/`off` mutes; unset or
/// anything else records the `PipeWire` default source).
pub const AUDIO_ENV: &str = "FLEX_REC_AUDIO";
/// Env override for the quality preset (`light`/`balanced`/`high`).
pub const QUALITY_ENV: &str = "FLEX_REC_QUALITY";
/// Env override for the framerate (`30`/`60`).
pub const FPS_ENV: &str = "FLEX_REC_FPS";

/// Options-tab row ids.
pub const CONFIRM_ID: &str = "rec-confirm";
pub const AUDIO_ID: &str = "rec-audio";
pub const QUALITY_ID: &str = "rec-quality";
pub const FPS_ID: &str = "rec-fps";
/// Audio-dimension value ids.
pub const AUDIO_ON_ID: &str = "rec-audio-on";
pub const AUDIO_OFF_ID: &str = "rec-audio-off";
/// Quality-dimension value ids.
pub const QUALITY_LIGHT_ID: &str = "rec-quality-light";
pub const QUALITY_BALANCED_ID: &str = "rec-quality-balanced";
pub const QUALITY_HIGH_ID: &str = "rec-quality-high";
/// Framerate-dimension value ids.
pub const FPS_30_ID: &str = "rec-fps-30";
pub const FPS_60_ID: &str = "rec-fps-60";

/// Default framerate (also the `wf-recorder -r` value before options).
pub const DEFAULT_FPS: u32 = 30;

/// Encoding quality preset: bitrate ladder over the fixed `av1_vaapi`
/// codec (`-p b=<bitrate> -p maxrate=<bitrate>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// `2M` bitrate (small files, screen-share friendly).
    Light,
    /// `5M` bitrate (today's hardcoded values).
    #[default]
    Balanced,
    /// `10M` bitrate (archival).
    High,
}

impl Quality {
    /// Parse a preset name (case-insensitive); `None` for anything else
    /// (callers fall back to [`Quality::default`]).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "balanced" => Some(Self::Balanced),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// Env/flag spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Balanced => "balanced",
            Self::High => "high",
        }
    }

    /// Menu spelling.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Balanced => "Balanced",
            Self::High => "High",
        }
    }

    /// `wf-recorder -p b=`/`maxrate=` value.
    #[must_use]
    pub fn bitrate(self) -> &'static str {
        match self {
            Self::Light => "2M",
            Self::Balanced => "5M",
            Self::High => "10M",
        }
    }
}

/// Effective recording settings: audio on/off, quality preset, framerate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecOptions {
    /// Whether the `PipeWire` default source is captured (`-a`).
    pub audio: bool,
    /// Encoding quality preset.
    pub quality: Quality,
    /// Constant framerate (`wf-recorder -r`).
    pub fps: u32,
}

impl RecOptions {
    /// Built-in defaults: audio on, balanced, 30 fps (today's `*-rec-audio`
    /// command shape).
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            audio: true,
            quality: Quality::Balanced,
            fps: DEFAULT_FPS,
        }
    }

    /// Resolve from [`AUDIO_ENV`]/[`QUALITY_ENV`]/[`FPS_ENV`]: unset, empty
    /// and garbage values all fall back to [`RecOptions::defaults`], like
    /// the wrapper's `${VAR:-default}`.
    #[must_use]
    pub fn from_env() -> Self {
        let audio = match std::env::var(AUDIO_ENV) {
            Ok(value) => !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            ),
            Err(_) => true,
        };
        let quality = std::env::var(QUALITY_ENV)
            .ok()
            .and_then(|value| Quality::parse(&value))
            .unwrap_or_default();
        let fps = std::env::var(FPS_ENV)
            .ok()
            .and_then(|value| {
                value
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|fps| (1..=240).contains(fps))
            })
            .unwrap_or(DEFAULT_FPS);
        Self {
            audio,
            quality,
            fps,
        }
    }

    /// Compact settings spelling for the confirm label
    /// (`Audio · Balanced · 30fps` / `Muted · High · 60fps`).
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · {}fps",
            if self.audio { "Audio" } else { "Muted" },
            self.quality.label(),
            self.fps,
        )
    }
}

/// Which dimension tab a submenu pick opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    /// Audio on/off.
    Audio,
    /// Quality preset.
    Quality,
    /// Framerate.
    Fps,
}

/// What the runner does with one submenu pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The confirm row: record with the current settings.
    Confirm,
    /// A dimension row: open that tab, then return to the options tab.
    Open(Dimension),
    /// A value row: settings updated, return to the options tab.
    Updated,
    /// Anything else: ignore, re-show the options tab.
    Unknown,
}

/// Apply one submenu `action_id` to `opts` (pure state machine; the runner
/// owns the TUI loops around it).
#[must_use]
pub fn apply_choice(opts: &mut RecOptions, action_id: &str) -> Choice {
    match action_id {
        CONFIRM_ID => Choice::Confirm,
        AUDIO_ID => Choice::Open(Dimension::Audio),
        QUALITY_ID => Choice::Open(Dimension::Quality),
        FPS_ID => Choice::Open(Dimension::Fps),
        AUDIO_ON_ID => {
            opts.audio = true;
            Choice::Updated
        }
        AUDIO_OFF_ID => {
            opts.audio = false;
            Choice::Updated
        }
        QUALITY_LIGHT_ID => {
            opts.quality = Quality::Light;
            Choice::Updated
        }
        QUALITY_BALANCED_ID => {
            opts.quality = Quality::Balanced;
            Choice::Updated
        }
        QUALITY_HIGH_ID => {
            opts.quality = Quality::High;
            Choice::Updated
        }
        FPS_30_ID => {
            opts.fps = 30;
            Choice::Updated
        }
        FPS_60_ID => {
            opts.fps = 60;
            Choice::Updated
        }
        _ => Choice::Unknown,
    }
}

/// Build the options tab: confirm-with-live-summary first, then one drill-in
/// row per dimension carrying its current value.
#[must_use]
pub fn options_tab(opts: &RecOptions) -> Tab {
    let mut tab = Tab::with_rows(
        TAB_NAME,
        vec![
            Row::with_meta(
                RowId::new(CONFIRM_ID),
                format!("Record ({})", opts.summary()),
                "MP4",
            ),
            Row::with_meta(
                RowId::new(AUDIO_ID),
                format!("Audio: {}", if opts.audio { "On" } else { "Muted" }),
                "",
            ),
            Row::with_meta(
                RowId::new(QUALITY_ID),
                format!("Quality: {}", opts.quality.label()),
                "",
            ),
            Row::with_meta(
                RowId::new(FPS_ID),
                format!("Framerate: {}fps", opts.fps),
                "",
            ),
        ],
    );
    tab.filterable = false;
    tab
}

/// Build the audio-dimension tab.
#[must_use]
pub fn audio_tab() -> Tab {
    let mut tab = Tab::with_rows(
        "Audio",
        vec![
            Row::with_meta(RowId::new(AUDIO_ON_ID), "With Audio", ""),
            Row::with_meta(RowId::new(AUDIO_OFF_ID), "Muted (no audio track)", ""),
        ],
    );
    tab.filterable = false;
    tab
}

/// Build the quality-dimension tab.
#[must_use]
pub fn quality_tab() -> Tab {
    let mut tab = Tab::with_rows(
        "Quality",
        vec![
            Row::with_meta(RowId::new(QUALITY_LIGHT_ID), "Light (2 Mbps)", ""),
            Row::with_meta(RowId::new(QUALITY_BALANCED_ID), "Balanced (5 Mbps)", ""),
            Row::with_meta(RowId::new(QUALITY_HIGH_ID), "High (10 Mbps)", ""),
        ],
    );
    tab.filterable = false;
    tab
}

/// Build the framerate-dimension tab.
#[must_use]
pub fn fps_tab() -> Tab {
    let mut tab = Tab::with_rows(
        "Framerate",
        vec![
            Row::with_meta(RowId::new(FPS_30_ID), "30 fps", ""),
            Row::with_meta(RowId::new(FPS_60_ID), "60 fps", ""),
        ],
    );
    tab.filterable = false;
    tab
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_vars(vars: &[(&str, Option<&str>)], check: impl FnOnce()) {
        let _guard = ENV_MUTEX.lock().unwrap();
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect();
        for (key, value) in vars {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        check();
        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn quality_parses_case_insensitively_and_rejects_garbage() {
        assert_eq!(Quality::parse("light"), Some(Quality::Light));
        assert_eq!(Quality::parse(" High "), Some(Quality::High));
        assert_eq!(Quality::parse("BALANCED"), Some(Quality::Balanced));
        assert_eq!(Quality::parse("ultra"), None);
        assert_eq!(Quality::parse(""), None);
        assert_eq!(Quality::default(), Quality::Balanced);
    }

    #[test]
    fn quality_maps_to_the_bitrate_ladder() {
        assert_eq!(Quality::Light.bitrate(), "2M");
        assert_eq!(Quality::Balanced.bitrate(), "5M");
        assert_eq!(Quality::High.bitrate(), "10M");
    }

    #[test]
    fn from_env_defaults_when_unset() {
        with_vars(
            &[(AUDIO_ENV, None), (QUALITY_ENV, None), (FPS_ENV, None)],
            || assert_eq!(RecOptions::from_env(), RecOptions::defaults()),
        );
    }

    #[test]
    fn from_env_honors_overrides_and_falls_back_on_garbage() {
        with_vars(
            &[
                (AUDIO_ENV, Some("0")),
                (QUALITY_ENV, Some("high")),
                (FPS_ENV, Some("60")),
            ],
            || {
                assert_eq!(
                    RecOptions::from_env(),
                    RecOptions {
                        audio: false,
                        quality: Quality::High,
                        fps: 60,
                    }
                );
            },
        );
        with_vars(
            &[
                (AUDIO_ENV, Some("off")),
                (QUALITY_ENV, Some("ultra")),
                (FPS_ENV, Some("banana")),
            ],
            || {
                let opts = RecOptions::from_env();
                assert!(!opts.audio);
                assert_eq!(opts.quality, Quality::Balanced);
                assert_eq!(opts.fps, DEFAULT_FPS);
            },
        );
    }

    #[test]
    fn summary_spells_audio_and_muted() {
        assert_eq!(RecOptions::defaults().summary(), "Audio · Balanced · 30fps");
        assert_eq!(
            RecOptions {
                audio: false,
                quality: Quality::High,
                fps: 60,
            }
            .summary(),
            "Muted · High · 60fps"
        );
    }

    #[test]
    fn options_tab_confirms_first_with_the_live_summary() {
        let opts = RecOptions {
            audio: false,
            quality: Quality::Light,
            fps: 60,
        };
        let tab = options_tab(&opts);
        assert_eq!(tab.name, TAB_NAME);
        let ids: Vec<&str> = tab.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec![CONFIRM_ID, AUDIO_ID, QUALITY_ID, FPS_ID]);
        assert_eq!(tab.rows[0].label, "Record (Muted · Light · 60fps)");
        assert_eq!(tab.rows[1].label, "Audio: Muted");
        assert_eq!(tab.rows[2].label, "Quality: Light");
        assert_eq!(tab.rows[3].label, "Framerate: 60fps");
    }

    #[test]
    fn apply_choice_drives_the_state_machine() {
        let mut opts = RecOptions::defaults();
        assert_eq!(apply_choice(&mut opts, CONFIRM_ID), Choice::Confirm);
        assert_eq!(
            apply_choice(&mut opts, AUDIO_ID),
            Choice::Open(Dimension::Audio)
        );
        assert_eq!(apply_choice(&mut opts, AUDIO_OFF_ID), Choice::Updated);
        assert!(!opts.audio);
        assert_eq!(apply_choice(&mut opts, QUALITY_HIGH_ID), Choice::Updated);
        assert_eq!(opts.quality, Quality::High);
        assert_eq!(apply_choice(&mut opts, FPS_60_ID), Choice::Updated);
        assert_eq!(opts.fps, 60);
        assert_eq!(apply_choice(&mut opts, "bogus"), Choice::Unknown);
    }

    #[test]
    fn dimension_tabs_list_every_value() {
        let tab = audio_tab();
        let ids: Vec<&str> = tab.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec![AUDIO_ON_ID, AUDIO_OFF_ID]);
        let tab = quality_tab();
        let ids: Vec<&str> = tab.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![QUALITY_LIGHT_ID, QUALITY_BALANCED_ID, QUALITY_HIGH_ID]
        );
        let tab = fps_tab();
        let ids: Vec<&str> = tab.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec![FPS_30_ID, FPS_60_ID]);
    }
}
