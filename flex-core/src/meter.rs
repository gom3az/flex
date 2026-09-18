//! Peak meter rendering — faithful port of wiremix `src/meter.rs`
//! (upstream commit `cbdc90f`).
//!
//! Upstream's math is kept verbatim: peak amplitudes are converted to dB
//! (`20·log10`), clamped to `-60..+6 dB`, normalized onto
//! `10^(-60/60)..10^(6/60)`, then split into inactive / active / overload
//! segments. Stereo draws `left │ live │ right` (`Fill(2)`, `Length(2)`,
//! `Fill(2)` with spacing 1); mono draws `live │ meter`
//! (`Length(1)`, `Fill(2)`).

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::charset::CharSet;
use crate::theme::Theme;
use crate::{Peaks, RowPeaks};

/// Split a peak into `(active, overload, inactive)` cell counts
/// (upstream `meter.rs:11-39`).
///
/// The float-to-integer casts are upstream's own arithmetic
/// (`round() as usize`); a meter is at most a few hundred cells wide, so the
/// conversions cannot lose anything that matters.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn segments(peak: f32, width: usize) -> (usize, usize, usize) {
    fn normalize(value: f32) -> f32 {
        let amplitude = 10.0_f32.powf(value / 60.0);
        let min = 10.0_f32.powf(-60.0 / 60.0);
        let max = 10.0_f32.powf(6.0 / 60.0);

        (amplitude - min) / (max - min)
    }

    let db = 20.0 * (peak + 1e-10).log10();
    let vu_value = db.clamp(-60.0, 6.0);

    let meter = normalize(vu_value);

    let total_chars = width;
    let lit = ((meter * total_chars as f32).round() as usize).min(total_chars);

    let zero_char = (normalize(0.0) * total_chars as f32).round() as usize;

    let active_size = lit.min(zero_char);
    let overload_size = lit.saturating_sub(zero_char);
    let inactive_size = total_chars
        .saturating_sub(active_size)
        .saturating_sub(overload_size);

    (active_size, overload_size, inactive_size)
}

/// Render the meter for `peaks` in `area`, honoring the menu's [`Peaks`] mode.
pub fn render(
    area: Rect,
    buf: &mut Buffer,
    peaks: RowPeaks,
    mode: Peaks,
    char_set: &CharSet,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 || mode == Peaks::Off {
        return;
    }
    match peaks {
        RowPeaks::Stereo(left, right) if mode != Peaks::Mono => {
            render_stereo(area, buf, left, right, char_set, theme);
        }
        // `Peaks::Mono` collapses stereo rows to their average, like upstream.
        RowPeaks::Mono(peak) => render_mono(area, buf, peak, char_set, theme),
        RowPeaks::Stereo(left, right) => {
            render_mono(area, buf, left.midpoint(right), char_set, theme);
        }
    }
}

/// Stereo meter: left bars grow from the right edge, right bars from the
/// left, with the `meter_center_*` live indicator between them
/// (upstream `meter.rs:41-121`).
pub fn render_stereo(
    meter_area: Rect,
    buf: &mut Buffer,
    left_peak: f32,
    right_peak: f32,
    char_set: &CharSet,
    theme: &Theme,
) {
    let [meter_left, meter_live, meter_right] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(2),
            Constraint::Length(2),
            Constraint::Fill(2),
        ])
        .spacing(1)
        .areas(meter_area);

    let (active, overload, inactive) = segments(left_peak, meter_left.width as usize);
    Line::from(vec![
        Span::styled(
            char_set.meter_left_inactive.repeat(inactive),
            theme.meter_inactive,
        ),
        Span::styled(
            char_set.meter_left_overload.repeat(overload),
            theme.meter_overload,
        ),
        Span::styled(
            char_set.meter_left_active.repeat(active),
            theme.meter_active,
        ),
    ])
    .alignment(Alignment::Right)
    .render(meter_left, buf);

    let (active, overload, inactive) = segments(right_peak, meter_right.width as usize);
    Line::from(vec![
        Span::styled(
            char_set.meter_right_active.repeat(active),
            theme.meter_active,
        ),
        Span::styled(
            char_set.meter_right_overload.repeat(overload),
            theme.meter_overload,
        ),
        Span::styled(
            char_set.meter_right_inactive.repeat(inactive),
            theme.meter_inactive,
        ),
    ])
    .render(meter_right, buf);

    Line::from(Span::styled(
        format!(
            "{}{}",
            char_set.meter_center_left_active, char_set.meter_center_right_active,
        ),
        theme.meter_center_active,
    ))
    .render(meter_live, buf);
}

