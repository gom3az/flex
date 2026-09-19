//! Golden tests over `ratatui::backend::TestBackend`.
//!
//! These assert the wiremix render contract directly: node rows are 3 lines +
//! 2 spacing with the header on the first line, an empty middle line carrying
//! `▒`, and the volume/config line on the third (`node_widget.rs:95-199`);
//! the list reserves a line above and below for `•••`
//! (`object_list.rs:301-311`); the tab bar is 1 line at the bottom with
//! `[active]`/` inactive ` widths (`app.rs:757-790`).

use std::time::Instant;

use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use flex_core::{render, width, Gauge, Menu, Peaks, Row, RowId, RowPeaks, Tab, Target, Theme};
use flex_rice::providers::power;

/// First entry header row for a filterable, non-bare tab:
/// top margin at y=0, filter at y=1, separator at y=2, top `•••` indicator at y=3, first entry at y=4.
const LIST_TOP: u16 = 4;
/// List height at 80x24 for a filterable, non-bare tab:
/// `24 - (chrome_top + tab bar)` = 24 - 3 - 1 = 20.
const LIST_H_80X24: u16 = 20;
/// List area starts at this buffer row (`chrome_top` = 3 for filterable tabs).
const LIST_AREA_TOP: u16 = 3;

/// On-screen header row of provider-row `index`, from the menu's current
/// metrics and scroll offset — correct in both node modes.
fn entry_y(menu: &Menu, index: u16) -> u16 {
    let state = &menu.app.active_tab().expect("tab").state;
    let scroll = u16::try_from(state.scroll).expect("scroll fits u16");
    LIST_TOP + (index - scroll) * render::node_metrics(menu).pitch()
}

fn rich_menu() -> Menu {
    let mut apps = Tab::with_rows(
        "apps",
        vec![
            Row::confirmable(RowId::new("id-shutdown"), "Shutdown"),
            Row::new(RowId::new("id-firefox"), "Firefox"),
            Row::with_meta(RowId::new("id-volume"), "Volume", "87%"),
            Row::new(
                RowId::new("id-long"),
                "a very long label that certainly does not fit in seventy-six cells of width \
                 so the tail here must be cut off by the ellipsis",
            ),
            Row::new(RowId::new("id-emoji"), "😀 family 👨‍👩‍👧 test"),
        ],
    );
    apps.bare_rows = false;
    let mut power = Tab::with_rows(
        "power",
        vec![
            Row::confirmable(RowId::new("id-reboot"), "Reboot"),
            Row::new(RowId::new("id-logout"), "Logout"),
        ],
    );
    power.bare_rows = false;
    flex_rice::menu("test", vec![apps, power])
}

/// A one-tab menu of `n` data-less rows (compact metrics).
fn compact_menu(n: usize) -> Menu {
    let rows: Vec<Row> = (0..n)
        .map(|i| Row::new(RowId::new(format!("id-{i}")), format!("App {i}")))
        .collect();
    detail_menu(rows)
}

/// A one-tab menu whose first row has a volume line, so the tab renders
/// upstream's 3-line nodes.
fn volume_menu(n: usize) -> Menu {
    let mut rows = vec![Row::with_volume(RowId::new("id-vol-0"), "Volume 0", 0.5)];
    rows.extend((1..n).map(|i| Row::new(RowId::new(format!("id-{i}")), format!("App {i}"))));
    detail_menu(rows)
}

