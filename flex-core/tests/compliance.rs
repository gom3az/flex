//! Wiremix compliance tests.
//!
//! Each assertion pins a value or relationship to the upstream source it was
//! ported from (github.com/tsowell/wiremix @ `cbdc90f`), so drift from the
//! design system fails here with a pointer to the upstream line. Rendered
//! detail (row order, selector block, colour styles) is covered by
//! `tests/golden.rs`; this file locks the constants and the cross-cutting
//! geometry.

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;
use ratatui::Terminal;

use flex_core::{
    render, width, CharSet, CharSetName, Menu, Peaks, Row, RowId, RowPeaks, Tab, Target, Theme,
    ThemeName,
};

fn menu_with(rows: Vec<Row>) -> Menu {
    let mut tab = Tab::with_rows("apps", rows);
    tab.bare_rows = false;
    Menu::new("test", vec![tab])
}

fn draw(menu: &mut Menu, w: u16, h: u16) -> Buffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    terminal
        .draw(|frame| render::render(frame, menu))
        .expect("render frame");
    terminal.backend().buffer().clone()
}

fn row_text(buf: &Buffer, y: u16, w: u16) -> String {
    let mut out = String::new();
    for x in 0..w {
        let cell = buf.cell((x, y)).expect("cell in frame");
        if !cell.skip {
            out.push_str(cell.symbol());
        }
    }
    out
}

// --- geometry -------------------------------------------------------------

/// `NodeWidget::height() == 3`, `NodeWidget::spacing() == 2`
/// (`node_widget.rs:53-60`).
#[test]
fn node_geometry_matches_upstream() {
    assert_eq!(render::NODE_HEIGHT, 3);
    assert_eq!(render::NODE_SPACING, 2);
}

/// Node pitch follows the tab's metrics: upstream's 3 + 2 rows when a row
/// draws a detail line, flex's compact 1 + 1 rows when none does.
#[test]
fn entry_pitch_matches_the_node_metrics() {
    let rows: Vec<Row> = (0..3)
        .map(|i| Row::new(RowId::new(format!("id-{i}")), format!("Entry {i}")))
        .collect();
    let mut menu = menu_with(rows);
    let buf = draw(&mut menu, 80, 24);
    assert_eq!(
        render::node_metrics(&menu),
        render::NodeMetrics::COMPACT,
        "no row has a detail line"
    );
    assert_eq!(
        entry_headers(&buf, "Entry "),
        vec![4, 6, 8],
        "compact pitch 2 (filter chrome at y=0..3, indicator at y=3)"
    );
    assert_eq!(
        render::node_metrics(&menu).pitch(),
        render::COMPACT_NODE_HEIGHT + render::COMPACT_NODE_SPACING
    );

    let mut menu = menu_with(vec![
        Row::with_volume(RowId::new("id-vol"), "Volume 1", 0.5),
        Row::new(RowId::new("id-plain"), "Entry 2"),
    ]);
    let buf = draw(&mut menu, 80, 24);
    assert_eq!(
        render::node_metrics(&menu),
        render::NodeMetrics::UPSTREAM,
        "one detail-bearing row switches the tab to upstream metrics"
    );
    assert_eq!(entry_headers(&buf, "Volume 1"), vec![4], "first row at y=4");
    assert_eq!(entry_headers(&buf, "Entry 2"), vec![9], "upstream pitch 5");
    assert_eq!(
        render::node_metrics(&menu).pitch(),
        render::NODE_HEIGHT + render::NODE_SPACING
    );
}

/// Rows whose label contains `needle` (entry header lines).
fn entry_headers(buf: &Buffer, needle: &str) -> Vec<u16> {
    (0..buf.area.height)
        .filter(|y| row_text(buf, *y, buf.area.width).contains(needle))
        .collect()
}

/// The viewport counts entries, not visual rows (`object_list.rs:246-256`).
#[test]
fn viewport_counts_entries() {
    let rows: Vec<Row> = (0..12)
        .map(|i| Row::new(RowId::new(format!("id-{i}")), format!("App {i}")))
        .collect();
    let mut menu = menu_with(rows);
    let _ = draw(&mut menu, 80, 24);
    let pitch = usize::from(render::node_metrics(&menu).pitch());
    // 80x24: list height 20 (filter top chrome 3 + tab bar chrome 1) minus 2
    // indicator rows = 18.
    let visible = 18 / pitch;
    assert!(visible > 1, "sanity: {visible} entries fit");

    menu.app.active_tab_mut().expect("tab").state.focus = visible;
    let _ = draw(&mut menu, 80, 24);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.scroll,
        1,
        "focus past a {visible}-entry viewport scrolls by one entry"
    );

    menu.app.active_tab_mut().expect("tab").state.focus = 11;
    let _ = draw(&mut menu, 80, 24);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.scroll,
        11 + 1 - visible,
        "last entry lands on the viewport bottom"
    );
}