/// Mono meter: `meter_center_right_active` live indicator, then one bar
/// (upstream `meter.rs:123-172`).
pub fn render_mono(
    meter_area: Rect,
    buf: &mut Buffer,
    peak: f32,
    char_set: &CharSet,
    theme: &Theme,
) {
    let [meter_live, meter_mono] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(1), Constraint::Fill(2)])
        .spacing(1)
        .areas(meter_area);

    let (active, overload, inactive) = segments(peak, meter_mono.width as usize);
    Line::from(vec![
        Span::styled(
            char_set.meter_right_active.repeat(active),
            theme.meter_active,
        ),
        Span::styled(
            char_set.meter_right_overload.repeat(overload),
            theme.meter_overload,
        ),
        Span::styled(
            char_set.meter_right_inactive.repeat(inactive),
            theme.meter_inactive,
        ),
    ])
    .render(meter_mono, buf);

    Line::from(Span::styled(
        char_set.meter_center_right_active,
        theme.meter_center_active,
    ))
    .render(meter_live, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn theme() -> Theme {
        Theme::DEFAULT
    }

    #[test]
    fn db_range_maps_onto_the_full_bar() {
        // -60 dB (or quieter) is empty, +6 dB or louder is full.
        assert_eq!(segments(0.0, 10).2, 10, "silence is all inactive");
        assert_eq!(segments(1e-3, 10), (0, 0, 10));
        let (active, overload, inactive) = segments(10.0, 10);
        assert_eq!(inactive, 0, "very loud fills the bar");
        assert_eq!(active + overload, 10);
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn zero_db_sits_at_the_overload_boundary() {
        let width = 20;
        let (active, overload, _) = segments(1.0, width);
        let zero_char = (10.0_f32.powf(0.0 / 60.0) - 10.0_f32.powf(-1.0))
            / (10.0_f32.powf(6.0 / 60.0) - 10.0_f32.powf(-1.0));
        let expected = (zero_char * width as f32).round() as usize;
        assert_eq!(active, expected, "0 dB is where red starts");
        assert_eq!(overload, 0);
    }

    #[test]
    fn mono_renders_live_indicator_and_bar() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
        render_mono(
            Rect::new(0, 0, 12, 1),
            &mut buf,
            1.0,
            &CharSet::DEFAULT,
            &theme(),
        );
        assert_eq!(buf.cell((0, 0)).expect("cell").symbol(), "▮");
        assert_eq!(
            buf.cell((0, 0)).expect("cell").fg,
            Color::LightGreen,
            "live indicator uses meter_center_active"
        );
        let text: String = (0..12)
            .map(|x| buf.cell((x, 0)).expect("cell").symbol())
            .collect();
        assert!(text.contains('▮'), "mono bar renders: {text:?}");
    }

    #[test]
    fn stereo_renders_live_pair_between_the_channels() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        render_stereo(
            Rect::new(0, 0, 20, 1),
            &mut buf,
            1e-4,
            1e-4,
            &CharSet::DEFAULT,
            &theme(),
        );
        let text: String = (0..20)
            .map(|x| buf.cell((x, 0)).expect("cell").symbol())
            .collect();
        // Fill(2)/Length(2)/Fill(2) with spacing 1 over 20 cells:
        // left 8, gap 1, live 2, gap 1, right 8.
        assert_eq!(text.chars().filter(|c| *c == '▮').count(), 18);
        assert_eq!(text.chars().filter(|c| *c == ' ').count(), 2);
        // Quiet input keeps the inactive color everywhere.
        assert_eq!(
            buf.cell((0, 0)).expect("cell").fg,
            Color::DarkGray,
            "quiet channel is inactive"
        );
    }

    #[test]
    fn peaks_off_draws_nothing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        render(
            Rect::new(0, 0, 10, 1),
            &mut buf,
            RowPeaks::Stereo(1.0, 1.0),
            Peaks::Off,
            &CharSet::DEFAULT,
            &theme(),
        );
        let text: String = (0..10)
            .map(|x| buf.cell((x, 0)).expect("cell").symbol())
            .collect();
        assert_eq!(text, " ".repeat(10), "Peaks::Off renders no meter");
    }

    #[test]
    fn mono_mode_collapses_stereo_input() {
        let mut stereo = Buffer::empty(Rect::new(0, 0, 10, 1));
        let mut mono = Buffer::empty(Rect::new(0, 0, 10, 1));
        render(
            Rect::new(0, 0, 10, 1),
            &mut stereo,
            RowPeaks::Stereo(0.0, 1.0),
            Peaks::Auto,
            &CharSet::DEFAULT,
            &theme(),
        );
        render(
            Rect::new(0, 0, 10, 1),
            &mut mono,
            RowPeaks::Stereo(0.0, 1.0),
            Peaks::Mono,
            &CharSet::DEFAULT,
            &theme(),
        );
        assert_ne!(stereo, mono, "Peaks::Mono averages instead of splitting");
    }
}