/// A one-tab menu whose rows carry volume/peaks/config/targets.
fn detail_menu(rows: Vec<Row>) -> Menu {
    let mut tab = Tab::with_rows("apps", rows);
    tab.bare_rows = false;
    flex_rice::menu("test", vec![tab])
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

fn col0(buf: &Buffer, y: u16) -> String {
    let cell = buf.cell((0, y)).expect("cell in frame");
    cell.symbol().to_string()
}

fn cell_at(buf: &Buffer, x: u16, y: u16) -> Cell {
    buf.cell((x, y)).expect("cell in frame").clone()
}

fn assert_full_width(buf: &Buffer) {
    let area = buf.area;
    for y in area.top()..area.bottom() {
        let mut x = area.left();
        let mut total = 0_usize;
        while x < area.right() {
            let cell = buf.cell((x, y)).expect("cell in frame");
            assert!(!cell.skip, "dangling skip cell at ({x}, {y})");
            let symbol_w = width::str_width(cell.symbol());
            total += symbol_w;
            x += u16::try_from(symbol_w.max(1)).expect("row width fits u16");
        }
        assert_eq!(
            total,
            usize::from(area.width),
            "row {y} display width must equal frame width"
        );
    }
}

fn selector_color() -> Color {
    Theme::DEFAULT
        .selector
        .fg
        .expect("selector style sets a foreground")
}

// --- selector block -------------------------------------------------------

/// The selected entry's block is exactly `░▒░` on the node's three lines, and
/// the two gap lines that follow belong to no entry.
#[test]
fn selected_selector_block_spans_exactly_three_lines() {
    let mut menu = volume_menu(6);
    menu.app.active_tab_mut().expect("tab").state.focus = 1;
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);

    let y = entry_y(&menu, 1);
    assert_eq!(col0(&buf, y), "░", "selector top on the header line");
    assert_eq!(
        cell_at(&buf, 0, y).fg,
        selector_color(),
        "selector uses theme.selector"
    );
    assert_eq!(col0(&buf, y + 1), "▒", "selector middle on the empty line");
    assert_eq!(col0(&buf, y + 2), "░", "selector bottom on the detail line");
    for gap in [y + 3, y + 4] {
        assert_eq!(col0(&buf, gap), " ", "gap line {gap} carries no selector");
        assert_eq!(row_text(&buf, gap, 80).trim(), "", "gap line {gap} blank");
    }

    // The middle line of a node is empty: only the selector shows.
    assert_eq!(
        row_text(&buf, y + 1, 80).trim_end(),
        "▒",
        "the node's middle line is empty"
    );

    // Exactly one `░▒░` block in the selector column.
    let marks: Vec<(u16, String)> = (LIST_AREA_TOP..=LIST_H_80X24)
        .map(|y| (y, col0(&buf, y)))
        .filter(|(_, symbol)| symbol != " ")
        .collect();
    assert_eq!(
        marks,
        vec![
            (y, "░".to_string()),
            (y + 1, "▒".to_string()),
            (y + 2, "░".to_string()),
        ],
        "column 0 holds one ░▒░ block (entry 1) and nothing else"
    );
}

/// Data-less tabs render flex's compact node: one header line per entry, one
/// blank gap row, and a single `░` selector glyph.
#[test]
fn compact_nodes_when_no_row_has_detail_data() {
    let mut menu = compact_menu(6);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    assert_eq!(
        render::node_metrics(&menu),
        render::NodeMetrics::COMPACT,
        "no row has volume/peaks/config"
    );
    for index in 0..5u16 {
        let y = entry_y(&menu, index);
        assert!(
            row_text(&buf, y, 80).contains(&format!("App {index}")),
            "entry {index} header at y={y}"
        );
        assert_eq!(
            row_text(&buf, y + 1, 80).trim(),
            "",
            "y={} is the blank gap row",
            y + 1
        );
        assert_eq!(y, LIST_TOP + index * 2, "compact pitch is 2");
    }
    assert_eq!(col0(&buf, entry_y(&menu, 0)), "░", "single-glyph selector");
    assert_eq!(col0(&buf, entry_y(&menu, 0) + 1), " ");
    let marks: Vec<(u16, String)> = (LIST_AREA_TOP..=LIST_H_80X24)
        .map(|y| (y, col0(&buf, y)))
        .filter(|(_, symbol)| symbol != " ")
        .collect();
    assert_eq!(marks, vec![(entry_y(&menu, 0), "░".to_string())]);
}

/// Unselected rows draw nothing in the selector column.
#[test]
fn unselected_rows_have_no_selector() {
    let mut menu = volume_menu(6);
    menu.app.active_tab_mut().expect("tab").state.focus = 2;
    let buf = draw(&mut menu, 80, 24);
    for index in [0, 1, 3] {
        let y = entry_y(&menu, index);
        for line in 0..render::NODE_HEIGHT {
            assert_eq!(
                col0(&buf, y + line),
                " ",
                "entry {index} line {line} has no selector"
            );
        }
    }
}