/// `•••` is only drawn when there are rows outside the viewport
/// (`object_list.rs:478-517`).
#[test]
fn list_more_only_when_scrollable() {
    let mut short = menu_with(vec![
        Row::new(RowId::new("a"), "alpha"),
        Row::new(RowId::new("b"), "bravo"),
    ]);
    let buf = draw(&mut short, 80, 24);
    let all: String = (0..24).map(|y| row_text(&buf, y, 80)).collect();
    assert!(!all.contains("•••"), "two rows need no indicator");

    let mut long = menu_with(
        (0..20)
            .map(|i| Row::new(RowId::new(format!("id-{i}")), format!("App {i}")))
            .collect(),
    );
    let buf = draw(&mut long, 80, 24);
    let all: String = (0..24).map(|y| row_text(&buf, y, 80)).collect();
    assert!(all.contains("•••"), "20 rows overflow the viewport");
}

#[test]
fn filter_line_renders_at_top_for_filterable_tab_only() {
    // Filterable tab renders top margin at y = 0, `› Search` at y = 1, separator at y = 2
    let mut filterable_menu = menu_with(vec![Row::new(RowId::new("a"), "alpha")]);
    let buf = draw(&mut filterable_menu, 80, 24);
    let row0 = row_text(&buf, 0, 80);
    assert_eq!(row0.trim(), "", "top margin at y=0 is empty");
    let filter_line = row_text(&buf, 1, 80);
    assert!(
        filter_line.contains("›") && filter_line.contains("Search"),
        "filterable tab draws `› Search` at y=1: {filter_line:?}"
    );
    let sep_line = row_text(&buf, 2, 80);
    assert!(sep_line.contains("╌"), "separator at y=2: {sep_line:?}");

    // Non-filterable tab renders no `›` prompt anywhere
    let mut fixed_tab = Tab::with_rows("fixed", vec![Row::new(RowId::new("a"), "alpha")]);
    fixed_tab.filterable = false;
    let mut fixed_menu = Menu::new("fixed", vec![fixed_tab]);
    let buf = draw(&mut fixed_menu, 80, 24);
    let all: String = (0..24).map(|y| row_text(&buf, y, 80)).collect();
    assert!(
        !all.contains('›'),
        "non-filterable tab draws no `›` prompt anywhere"
    );
}

// --- character set --------------------------------------------------------

/// Upstream default set values (`char_set.rs:130-160`), asserted through the
/// rendered frame (selector, list indicator, volume bar, dropdown chrome).
#[test]
fn default_charset_renders_upstream_glyphs() {
    let row = Row::with_volume(RowId::new("id-vol"), "Volume", 0.5);
    let mut menu = menu_with(vec![row]);
    let char_set = CharSet::DEFAULT;
    assert_eq!(char_set.selector_top, "░");
    assert_eq!(char_set.selector_middle, "▒");
    assert_eq!(char_set.selector_bottom, "░");
    assert_eq!(char_set.volume_filled, "━");
    assert_eq!(char_set.volume_empty, "╌");
    assert_eq!(char_set.list_more, "•••");
    assert_eq!(char_set.dropdown_icon, "▼");
    assert_eq!(char_set.dropdown_selector, ">");
    assert_eq!(char_set.dropdown_more, "•••");
    assert_eq!(char_set.help_more, "•••");
    assert_eq!(char_set.help_border, BorderType::Rounded);
    assert_eq!(char_set.dropdown_border, BorderType::Rounded);

    let buf = draw(&mut menu, 80, 24);
    assert_eq!(row_text(&buf, 4, 80).chars().next(), Some('░'));
    assert_eq!(row_text(&buf, 5, 80).trim_end(), "▒");
    let detail = row_text(&buf, 6, 80);
    assert!(detail.contains('━') && detail.contains('╌'));
}