// --- header layout --------------------------------------------------------

/// Upstream header: col 2 marker, col 4 title (`node_widget.rs:281-318`).
#[test]
fn header_reserves_the_default_marker_column() {
    let rows = vec![
        Row::new(RowId::new("id-plain"), "Plain"),
        Row::new(RowId::new("id-default"), "Default").default_marked(),
    ];
    let mut menu = detail_menu(rows);
    menu.app.active_tab_mut().expect("tab").state.focus = 1;
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);

    let plain = entry_y(&menu, 0);
    let marked = entry_y(&menu, 1);
    assert_eq!(cell_at(&buf, 2, plain).symbol(), " ", "no marker");
    assert_eq!(
        cell_at(&buf, 4, plain).symbol(),
        "P",
        "titles start at column 4"
    );
    assert_eq!(cell_at(&buf, 2, marked).symbol(), "◇", "default marker");
    assert_eq!(
        cell_at(&buf, 2, marked).fg,
        Theme::DEFAULT.default_device.fg.unwrap_or(Color::Reset),
        "marker uses theme.default_device"
    );
    assert_eq!(cell_at(&buf, 4, marked).symbol(), "D", "title after marker");
}

/// The right column is right-aligned with one cell of margin and keeps the
/// terminal default foreground (upstream `node_target`).
#[test]
fn meta_is_right_aligned_in_default_style() {
    let mut menu = rich_menu();
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let y = entry_y(&menu, 2);
    let text = row_text(&buf, y, 80);
    assert!(
        text.ends_with("87% "),
        "meta right-aligned + 1 pad: {text:?}"
    );
    let meta_cell = cell_at(&buf, 76, y);
    assert_eq!(meta_cell.symbol(), "8");
    assert_eq!(
        meta_cell.fg,
        Theme::DEFAULT.node_target.fg.unwrap_or(Color::Reset)
    );
    assert_ne!(
        meta_cell.fg,
        Color::DarkGray,
        "meta is not dimmed (upstream node_target is default)"
    );
}

/// A target whose `is_default` is set gets the `◇ ` prefix
/// (upstream `target_line`, `node_widget.rs:258-273`).
#[test]
fn default_target_is_prefix_marked() {
    let targets = vec![
        Target::default_target(RowId::new("t-default"), "Default: Speakers"),
        Target::new(RowId::new("t-hdmi"), "HDMI"),
    ];
    let mut menu = detail_menu(vec![Row::with_targets(
        RowId::new("id-vol"),
        "Volume",
        targets,
        0,
    )]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let text = row_text(&buf, entry_y(&menu, 0), 80);
    assert!(
        text.ends_with("◇ Default: Speakers "),
        "target line: {text:?}"
    );

    menu.app.active_tab_mut().expect("tab").state.focus = 0;
    menu.app.active_tab_mut().expect("tab").rows[0].target_index = 1;
    let buf = draw(&mut menu, 80, 24);
    let text = row_text(&buf, entry_y(&menu, 0), 80);
    assert!(text.ends_with("HDMI "), "non-default target: {text:?}");
    assert!(!text.contains('◇'), "no marker for a plain target");
}

/// A label that does not fit gets upstream's 3-cell `...` area
/// (`node_widget.rs:323-342`) — not a single `…`.
#[test]
fn long_label_gets_three_dot_ellipsis_area() {
    let mut menu = rich_menu();
    menu.app.active_tab_mut().expect("tab").state.focus = 3;
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    // Nine compact entries fit at 80x24, so nothing scrolls.
    assert_eq!(menu.app.active_tab().expect("tab").state.scroll, 0);
    let text = row_text(&buf, entry_y(&menu, 3), 80);
    assert!(text.contains("..."), "3-cell ellipsis: {text:?}");
    assert!(!text.contains('…'), "upstream uses three ASCII dots");
    assert!(!text.contains("ellipsis"), "tail is cut: {text:?}");
}

/// Emoji labels keep the frame's exact width.
#[test]
fn emoji_row_keeps_exact_width_at_80x24() {
    let mut menu = rich_menu();
    menu.app.active_tab_mut().expect("tab").state.focus = 4;
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.scroll,
        0,
        "all five compact entries fit"
    );
    let text = row_text(&buf, entry_y(&menu, 4), 80);
    assert!(text.contains('😀'), "emoji survives: {text:?}");
}

/// An oversized meta is clipped by the layout instead of being dropped.
#[test]
fn oversized_meta_is_clipped_not_dropped() {
    let mut menu = detail_menu(vec![Row::with_meta(
        RowId::new("id-x"),
        "hi",
        "x".repeat(77).as_str(),
    )]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    // Upstream's `Min(1)` title area collapses and the target keeps its
    // width, so the meta's tail stays pinned to the right edge.
    let text = row_text(&buf, entry_y(&menu, 0), 80);
    assert!(
        width::str_width(&text) <= 80,
        "row stays inside the frame: {text:?}"
    );
    let tail = row_text(&buf, entry_y(&menu, 0), 80);
    assert!(
        tail.ends_with("xx "),
        "meta tail at the right edge: {tail:?}"
    );
    assert_eq!(
        col0(&buf, entry_y(&menu, 0)),
        "░",
        "selector survives the squeeze"
    );
}

// --- detail line ----------------------------------------------------------

/// The volume line lives on the node's THIRD line with the 5-cell
/// right-aligned percentage and the bar starting after it
/// (`node_widget.rs:381-419`).
#[test]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn volume_line_is_the_detail_line() {
    let mut menu = detail_menu(vec![Row::with_volume(RowId::new("id-vol"), "Volume", 0.85)]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);

    let y = entry_y(&menu, 0);
    // Middle line: empty.
    assert_eq!(row_text(&buf, y + 1, 80).trim_end(), "▒");

    // 5-cell right-aligned percentage in the label area at cols 3..7.
    let label = row_text(&buf, y + 2, 80);
    assert!(label.starts_with("░    85%"), "label line: {label:?}");
    assert_eq!(
        cell_at(&buf, 5, y + 2).symbol(),
        "8",
        "percentage starts at col 5"
    );
    assert_eq!(
        cell_at(&buf, 7, y + 2).symbol(),
        "%",
        "percentage ends at col 7"
    );

    // Bar geometry: node area 1..80, [pad 2][Fill(9)][Fill(1)] → volume area
    // starts at col 3, label 3..8, spacing col 8, bar from col 9.
    let bar_x = 9;
    let bar_w = 63; // volume area 69 minus 5-cell label and 1-cell spacing
    assert_eq!(
        cell_at(&buf, bar_x, y + 2).symbol(),
        "━",
        "bar starts at col 9"
    );
    assert_eq!(
        cell_at(&buf, bar_x, y + 2).fg,
        Theme::DEFAULT
            .volume_filled
            .fg
            .expect("volume_filled sets a fg")
    );
    // 0.85 / 1.5 * 63 = 35.7 → 36 filled cells.
    let filled = ((0.85_f32.clamp(0.0, 1.5) / 1.5) * f32::from(bar_w)).round() as u16;
    assert_eq!(filled, 36);
    assert_eq!(cell_at(&buf, bar_x + filled - 1, y + 2).symbol(), "━");
    assert_eq!(cell_at(&buf, bar_x + filled, y + 2).symbol(), "╌");
    assert_eq!(
        cell_at(&buf, bar_x + filled, y + 2).fg,
        Theme::DEFAULT
            .volume_empty
            .fg
            .expect("volume_empty sets a fg")
    );
    assert_eq!(cell_at(&buf, bar_x + bar_w - 1, y + 2).symbol(), "╌");
    assert!(
        row_text(&buf, y + 2, 80).ends_with(' '),
        "1-cell right padding after the bar"
    );
}

/// Rows without volume data leave the detail line empty — the old decorative
/// bar is gone.
#[test]
fn rows_without_volume_have_no_bar() {
    let mut menu = rich_menu();
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let all: String = (LIST_TOP..LIST_AREA_TOP + LIST_H_80X24)
        .map(|y| row_text(&buf, y, 80))
        .collect();
    assert!(
        !all.contains('━') && !all.contains('╌'),
        "no volume bar for rows without volume data"
    );
}