/// `compat` and `extracompat` values (`char_set.rs:170-235`).
#[test]
fn alternate_charsets_match_upstream() {
    let compat = CharSet::COMPAT;
    assert_eq!(compat.default_device, "◊");
    assert_eq!(compat.volume_empty, "─");
    assert_eq!(compat.meter_left_active, "┃");
    assert_eq!(compat.meter_center_left_active, "█");
    assert_eq!(compat.dropdown_icon, "▼");
    assert_eq!(compat.help_border, BorderType::Plain);

    let extra = CharSet::EXTRA_COMPAT;
    assert_eq!(extra.default_device, "*");
    assert_eq!(extra.selector_top, "-");
    assert_eq!(extra.selector_middle, "=");
    assert_eq!(extra.list_more, "~~~");
    assert_eq!(extra.meter_left_overload, "!");
    assert_eq!(extra.dropdown_icon, "\\");
    assert_eq!(extra.dropdown_border, BorderType::Plain);

    for name in CharSetName::ALL {
        assert_eq!(CharSetName::parse(name.as_str()), Some(name));
    }
    // Selection is what `--char-set` feeds the menu.
    let mut menu = menu_with(vec![Row::new(RowId::new("a"), "alpha")]);
    menu.char_set = CharSet::get(CharSetName::ExtraCompat);
    let buf = draw(&mut menu, 80, 24);
    assert_eq!(row_text(&buf, 4, 80).chars().next(), Some('-'));
}

// --- theme ----------------------------------------------------------------

/// Upstream default theme tokens (`theme.rs:110-155`).
#[test]
fn default_theme_matches_upstream() {
    let theme = Theme::DEFAULT;
    assert_eq!(theme.selector, Style::new().fg(Color::LightCyan));
    assert_eq!(theme.tab_selected, Style::new().fg(Color::LightCyan));
    assert_eq!(theme.tab_marker, Style::new().fg(Color::LightCyan));
    assert_eq!(theme.tab, Style::new(), "inactive tab: default fg");
    assert_eq!(theme.list_more, Style::new().fg(Color::DarkGray));
    assert_eq!(theme.node_target, Style::new(), "target/meta: default fg");
    assert_eq!(theme.volume_empty, Style::new().fg(Color::DarkGray));
    assert_eq!(theme.volume_filled, Style::new().fg(Color::LightBlue));
    assert_eq!(theme.meter_inactive, Style::new().fg(Color::DarkGray));
    assert_eq!(theme.meter_active, Style::new().fg(Color::LightGreen));
    assert_eq!(theme.meter_overload, Style::new().fg(Color::Red));
    assert_eq!(
        theme.dropdown_selected,
        Style::new()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::REVERSED)
    );
    assert_eq!(theme.help_border, Style::new(), "help border: default fg");
    assert_eq!(theme.help_more, Style::new().fg(Color::DarkGray));

    // A fresh menu renders with the default theme.
    assert_eq!(Menu::new("test", Vec::new()).theme, Theme::DEFAULT);
}

/// `nocolor` uses modifiers only, `plain` is entirely default
/// (`theme.rs:167-230`).
#[test]
fn alternate_themes_match_upstream() {
    let nocolor = Theme::NOCOLOR;
    assert!(nocolor.selector.add_modifier.contains(Modifier::BOLD));
    assert!(nocolor.volume_empty.add_modifier.contains(Modifier::DIM));
    assert!(nocolor
        .dropdown_selected
        .add_modifier
        .contains(Modifier::REVERSED | Modifier::BOLD));
    assert_eq!(nocolor.selector.fg, None, "nocolor never sets a foreground");

    let plain = Theme::PLAIN;
    assert_eq!(plain.selector, Style::new());
    assert_eq!(plain.volume_filled, Style::new());
    assert_eq!(plain.meter_overload, Style::new());

    for name in ThemeName::ALL {
        assert_eq!(ThemeName::parse(name.as_str()), Some(name));
    }
}

// --- detail line splits ---------------------------------------------------

/// Peaks on: `[pad 2][Fill(4) volume][pad 1][Fill(4) meter][pad 1]`, peaks
/// off: `[pad 2][Fill(9) volume][pad 1]` (`node_widget.rs:166-198`).
#[test]
fn volume_and_meter_areas_follow_upstream_splits() {
    let row = Row::with_volume(RowId::new("id-vol"), "Volume", 0.5)
        .with_peaks(RowPeaks::Stereo(1.0, 0.5));
    let mut menu = menu_with(vec![row.clone()]);
    let buf = draw(&mut menu, 80, 24);
    let y = 6; // first entry's detail line (top margin at y=0, filter at y=1, separator at y=2, indicator at y=3, header at y=4, middle at y=5, detail at y=6)
    let with_peaks = row_text(&buf, y, 80);
    let meter_cells = with_peaks.chars().filter(|c| *c == '▮').count();
    assert!(meter_cells > 0, "meters render: {with_peaks:?}");

    menu = menu_with(vec![row]);
    menu.peaks = Peaks::Off;
    let buf = draw(&mut menu, 80, 24);
    let without_peaks = row_text(&buf, y, 80);
    assert!(
        without_peaks.chars().filter(|c| *c == '━').count()
            > with_peaks.chars().filter(|c| *c == '━').count(),
        "the volume bar gets more room when meters are off"
    );
    assert!(
        without_peaks.trim_end().ends_with('╌') || without_peaks.contains('╌'),
        "volume bar fills the wider area: {without_peaks:?}"
    );
    // Both splits leave the 1-cell right padding.
    assert!(without_peaks.ends_with(' '));
}