/// `muted` replaces the percentage in the label area (`node_widget.rs:421-423`).
#[test]
fn muted_shows_the_word_muted() {
    let score = Row::with_volume(RowId::new("id-vol"), "Volume", 0.5).muted();
    let mut menu = detail_menu(vec![score]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let y = entry_y(&menu, 0);
    let line = row_text(&buf, y + 2, 80);
    assert!(line.starts_with("░  muted"), "muted label: {line:?}");
    assert!(!line.contains('%'), "percentage is replaced: {line:?}");
    assert!(line.contains('━'), "bar still renders while muted");
}

/// Device-style rows draw `▼ config` on the detail line
/// (`device_widget.rs:141-153`).
#[test]
fn config_line_renders_dropdown_icon() {
    let mut menu = detail_menu(vec![Row::with_config(
        RowId::new("id-dev"),
        "Speakers",
        "Analog Stereo",
    )]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let y = entry_y(&menu, 0);
    let line = row_text(&buf, y + 2, 80);
    assert!(
        line.starts_with("░    ▼ Analog Stereo"),
        "config line: {line:?}"
    );
    assert_eq!(cell_at(&buf, 5, y + 2).symbol(), "▼", "icon at col 5");
}

/// Peak meters render beside the volume bar; `Peaks::Off` removes them.
#[test]
fn peak_meters_render_and_honor_peaks_off() {
    let row = Row::with_volume(RowId::new("id-vol"), "Volume", 0.5)
        .with_peaks(RowPeaks::Stereo(1.0, 0.5));
    let mut menu = detail_menu(vec![row.clone()]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let line = row_text(&buf, entry_y(&menu, 0) + 2, 80);
    assert!(line.contains('▮'), "meter glyphs render: {line:?}");

    menu = detail_menu(vec![row]);
    menu.peaks = Peaks::Off;
    let buf = draw(&mut menu, 80, 24);
    let line = row_text(&buf, entry_y(&menu, 0) + 2, 80);
    assert!(!line.contains('▮'), "Peaks::Off hides meters: {line:?}");
    assert!(line.contains('━'), "volume bar stays");
}

// --- list indicators ------------------------------------------------------

/// `•••` in the reserved rows, with upstream's bottom suppression rule
/// (`object_list.rs:478-517`).
#[test]
fn list_indicators_mark_hidden_rows() {
    let rows: Vec<Row> = (0..12)
        .map(|i| Row::new(RowId::new(format!("id-{i}")), format!("App {i}")))
        .collect();
    let mut menu = detail_menu(rows);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);

    // At the top: nothing above, `•••` below.
    assert_eq!(
        row_text(&buf, LIST_AREA_TOP, 80).trim(),
        "",
        "no top indicator at scroll 0"
    );
    let footer = row_text(&buf, LIST_AREA_TOP + LIST_H_80X24 - 1, 80);
    assert!(footer.contains("•••"), "bottom indicator: {footer:?}");
    let trimmed = footer.trim_end();
    let left_pad = width::str_width(&trimmed[..trimmed.find('•').expect("bullet")]);
    let right_pad = 80 - left_pad - 3;
    assert_eq!(left_pad, 38, "indicator centered in 80 cells: {footer:?}");
    assert!(
        (37..=39).contains(&right_pad),
        "right padding {right_pad} is centered too: {footer:?}"
    );
    let y = LIST_AREA_TOP + LIST_H_80X24 - 1;
    let cell = cell_at(&buf, 40, y);
    assert_eq!(
        cell.fg,
        Theme::DEFAULT.list_more.fg.expect("list_more sets a fg")
    );

    // Scrolled to the end: top indicator only.
    let last = 11;
    menu.app.active_tab_mut().expect("tab").state.focus = last;
    let buf = draw(&mut menu, 80, 24);
    assert!(
        row_text(&buf, LIST_AREA_TOP, 80).contains("•••"),
        "top indicator once scrolled"
    );
    assert_eq!(
        row_text(&buf, LIST_AREA_TOP + LIST_H_80X24 - 1, 80).trim(),
        "",
        "no bottom indicator at the end"
    );
}

/// The bottom indicator is suppressed when the last entry is only partially
/// rendered but shows everything that matters.
#[test]
fn bottom_indicator_is_suppressed_for_a_partial_last_row() {
    // 12 compact rows: with odd rows_area height 19 (at terminal height 25: 25 - 4 = 21, 21 - 2 = 19),
    // focus 10 → scroll 2, so only row 11 is below the viewport and its single header line fits (19 % 2 == 1),
    // which suppresses the indicator (upstream `object_list.rs:497-511`).
    let mut menu = compact_menu(12);
    menu.app.active_tab_mut().expect("tab").state.focus = 10;
    let buf = draw(&mut menu, 80, 25);
    let scroll = menu.app.active_tab().expect("tab").state.scroll;
    assert_eq!(scroll, 2);
    // Footer row is at LIST_AREA_TOP + list_h - 1 = 3 + 21 - 1 = 23
    assert_eq!(
        row_text(&buf, 23, 80).trim(),
        "",
        "the partially rendered last row hides the indicator"
    );
    assert!(
        row_text(&buf, LIST_AREA_TOP, 80).contains("•••"),
        "rows above the viewport still show the top indicator"
    );
}

// --- tab bar --------------------------------------------------------------

/// Upstream tab widths: `name + 2`, `[active]`, ` inactive ` with the
/// terminal default foreground and no extra separator.
#[test]
fn tab_bar_matches_upstream_widths_and_styles() {
    let mut menu = rich_menu();
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let y = 23;
    let text = row_text(&buf, y, 80);
    assert!(
        text.starts_with("[apps] power "),
        "tab bar layout: {text:?}"
    );
    assert_eq!(cell_at(&buf, 0, y).symbol(), "[");
    assert_eq!(cell_at(&buf, 5, y).symbol(), "]");
    assert_eq!(cell_at(&buf, 6, y).symbol(), " ", "inactive tab left pad");
    assert_eq!(cell_at(&buf, 7, y).symbol(), "p");
    assert_eq!(
        cell_at(&buf, 2, y).fg,
        Theme::DEFAULT
            .tab_selected
            .fg
            .expect("tab_selected sets a fg")
    );
    assert_eq!(
        cell_at(&buf, 7, y).fg,
        Color::Reset,
        "inactive tabs keep the default foreground"
    );
    assert!(
        !cell_at(&buf, 2, y).modifier.contains(Modifier::BOLD),
        "upstream does not bold the active tab"
    );
}

// --- help overlay ---------------------------------------------------------

/// The overlay has no title, a default-colored border, and upstream geometry
/// (`app.rs:811-840`, `help.rs:46-53`).
#[test]
fn help_overlay_geometry_and_chrome() {
    let mut menu = rich_menu();
    menu.app.help_open = true;
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);

    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(
        !all.contains("flex help"),
        "upstream has no help title: {all:?}"
    );
    assert!(
        all.contains(render::HELP_LINES[0]),
        "help body renders: {all:?}"
    );

    // Border rows: `╭`/`╰` corners in the default style, centered inside the
    // list area (rows 0..21).
    let top = (0..24)
        .find(|y| row_text(&buf, *y, 80).contains('╭'))
        .expect("top border");
    let bottom = (0..24)
        .find(|y| row_text(&buf, *y, 80).contains('╰'))
        .expect("bottom border");
    assert!(bottom <= LIST_H_80X24, "overlay stays inside the list area");
    let corner_x = row_text(&buf, top, 80).find('╭').expect("corner column");
    #[allow(clippy::cast_possible_truncation)]
    let corner_x = corner_x as u16;
    assert_eq!(
        cell_at(&buf, corner_x, top).fg,
        Theme::DEFAULT.help_border.fg.unwrap_or(Color::Reset),
        "help border keeps the default foreground"
    );
    assert!(
        corner_x > 0 && corner_x < 80,
        "overlay is horizontally centered"
    );
    // 90% cap: at most 21 rows of list area → at most 18 rows of overlay.
    assert!(bottom - top < 19, "overlay height is capped");
}

/// `•••` on the help border when the text does not fit, plus scrolling.
#[test]
fn help_scrolls_with_indicators() {
    let mut menu = rich_menu();
    menu.app.help_open = true;
    // Force a small overlay by shrinking the frame: 10 rows total, chrome 3,
    // list area 6 → 90% of 6 = 5 rows, so only 3 help lines fit.
    let buf = draw(&mut menu, 80, 10);
    let mut all = String::new();
    for y in 0..10 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(all.contains("•••"), "help_more indicator: {all:?}");

    let before = all.clone();
    menu.app.help_scroll = 1;
    let buf = draw(&mut menu, 80, 10);
    let mut after = String::new();
    for y in 0..10 {
        after.push_str(&row_text(&buf, y, 80));
    }
    assert_ne!(before, after, "scrolled help renders different lines");
}

// --- flex-extension chrome ------------------------------------------------

#[test]
fn armed_confirmable_row_at_80x24() {
    let mut menu = rich_menu();
    menu.app
        .active_tab_mut()
        .expect("tab")
        .state
        .arm(Instant::now());
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let y = entry_y(&menu, 0);
    let text = row_text(&buf, y, 80);
    assert!(text.contains("-- confirm"), "armed confirm text: {text:?}");
    let mut all = String::new();
    for row in 0..24 {
        all.push_str(&row_text(&buf, row, 80));
    }
    assert!(!all.contains("Enter again"), "bottom hint line is removed");
}

#[test]
fn gauge_online_offline_and_mute_at_80x24() {
    let mut menu = rich_menu();
    menu.gauge = Some(Gauge::new("Volume", 65));
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let gauge_line = row_text(&buf, 22, 80);
    assert!(gauge_line.contains("65%"), "gauge value: {gauge_line:?}");

    menu.gauge.as_mut().expect("gauge").toggle_mute();
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let text = row_text(&buf, 22, 80);
    assert!(text.contains("muted"), "mute label: {text:?}");
    assert!(
        !text.contains("65%"),
        "value replaced while muted: {text:?}"
    );

    menu.gauge.as_mut().expect("gauge").online = false;
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    assert!(
        row_text(&buf, 22, 80).contains("— offline"),
        "offline label: {:?}",
        row_text(&buf, 22, 80)
    );
}

#[test]
fn empty_state_is_centered_in_the_list() {
    let mut menu = rich_menu();
    menu.app.active_tab_mut().expect("tab").state.filter = "zzz-no-match".to_string();
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let mut found = None;
    for y in LIST_AREA_TOP..=LIST_H_80X24 {
        if row_text(&buf, y, 80).contains("— no matches —") {
            found = Some(y);
        }
    }
    let y = found.expect("empty state renders");
    assert_eq!(
        y,
        LIST_AREA_TOP + LIST_H_80X24 / 2,
        "empty state sits mid-list"
    );
}

#[test]
fn bare_rows_mode_has_no_chrome_and_no_indicators() {
    let mut tab = Tab::with_rows("launch", vec![Row::new(RowId::new("id-a"), "alpha")]);
    tab.bare_rows = true;
    tab.filterable = false;
    let mut menu = flex_rice::menu("launch", vec![tab]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(
        !all.contains('›'),
        "bare non-filterable mode has no filter line"
    );
    assert!(!all.contains("navigate"), "bare mode has no hints line");
    assert!(!all.contains("•••"), "one row needs no scroll indicator");
    assert!(
        row_text(&buf, 1, 80).starts_with('░'),
        "row 0 starts with the selector"
    );
}

#[test]
fn bare_filterable_tab_has_top_filter_chrome() {
    let mut tab = Tab::with_rows("launch", vec![Row::new(RowId::new("id-a"), "alpha")]);
    tab.bare_rows = true;
    tab.filterable = true;
    let mut menu = flex_rice::menu("launch", vec![tab]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let r0 = row_text(&buf, 0, 80);
    assert_eq!(r0.trim(), "", "row 0 is top margin");
    let r1 = row_text(&buf, 1, 80);
    assert!(r1.contains('›'), "top filter line at y=1: {r1:?}");
    assert!(r1.contains("Search"), "search placeholder at y=1: {r1:?}");
    assert_eq!(buf.cell((2, 1)).expect("glyph").symbol(), "›");
    assert_eq!(buf.cell((4, 1)).expect("search start").symbol(), "S");
    let r2 = row_text(&buf, 2, 80);
    assert!(r2.contains('╌'), "separator at y=2: {r2:?}");
    assert!(
        row_text(&buf, 4, 80).starts_with('░'),
        "first list row starts at y=4"
    );
}

#[test]
fn power_tab_has_launch_style_chrome_at_80x24() {
    let mut menu = flex_rice::menu(power::PROVIDER, vec![power::power_tab()]);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(
        row_text(&buf, 1, 80).starts_with('░'),
        "row 0 starts with the selector"
    );
    assert!(!all.contains('›'), "no filter line");
    assert!(!all.contains("navigate"), "no hints line");
    assert!(!all.contains("hyprlock"), "no metas in bare mode");
    assert!(all.contains("Lock Screen"), "rows still render");
    assert!(all.contains("Reboot"), "Reboot row renders");
}

#[test]
fn full_menu_at_125x30() {
    let mut menu = rich_menu();
    menu.app.active_tab_mut().expect("tab").state.focus = 2;
    menu.gauge = Some(Gauge::new("Volume", 87));
    menu.app.help_open = true;
    let buf = draw(&mut menu, 125, 30);
    assert_full_width(&buf);
    assert_eq!(buf.area.width, 125);
    assert_eq!(buf.area.height, 30);
    let mut all = String::new();
    for y in 0..30 {
        all.push_str(&row_text(&buf, y, 125));
    }
    for token in ["apps", "power", "Volume", "87%"] {
        assert!(all.contains(token), "125x30 contains {token:?}");
    }
    assert!(all.contains(render::HELP_LINES[0]), "help body renders");
    // Gauge at y = H - 1 - TAB_BAR_HEIGHT = 30 - 1 - 1 = 28
    assert!(row_text(&buf, 28, 125).contains("87%"), "gauge at H-1-tab");
    // Filter at y = 1
    assert!(row_text(&buf, 1, 125).contains('›'), "filter at y=1");
}

#[test]
fn degenerate_frames_never_panic_and_stay_exact() {
    let mut empty = flex_rice::menu("test", vec![]);
    let buf = draw(&mut empty, 20, 6);
    assert_full_width(&buf);
    let mut menu = rich_menu();
    menu.app.active_tab_mut().expect("tab").state.filter = "firefox".to_string();
    let buf = draw(&mut menu, 20, 6);
    assert_full_width(&buf);
    let buf = draw(&mut menu, 10, 3);
    assert_full_width(&buf);
    let buf = draw(&mut menu, 1, 1);
    assert_full_width(&buf);
}

#[test]
fn charset_and_theme_switch_the_rendering() {
    use flex_core::{CharSet, CharSetName, ThemeName};

    let mut menu = detail_menu(vec![Row::with_volume(RowId::new("id-vol"), "Volume", 1.0)]);
    menu.char_set = CharSet::get(CharSetName::ExtraCompat);
    menu.theme = Theme::get(ThemeName::NoColor);
    let buf = draw(&mut menu, 80, 24);
    assert_full_width(&buf);
    let y = entry_y(&menu, 0);
    assert_eq!(col0(&buf, y), "-", "extracompat selector top");
    assert_eq!(col0(&buf, y + 1), "=", "extracompat selector middle");
    let line = row_text(&buf, y + 2, 80);
    assert!(
        line.starts_with("-   100% ===="),
        "extracompat label + bar: {line:?}"
    );
    assert!(
        line.trim_end().ends_with('-'),
        "extracompat empty bar: {line:?}"
    );
    assert_eq!(
        cell_at(&buf, 0, y).fg,
        Color::Reset,
        "nocolor theme leaves colors to the terminal"
    );
    assert!(
        cell_at(&buf, 0, y).modifier.contains(Modifier::BOLD),
        "nocolor selector uses BOLD"
    );
}