/// The volume bar fills `volume / (max_volume_percent / 100)`
/// (`node_widget.rs:405-414`).
#[test]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn volume_bar_uses_the_configured_maximum() {
    let mut menu = menu_with(vec![Row::with_volume(RowId::new("id-vol"), "Volume", 0.75)]);
    assert!(
        (menu.max_volume_percent - flex_core::DEFAULT_MAX_VOLUME_PERCENT).abs() < f32::EPSILON,
        "default ceiling is upstream's 150%"
    );
    let buf = draw(&mut menu, 80, 24);
    let bar_w = 63; // 80-wide frame: see tests/golden.rs for the derivation
                    // Detail line (volume bar) at y=6
    let filled = row_text(&buf, 6, 80).chars().filter(|c| *c == '━').count();
    let expected = ((0.75_f32.clamp(0.0, 1.5) / 1.5) * bar_w as f32).round() as usize;
    assert_eq!(filled, expected, "75% of a 150% ceiling");

    // Raising the ceiling shrinks the fill.
    menu.max_volume_percent = 300.0;
    let buf = draw(&mut menu, 80, 24);
    let filled_high = row_text(&buf, 6, 80).chars().filter(|c| *c == '━').count();
    assert!(
        filled_high < filled,
        "a higher ceiling fills less of the bar: {filled_high} < {filled}"
    );
}

// --- dropdown -------------------------------------------------------------

/// Dropdown geometry: width = longest target + 4, height = min(5, n) + 2,
/// right-aligned in the list area, one row above the selected row
/// (`node_widget.rs:62-89`).
#[test]
fn dropdown_geometry_matches_upstream() {
    let targets = vec![
        Target::default_target(RowId::new("t0"), "Default: Speakers"),
        Target::new(RowId::new("t1"), "HDMI"),
        Target::new(RowId::new("t2"), "USB Headset"),
    ];
    let mut menu = menu_with(vec![Row::with_targets(
        RowId::new("id-vol"),
        "Volume",
        targets,
        0,
    )]);
    assert!(menu.app.open_dropdown());
    let buf = draw(&mut menu, 80, 24);

    let longest = width::str_width("Default: Speakers");
    let dropdown_w = u16::try_from(longest + 4).expect("dropdown width fits"); // 21
    let dropdown_h = 3 + 2;
    // The selected row is the first entry: header row at y=4, so the dropdown starts one row above it (y=3)
    let x = 80 - dropdown_w;
    assert_eq!(buf.cell((x, 3)).expect("corner").symbol(), "╭");
    assert_eq!(
        buf.cell((80 - 1, 3 + dropdown_h - 1))
            .expect("corner")
            .symbol(),
        "╯"
    );
    let text = row_text(&buf, 4, 80);
    assert!(text.contains("Default: Speakers"), "first item: {text:?}");
    assert!(
        text.contains("> Default: Speakers"),
        "highlight symbol `> `: {text:?}"
    );
    let selected = buf.cell((x + 2, 4)).expect("highlighted item cell");
    assert_eq!(
        selected.bg,
        Theme::DEFAULT.dropdown_selected.bg.unwrap_or(Color::Reset),
        "highlight style is applied"
    );
    assert!(selected.modifier.contains(Modifier::REVERSED));
}

/// More than five targets scroll with `•••` on the borders
/// (`dropdown_widget.rs:88-135`).
#[test]
fn dropdown_scrolls_with_indicators() {
    let targets: Vec<Target> = (0..8)
        .map(|i| Target::new(RowId::new(format!("t{i}")), format!("Target {i}")))
        .collect();
    let mut menu = menu_with(vec![Row::with_targets(
        RowId::new("id-vol"),
        "Volume",
        targets,
        0,
    )]);
    assert!(menu.app.open_dropdown());
    let buf = draw(&mut menu, 80, 24);
    let all: String = (0..12).map(|y| row_text(&buf, y, 80)).collect();
    assert!(all.contains("•••"), "scroll indicator on the border");

    // Move the highlight past the visible window: the offset follows.
    for _ in 0..7 {
        menu.app.move_dropdown(1);
    }
    let _ = draw(&mut menu, 80, 24);
    let dropdown = menu.app.dropdown().expect("dropdown open");
    assert_eq!(dropdown.selected, 7);
    assert!(
        dropdown.top > 0,
        "offset follows the highlight: {dropdown:?}"
    );
}
