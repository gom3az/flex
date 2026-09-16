//! Deterministic renderer over `ratatui` (zero raw ANSI).
//!
//! Geometry and element order are a direct port of wiremix (upstream commit
//! `cbdc90f`); every section cites the upstream line it mirrors, and
//! `Docs/design_system.md` records the mapping. Upstream's per-element layout
//! math is reproduced with `ratatui::layout::Layout`, so widths, alignment
//! and clipping match instead of being re-derived.
//!
//! ```text
//! Main layout (flex adds the chrome rows; upstream is list + tab bar):
//!   ┌─────────────────────────────────────┐
//!   │  •••            ← scroll indicator  │  1 line, list_more (object_list.rs:478-517)
//!   │  ░ ◇ Title                Target    │  node rows, 3 lines + 2 spacing
//!   │  ▒                                  │  (node_widget.rs:53-60)
//!   │  ░   85% ━━━━━━╌╌╌╌ ▮▮▮▮           │
//!   │  •••            ← scroll indicator  │
//!   ├─────────────────────────────────────┤
//!   │  [Playback] Recording               │  tab bar — 1 line, bottom (app.rs:747-790)
//!   └─────────────────────────────────────┘
//! ```
//!
//! Node row internals (upstream `NodeWidget::render`, `node_widget.rs:95-199`):
//!
//! - col 0: selector column, `░` / `▒` / `░` on the node's three lines
//!   (`node_widget.rs:213-236`)
//! - header line: `default_device` marker (col 2), blank, title (col 4),
//!   right-aligned target (`node_widget.rs:281-318`)
//! - middle line: empty — only the selector's `▒` shows
//! - detail line: config line (`▼ profile`, `device_widget.rs:141-153`) *or*
//!   volume label + bar and peak meters (`node_widget.rs:166-198`)
//!
//! Flex extensions (no upstream counterpart, documented in
//! `Docs/design_system.md` §10): the gauge / filter / hint chrome rows, the
//! `— no matches —` empty state, the `-- confirm` suffix, offline dimming and
//! label sanitizing.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, StatefulWidget, Widget};
use ratatui::Frame;

use crate::charset::CharSet;
use crate::preview;
use crate::theme::Theme;
use crate::width;
use crate::{meter, Menu, Peaks, Row};

/// Upstream `NodeWidget::height()` (`node_widget.rs:53-55`).
pub const NODE_HEIGHT: u16 = 3;
/// Upstream `NodeWidget::spacing()` (`node_widget.rs:58-60`).
pub const NODE_SPACING: u16 = 2;
/// Compact node height (flex extension): a node whose row has no detail data
/// shrinks to its header line.
pub const COMPACT_NODE_HEIGHT: u16 = 1;
/// Compact node spacing (flex extension): one blank line between items.
pub const COMPACT_NODE_SPACING: u16 = 1;

/// Geometry of one node in the list (flex extension, §10 of the design doc).
///
/// Upstream always renders 3-line nodes because every `PipeWire` node carries
/// volumes (`node_widget.rs:53-60`). Flex's providers mostly carry nothing but
/// a label, so a data-less tab would render one inked line per five rows. The
/// metrics therefore follow the data: [`NodeMetrics::UPSTREAM`] when any row
/// in the tab has a detail line to draw (volume, enabled peaks, or a config
/// line), [`NodeMetrics::COMPACT`] otherwise. Both modes run upstream's own
/// widget code — a 1-row node area simply degenerates the selector to its top
/// glyph, exactly as upstream's `SelectorWidget` would (`node_widget.rs:217-224`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeMetrics {
    /// Node height in lines.
    pub height: u16,
    /// Blank lines between nodes.
    pub spacing: u16,
}

impl NodeMetrics {
    /// Upstream's 3 lines + 2 spacing (`node_widget.rs:53-60`).
    pub const UPSTREAM: Self = Self {
        height: NODE_HEIGHT,
        spacing: NODE_SPACING,
    };
    /// Flex's compact 1 line + 1 spacing for data-less rows.
    pub const COMPACT: Self = Self {
        height: COMPACT_NODE_HEIGHT,
        spacing: COMPACT_NODE_SPACING,
    };

    /// Rows per node, including the gap.
    #[must_use]
    pub const fn pitch(self) -> u16 {
        self.height + self.spacing
    }

    /// Metrics for a tab: upstream when any row draws a detail line.
    ///
    /// `peaks` is the menu's peak mode: rows whose peaks are hidden
    /// ([`Peaks::Off`]) have no detail line either.
    #[must_use]
    pub fn for_rows(rows: &[Row], peaks: Peaks) -> Self {
        let has_detail = rows.iter().any(|row| {
            row.volume.is_some()
                || row.config.is_some()
                || (row.peaks.is_some() && peaks != Peaks::Off)
        });
        if has_detail {
            Self::UPSTREAM
        } else {
            Self::COMPACT
        }
    }
}
/// Fixed row of tab titles at the bottom (`app.rs:747-756`).
const TAB_BAR_HEIGHT: u16 = 1;
/// Rows reserved above and below the list for the `list_more` indicator
/// (`object_list.rs:301-311`).
pub const LIST_INDICATOR_ROWS: u16 = 2;
/// Upstream `max_visible_items` in `dropdown_area` (`node_widget.rs:69`).
const DROPDOWN_MAX_ITEMS: usize = 5;

pub const EMPTY_STATE: &str = "— no matches —";
pub const OFFLINE_STATE: &str = "— offline";
/// Flex extension: the filter prompt glyph (`›`).
pub const FILTER_PROMPT: char = '›';
/// Flex extension: the count separator on the filter line.
pub const HELP_LINES: &[&str] = &[
    "j/k move focus      h/l switch tab",
    "Tab / Shift-Tab cycle tabs",
    "1-9 switch tab (empty filter)",
    "Alt-1..9 always switch tab",
    "type to filter      Backspace del",
    "Ctrl-u clear        Ctrl-w kill word",
    "Ctrl-n / Ctrl-p move",
    "Ctrl-o flip NORMAL / NAVIGATE",
    "m mark (NAVIGATE)   Del confirm",
    "Enter select (dropdown rows: open)",
    "Enter in dropdown picks the target",
    "F1 mute gauge       ? toggle help",
    "q quit (NORMAL + empty filter)",
    "Esc clear / close / disarm / quit",
];

/// Vertical frame split: the list height plus the chrome flags the renderer
/// branches on (`run` needs the same numbers to place the image pane).
struct FrameLayout {
    /// Rows owned by the list (and therefore by the preview pane).
    list_h: u16,
    /// Bare-rows tab: no chrome at all, the list owns the frame.
    bare: bool,
    /// Tab shows a filter line (and the caret).
    filterable: bool,
    /// Gauge widget occupies the last rows above the tab bar.
    gauge_on: bool,
}

fn frame_layout(area: Rect, menu: &Menu) -> FrameLayout {
    let bare = menu.app.active_tab().is_some_and(|tab| tab.bare_rows);
    let filterable = menu.app.active_tab().is_some_and(|tab| tab.filterable);
    let gauge_on = !bare && menu.gauge.is_some();

    // Bare tabs (flex extension) draw no chrome at all, so the list owns the
    // whole frame; otherwise it stops above the gauge/filter/hints rows and
    // the tab bar.
    let chrome_bottom: u16 = if bare {
        0
    } else if gauge_on {
        3 + TAB_BAR_HEIGHT
    } else if filterable {
        2 + TAB_BAR_HEIGHT
    } else {
        1 + TAB_BAR_HEIGHT
    };

    let list_h = usize::from(area.height).saturating_sub(usize::from(chrome_bottom));
    FrameLayout {
        list_h: u16::try_from(list_h).unwrap_or(u16::MAX),
        bare,
        filterable,
        gauge_on,
    }
}

/// Image preview pane this frame reserves, if any.
///
/// `run` calls this after drawing (the geometry is identical to the frame the
/// renderer just laid out) and hands the result to [`preview::Preview`], which
/// paints the image out-of-band: the renderer itself emits no raw escapes, so
/// goldens stay pixel-exact.
#[must_use]
pub fn preview_area(area: Rect, menu: &Menu) -> Option<Rect> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    preview::pane(area, frame_layout(area, menu).list_h, menu.preview)
}

pub fn render(frame: &mut Frame, menu: &mut Menu) {
    let area = frame.area();
    let width_cells = usize::from(area.width);
    let height = usize::from(area.height);

    if width_cells == 0 || height == 0 {
        return;
    }

    let FrameLayout {
        list_h,
        bare,
        filterable,
        gauge_on,
    } = frame_layout(area, menu);

    // Image preview pane: reserved on the right of the list area only (the
    // chrome below stays full width). `None` on frames too small for it, and
    // whenever the provider did not ask for one — then the list keeps the
    // whole width, exactly as before.
    let pane = preview::pane(area, list_h, menu.preview);
    let list_w = match pane {
        // One gutter column between the rows and the image.
        Some(pane) => pane.x.saturating_sub(area.x).saturating_sub(1),
        None => area.width,
    };
    let list_area = Rect::new(area.x, area.y, list_w, list_h);

    // Viewport sizing (upstream `ObjectList::visible_count`,
    // `object_list.rs:246-256`): entry units, not visual rows.
    let metrics = node_metrics(menu);
    let list_rows = list_h.saturating_sub(LIST_INDICATOR_ROWS);
    let entries_visible = usize::from(list_rows) / usize::from(metrics.pitch());
    menu.app.ensure_visible(entries_visible);

    let buf = frame.buffer_mut();
    fill_bg(buf, area.x, area.y, width_cells, height);

    if bare {
        draw_list(buf, list_area, menu);
    } else {
        draw_list(buf, list_area, menu);
        if gauge_on && height >= 4 {
            draw_gauge(
                buf,
                area.x,
                area.y + area.height - 3 - TAB_BAR_HEIGHT,
                width_cells,
                menu,
            );
        }
        if height >= 2 {
            if filterable {
                draw_filter(
                    buf,
                    area.x,
                    area.y + area.height - 2 - TAB_BAR_HEIGHT,
                    width_cells,
                    menu,
                );
            }
            draw_hints(
                buf,
                area.x,
                area.y + area.height - 1 - TAB_BAR_HEIGHT,
                width_cells,
                menu,
            );
        }
        draw_tab_bar(
            buf,
            area.x,
            area.y + area.height - TAB_BAR_HEIGHT,
            width_cells,
            menu,
        );
    }

    draw_dropdown(buf, list_area, menu);

    if menu.app.help_open {
        draw_help(buf, list_area, menu);
    }

    place_cursor(frame, area, menu, bare || !filterable);
}

fn fill_bg(buf: &mut Buffer, ox: u16, oy: u16, width: usize, height: usize) {
    let style = Theme::background();
    for dy in 0..height {
        for dx in 0..width {
            #[allow(clippy::cast_possible_truncation)]
            let (x, y) = (ox + dx as u16, oy + dy as u16);
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_style(style);
                cell.set_skip(false);
            }
        }
    }
}

/// Tab bar: `[Active] Inactive ` with upstream widths (`app.rs:757-790`).
///
/// Each tab gets `title width + 2` cells — `[x]` for the active tab and
/// ` x ` for the others — so there is no extra separator between tabs, and
/// inactive tabs keep the terminal's default foreground (`theme.tab`).
fn draw_tab_bar(buf: &mut Buffer, ox: u16, y: u16, width: usize, menu: &Menu) {
    if width == 0 || menu.app.tabs.is_empty() {
        return;
    }
    let area = Rect::new(ox, y, u16::try_from(width).unwrap_or(u16::MAX), 1);
    let char_set = &menu.char_set;
    let theme = &menu.theme;
    let constraints: Vec<Constraint> = menu
        .app
        .tabs
        .iter()
        .map(|tab| {
            let name_w = u16::try_from(width::str_width(&tab.name)).unwrap_or(u16::MAX);
            Constraint::Length(name_w.saturating_add(2))
        })
        .collect();
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    for (index, tab) in menu.app.tabs.iter().enumerate() {
        let Some(&tab_area) = areas.get(index) else {
            break;
        };
        let line = if index == menu.app.active {
            Line::from(vec![
                Span::styled(char_set.tab_marker_left, theme.tab_marker),
                Span::styled(tab.name.clone(), theme.tab_selected),
                Span::styled(char_set.tab_marker_right, theme.tab_marker),
            ])
        } else {
            Line::from(Span::styled(
                format!(" {} ", width::sanitize(&tab.name)),
                theme.tab,
            ))
        };
        line.render(tab_area, buf);
    }
}

/// Node metrics for the active tab (see [`NodeMetrics`]).
#[must_use]
pub fn node_metrics(menu: &Menu) -> NodeMetrics {
    match menu.app.active_tab() {
        Some(tab) => NodeMetrics::for_rows(&tab.rows, menu.peaks),
        None => NodeMetrics::COMPACT,
    }
}

fn draw_list(buf: &mut Buffer, list_area: Rect, menu: &mut Menu) {
    if list_area.width == 0 || list_area.height == 0 {
        return;
    }
    let visible = menu.app.visible_rows();
    let Some(tab) = menu.app.active_tab() else {
        draw_empty(buf, list_area, menu);
        return;
    };
    if visible.is_empty() {
        draw_empty(buf, list_area, menu);
        return;
    }

    // Upstream `ObjectListWidget::areas` reserves one line above and one
    // below the list for the `•••` indicators (`object_list.rs:301-311`).
    let [header_area, rows_area, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header_area
            Constraint::Min(0),    // list_area
            Constraint::Length(1), // footer_area
        ])
        .areas(list_area);

    let metrics = node_metrics(menu);
    let row_h = usize::from(metrics.pitch());
    let entries_visible = usize::from(rows_area.height) / row_h;
    let len = visible.len();
    let scroll = tab.state.scroll;

    // `•••` above when rows are hidden above the viewport
    // (upstream `object_list.rs:478-489`).
    if scroll > 0 {
        Line::from(Span::styled(menu.char_set.list_more, menu.theme.list_more))
            .alignment(Alignment::Center)
            .render(header_area, buf);
    }

    // `•••` below, except when the last row is only partially rendered but
    // still shows everything that matters (upstream `object_list.rs:491-517`).
    let is_bottom_last = scroll.saturating_add(entries_visible) == len.saturating_sub(1);
    let is_bottom_enough = (usize::from(rows_area.height) % row_h) >= usize::from(metrics.height);
    if scroll.saturating_add(entries_visible) < len && !(is_bottom_last && is_bottom_enough) {
        Line::from(Span::styled(menu.char_set.list_more, menu.theme.list_more))
            .alignment(Alignment::Center)
            .render(footer_area, buf);
    }

    // Row areas: `entries_visible` full rows plus one partial row for the
    // remainder (upstream `object_list.rs:519-527`).
    let mut constraints = vec![Constraint::Length(metrics.height); entries_visible];
    constraints.push(Constraint::Max(metrics.height));
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .spacing(metrics.spacing)
        .split(rows_area);

    for (offset, row_area) in row_areas.iter().enumerate() {
        let Some(&provider_index) = visible.get(scroll + offset) else {
            break;
        };
        let Some(row) = tab.rows.get(provider_index) else {
            continue;
        };
        let selected = scroll + offset == tab.state.focus;
        let armed_confirm = selected && row.confirmable && tab.state.is_armed();
        draw_node(buf, *row_area, menu, row, selected, armed_confirm, metrics);
    }
}

/// One node row: selector column + header + detail line
/// (upstream `node_widget.rs:95-199`).
fn draw_node(
    buf: &mut Buffer,
    area: Rect,
    menu: &Menu,
    row: &Row,
    selected: bool,
    armed_confirm: bool,
    metrics: NodeMetrics,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let [selector_area, node_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1), // selector_area
            Constraint::Min(0),    // node_area
        ])
        .areas(area);

    draw_selector(
        buf,
        selector_area,
        selected,
        metrics,
        &menu.char_set,
        &menu.theme,
    );

    // Header on the node's first line (upstream `node_widget.rs:147-157`).
    if node_area.height >= 1 {
        draw_header(
            buf,
            Rect::new(node_area.x, node_area.y, node_area.width, 1),
            menu,
            row,
            armed_confirm,
        );
    }

    // Detail line on the node's third line: a compact node (flex extension)
    // is header-only, and a partially rendered node shows whatever fits.
    if metrics.height >= NODE_HEIGHT && node_area.height >= NODE_HEIGHT {
        draw_detail(
            buf,
            Rect::new(
                node_area.x,
                node_area.y + NODE_HEIGHT - 1,
                node_area.width,
                1,
            ),
            menu,
            row,
        );
    }
}

/// `░` / `▒` / `░` over the node's lines, only when selected
/// (upstream `SelectorWidget`, `node_widget.rs:213-236`).
///
/// Glyphs are written line by line rather than through a `Layout` split: with
/// fewer lines than the selector has glyphs, a layout solver reorders the
/// constraints and lands `▒` on the only line. A compact node
/// (`metrics.height == 1`) shows just `selector_top`, which is the same thing
/// upstream's three-row split degenerates to.
fn draw_selector(
    buf: &mut Buffer,
    area: Rect,
    selected: bool,
    metrics: NodeMetrics,
    char_set: &CharSet,
    theme: &Theme,
) {
    if !selected || area.width == 0 || area.height == 0 {
        return;
    }
    let glyphs = [
        char_set.selector_top,
        char_set.selector_middle,
        char_set.selector_bottom,
    ];
    for (index, glyph) in glyphs.iter().enumerate() {
        let offset = u16::try_from(index).unwrap_or(u16::MAX);
        if offset >= metrics.height || offset >= area.height {
            break;
        }
        Line::from(Span::styled(*glyph, theme.selector))
            .render(Rect::new(area.x, area.y + offset, area.width, 1), buf);
    }
}

/// Header line: `◇` marker, title, right-aligned target/meta
/// (upstream `HeaderWidget`, `node_widget.rs:239-360`).
fn draw_header(buf: &mut Buffer, area: Rect, menu: &Menu, row: &Row, armed_confirm: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let char_set = &menu.char_set;
    let theme = &menu.theme;
    let bare = menu.app.active_tab().is_some_and(|tab| tab.bare_rows);

    // Right column: the row's current target (upstream `target_line`,
    // `node_widget.rs:258-279`), else flex's generic meta.
    let target_line = if bare {
        Line::default()
    } else if let Some(target) = row.current_target() {
        let title = width::sanitize(&target.title);
        if target.is_default {
            Line::from(vec![
                Span::styled(char_set.default_stream, theme.default_stream),
                Span::from(" "),
                Span::styled(title, theme.node_target),
            ])
        } else {
            Line::from(Span::styled(title, theme.node_target))
        }
    } else {
        match &row.meta {
            Some(meta) => Line::from(Span::styled(width::sanitize(meta), theme.node_target)),
            None => Line::default(),
        }
    };
    let target_width = u16::try_from(target_line.width()).unwrap_or(u16::MAX);

    // Upstream splits `Min(1) title_area | Length(target_width) target_area`
    // with a 1-cell horizontal margin and 1-cell spacing
    // (`node_widget.rs:309-318`).
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(target_width)])
        .horizontal_margin(1)
        .spacing(1)
        .split(area);
    let mut title_area = layout[0];
    let mut target_area = layout[1];

    let default_span = if row.is_default {
        Span::styled(char_set.default_device, theme.default_device)
    } else {
        Span::from(" ")
    };
    let label = if armed_confirm {
        format!("{} -- confirm", row.label)
    } else {
        row.label.clone()
    };
    let title_style = if row.offline {
        theme.offline
    } else {
        theme.node_title
    };
    let title_line = Line::from(vec![
        default_span,
        Span::from(" "),
        Span::styled(width::sanitize(&label), title_style),
    ]);

    let mut ellipses_area = None;
    if title_line.width() > title_area.width as usize {
        // Title does not fit: upstream inserts a 3-cell `...` area between the
        // title and the target (`node_widget.rs:323-342`).
        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),               // title_area
                Constraint::Length(3),            // ellipses_area
                Constraint::Length(1),            // _padding
                Constraint::Length(target_width), // target_area
            ])
            .horizontal_margin(1)
            .split(area);
        title_area = layout[0];
        ellipses_area = Some(layout[1]);
        target_area = layout[3];
    }

    target_line
        .alignment(Alignment::Right)
        .render(target_area, buf);
    if let Some(ellipses_area) = ellipses_area {
        Span::styled("...", theme.node_title).render(ellipses_area, buf);
    }
    title_line.render(title_area, buf);
}

/// Detail line: device config (`▼ profile`) or the volume/meter widgets
/// (upstream `device_widget.rs:141-153` and `node_widget.rs:165-198`).
fn draw_detail(buf: &mut Buffer, area: Rect, menu: &Menu, row: &Row) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let meter_on = row.peaks.is_some() && menu.peaks != Peaks::Off;
    let volume_on = row.volume.is_some();

    if volume_on || (meter_on && row.peaks.is_some()) {
        let (volume_area, meter_area) = if meter_on {
            let layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(vec![
                    Constraint::Length(2), // _padding
                    Constraint::Fill(4),   // volume_area
                    Constraint::Fill(1),   // _padding
                    Constraint::Fill(4),   // meter_area
                    Constraint::Fill(1),   // _padding
                ])
                .split(area);
            (layout[1], Some(layout[3]))
        } else {
            let layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(vec![
                    Constraint::Length(2), // _padding
                    Constraint::Fill(9),   // volume_area
                    Constraint::Fill(1),   // _padding
                ])
                .split(area);
            (layout[1], None)
        };

        draw_volume(buf, volume_area, menu, row);
        if let (Some(meter_area), Some(peaks)) = (meter_area, row.peaks) {
            meter::render(
                meter_area,
                buf,
                peaks,
                menu.peaks,
                &menu.char_set,
                &menu.theme,
            );
        }
    } else if let Some(config) = &row.config {
        Line::from(vec![
            Span::from("    "),
            Span::styled(menu.char_set.dropdown_icon, menu.theme.dropdown_icon),
            Span::from(" "),
            Span::styled(width::sanitize(config), menu.theme.config_profile),
        ])
        .render(area, buf);
    }
}

/// Volume label + bar (upstream `VolumeWidget`, `node_widget.rs:373-423`).
fn draw_volume(buf: &mut Buffer, area: Rect, menu: &Menu, row: &Row) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let char_set = &menu.char_set;
    let theme = &menu.theme;
    let [label_area, bar_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(5), // volume_label
            Constraint::Min(0),    // volume_bar
        ])
        .spacing(1)
        .areas(area);

    if let Some(volume) = row.volume {
        let percent = (volume * 100.0).round();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let percent = percent as u32;
        Line::from(Span::styled(format!("{percent}%"), theme.volume))
            .alignment(Alignment::Right)
            .render(label_area, buf);

        let max_volume = menu.max_volume_percent / 100.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let count = ((volume.clamp(0.0, max_volume) / max_volume) * f32::from(bar_area.width))
            .round() as usize;
        let count = count.min(usize::from(bar_area.width));
        let filled = char_set.volume_filled.repeat(count);
        let blank = char_set
            .volume_empty
            .repeat((bar_area.width as usize).saturating_sub(count));
        Line::from(vec![
            Span::styled(filled, theme.volume_filled),
            Span::styled(blank, theme.volume_empty),
        ])
        .render(bar_area, buf);
    }

    // Upstream draws `muted` over the label area after the percentage
    // (`node_widget.rs:421-423`).
    if row.muted {
        Line::from(Span::styled("muted", theme.volume)).render(label_area, buf);
    }
}

/// Target dropdown, right-aligned over the list (upstream `DropdownWidget`,
/// `dropdown_widget.rs`; geometry from `node_widget.rs:62-89`).
fn draw_dropdown(buf: &mut Buffer, list_area: Rect, menu: &mut Menu) {
    let Some(dropdown) = menu.app.dropdown().cloned() else {
        return;
    };
    let visible = menu.app.visible_rows();
    let Some(tab) = menu.app.active_tab() else {
        return;
    };
    let Some(row) = tab.rows.get(dropdown.row) else {
        return;
    };
    if row.targets.is_empty() || list_area.width == 0 {
        return;
    }

    // The dropdown opens on the selected row: find its on-screen band.
    let Some(offset) = visible.iter().position(|index| *index == dropdown.row) else {
        return;
    };
    let row_h = usize::from(node_metrics(menu).pitch());
    let rows_area = Rect::new(
        list_area.x,
        list_area.y.saturating_add(1),
        list_area.width,
        list_area.height.saturating_sub(LIST_INDICATOR_ROWS),
    );
    let rel = offset.saturating_sub(tab.state.scroll);
    #[allow(clippy::cast_possible_truncation)]
    let object_y = rows_area.y.saturating_add((rel * row_h) as u16);

    // Upstream: width = longest target + 4 (borders + highlight symbol),
    // height = min(5, items) + 2, right-aligned to the list area, one row
    // above the selected object (`node_widget.rs:63-89`). Flex measures the
    // display width (upstream uses the byte length).
    let max_target_width = row
        .targets
        .iter()
        .map(|target| width::str_width(&target.title))
        .max()
        .unwrap_or(0);
    #[allow(clippy::cast_possible_truncation)]
    let dropdown_width = (max_target_width.saturating_add(4)) as u16;
    #[allow(clippy::cast_possible_truncation)]
    let dropdown_height = (row.targets.len().min(DROPDOWN_MAX_ITEMS).saturating_add(2)) as u16;
    if dropdown_width == 0 || dropdown_height == 0 {
        return;
    }
    let x = list_area
        .right()
        .saturating_sub(dropdown_width)
        .max(list_area.x);
    let y = object_y.saturating_sub(1);
    let dropdown_area = Rect::new(x, y, dropdown_width, dropdown_height);

    Clear.render(dropdown_area, buf);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(menu.theme.dropdown_border)
        .border_type(menu.char_set.dropdown_border);
    let items: Vec<ListItem> = row
        .targets
        .iter()
        .map(|target| ListItem::new(Line::from(width::sanitize(&target.title))))
        .collect();
    let highlight_symbol = format!("{} ", menu.char_set.dropdown_selector);
    let list = List::new(items)
        .block(block)
        .style(menu.theme.dropdown_item)
        .highlight_symbol(highlight_symbol.as_str())
        .highlight_style(menu.theme.dropdown_selected);

    let mut state = ListState::default()
        .with_selected(Some(dropdown.selected))
        .with_offset(dropdown.top);
    StatefulWidget::render(list, dropdown_area, buf, &mut state);

    // `•••` on the borders when the target list is scrolled
    // (upstream `dropdown_widget.rs:88-135`).
    let inner_height = usize::from(dropdown_area.height.saturating_sub(2));
    let top_index = state.offset();
    let bottom_index = top_index.saturating_add(inner_height);
    let indicator_style = menu.theme.dropdown_more;
    let indicator = menu.char_set.dropdown_more;
    if top_index > 0 {
        Line::from(Span::styled(indicator, indicator_style))
            .alignment(Alignment::Center)
            .render(
                Rect::new(dropdown_area.x, dropdown_area.y, dropdown_area.width, 1),
                buf,
            );
    }
    if bottom_index < row.targets.len() {
        let y = dropdown_area
            .y
            .saturating_add(dropdown_area.height.saturating_sub(1));
        Line::from(Span::styled(indicator, indicator_style))
            .alignment(Alignment::Center)
            .render(Rect::new(dropdown_area.x, y, dropdown_area.width, 1), buf);
    }

    if let Some(state) = menu.app.dropdown_mut() {
        state.top = top_index;
    }
}

fn draw_empty(buf: &mut Buffer, list_area: Rect, menu: &Menu) {
    let text = if menu.offline {
        OFFLINE_STATE
    } else {
        EMPTY_STATE
    };
    let w = width::str_width(text);
    let base = usize::from(list_area.x);
    #[allow(clippy::cast_possible_truncation)]
    let y = list_area.y + list_area.height / 2;
    let mut x = base + usize::from(list_area.width).saturating_sub(w) / 2;
    for c in text.chars() {
        if x >= base + usize::from(list_area.width) {
            break;
        }
        #[allow(clippy::cast_possible_truncation)]
        if let Some(cell) = buf.cell_mut((x as u16, y)) {
            cell.set_symbol(&c.to_string());
            cell.set_style(menu.theme.offline);
            cell.set_skip(false);
        }
        x += width::char_width(c).max(1);
    }
}

/// Help overlay (upstream geometry `app.rs:811-840`, chrome `help.rs:46-53`):
/// centered inside the list area, `sum(widths) × min(rows + 2, 90%)`, `Clear`
/// behind it, `help_border` block (no title upstream), and `•••` on the
/// border rows when the list scrolls.
fn draw_help(buf: &mut Buffer, list_area: Rect, menu: &mut Menu) {
    if list_area.width == 0 || list_area.height == 0 {
        return;
    }
    let lines = HELP_LINES;
    let content_width = lines
        .iter()
        .map(|line| width::str_width(line))
        .max()
        .unwrap_or(0);
    #[allow(clippy::cast_possible_truncation)]
    let wanted_width = (content_width.saturating_add(4)) as u16; // borders + padding
    let [help_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Max(wanted_width)])
        .flex(Flex::Center)
        .areas(list_area);

    // Upstream caps the overlay at 90% of the available height
    // (`app.rs:826-835`).
    #[allow(clippy::cast_possible_truncation)]
    let wanted_height = (lines.len().saturating_add(2)) as u16;
    // Upstream caps the overlay at 90% of the available height; the value is
    // bounded by the frame height, so the truncation cannot be significant.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let max_height = (f32::from(help_area.height) * 0.90) as u16;
    let height = wanted_height.min(max_height.max(2));
    let [help_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(height.min(help_area.height))])
        .flex(Flex::Center)
        .areas(help_area);

    Clear.render(help_area, buf);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(menu.theme.help_border)
        .border_type(menu.char_set.help_border);
    let inner = block.inner(help_area);
    block.render(help_area, buf);

    let max_scroll = lines.len().saturating_sub(usize::from(inner.height));
    if menu.app.help_scroll > max_scroll {
        menu.app.help_scroll = max_scroll;
    }
    let scroll = menu.app.help_scroll;

    if scroll > 0 {
        Line::from(Span::styled(menu.char_set.help_more, menu.theme.help_more))
            .alignment(Alignment::Center)
            .render(Rect::new(help_area.x, help_area.y, help_area.width, 1), buf);
    }
    let last = scroll.saturating_add(usize::from(inner.height));
    if last < lines.len() {
        let y = help_area
            .y
            .saturating_add(help_area.height.saturating_sub(1));
        Line::from(Span::styled(menu.char_set.help_more, menu.theme.help_more))
            .alignment(Alignment::Center)
            .render(Rect::new(help_area.x, y, help_area.width, 1), buf);
    }

    for (offset, line) in lines
        .iter()
        .skip(scroll)
        .take(usize::from(inner.height))
        .enumerate()
    {
        #[allow(clippy::cast_possible_truncation)]
        let y = inner.y + offset as u16;
        Line::from(Span::styled(
            width::truncate_exact(line, usize::from(inner.width)),
            menu.theme.help_item,
        ))
        .render(Rect::new(inner.x, y, inner.width, 1), buf);
    }
}

/// Flex extension: the filter line (`› text …          3/87`).
fn draw_filter(buf: &mut Buffer, ox: u16, y: u16, width: usize, menu: &Menu) {
    if width == 0 {
        return;
    }
    let area = Rect::new(ox, y, u16::try_from(width).unwrap_or(u16::MAX), 1);
    let theme = &menu.theme;

    Span::styled(FILTER_PROMPT.to_string(), theme.filter_prompt).render(area, buf);
    let prompt_area = Rect::new(ox.saturating_add(2), y, area.width.saturating_sub(2), 1);
    if prompt_area.width == 0 {
        return;
    }

    let total = menu.app.visible_len();
    let filter = menu
        .app
        .active_tab()
        .map(|tab| tab.state.filter.clone())
        .unwrap_or_default();
    let pos = menu.app.active_tab().map_or(0, |tab| {
        if total == 0 {
            0
        } else {
            tab.state.focus.min(total - 1) + 1
        }
    });

    let prompt: &str = if filter.is_empty() {
        "filter…"
    } else {
        &filter
    };
    let prompt_style = if filter.is_empty() {
        theme.hint
    } else {
        theme.node_title
    };
    Line::from(Span::styled(prompt.to_string(), prompt_style)).render(prompt_area, buf);

    let right = format!("{pos}/{total}");
    let right_w = u16::try_from(width::str_width(&right)).unwrap_or(u16::MAX);
    if right_w < prompt_area.width {
        let right_area = Rect::new(area.right().saturating_sub(right_w), y, right_w, 1);
        Line::from(Span::styled(right, theme.hint)).render(right_area, buf);
    }
}

/// Flex extension: the hint line under the list.
fn draw_hints(buf: &mut Buffer, ox: u16, y: u16, width: usize, menu: &Menu) {
    if width == 0 {
        return;
    }
    let pending = menu.app.active_state().is_some_and(|s| s.confirm_pending);
    let text = if pending {
        "Delete/Enter deletes · esc cancels"
    } else if menu.app.is_armed() {
        "Enter again to confirm · esc disarms"
    } else if menu.app.dropdown().is_some() {
        "↑↓ choose target · enter applies · esc closes"
    } else {
        "↑↓ navigate · enter select · esc cancel"
    };
    let area = Rect::new(ox, y, u16::try_from(width).unwrap_or(u16::MAX), 1);
    Line::from(Span::styled(
        width::truncate_exact(text, width),
        menu.theme.hint,
    ))
    .render(area, buf);
}

/// Flex extension: the `center` gauge row.
fn draw_gauge(buf: &mut Buffer, ox: u16, y: u16, width: usize, menu: &Menu) {
    let Some(gauge) = menu.gauge.as_ref() else {
        return;
    };
    if width == 0 {
        return;
    }
    let theme = &menu.theme;
    let base = usize::from(ox);

    if !gauge.online || menu.offline {
        let mut x = base;
        for c in OFFLINE_STATE.chars() {
            if x >= base + width {
                break;
            }
            #[allow(clippy::cast_possible_truncation)]
            if let Some(cell) = buf.cell_mut((x as u16, y)) {
                cell.set_symbol(&c.to_string());
                cell.set_style(theme.offline);
                cell.set_skip(false);
            }
            x += width::char_width(c).max(1);
        }
        return;
    }

    // `muted` replaces the percentage, like upstream's volume label.
    let value = if gauge.muted {
        "muted".to_string()
    } else {
        format!("{}%", gauge.value.min(100))
    };
    let left = format!(" {} ", gauge.label);
    let right = format!(" {value}");
    let inner = width
        .saturating_sub(width::str_width(&left))
        .saturating_sub(width::str_width(&right))
        .saturating_sub(2);

    let mut x = base;
    let mut put = |buf: &mut Buffer, text: &str, style: Style| {
        for c in text.chars() {
            if x >= base + width {
                break;
            }
            #[allow(clippy::cast_possible_truncation)]
            if let Some(cell) = buf.cell_mut((x as u16, y)) {
                cell.set_symbol(&c.to_string());
                cell.set_style(style);
                cell.set_skip(false);
            }
            x += width::char_width(c).max(1);
        }
    };

    put(buf, &left, theme.hint);
    put(buf, "[", theme.hint);

    let fill = inner * usize::from(gauge.value.min(100)) / 100;
    for _ in 0..fill {
        put(buf, menu.char_set.volume_filled, theme.gauge_fill);
    }
    for _ in fill..inner {
        put(buf, menu.char_set.volume_empty, theme.volume_empty);
    }
    put(buf, "]", theme.hint);
    put(buf, &right, theme.hint);
}

fn place_cursor(frame: &mut Frame, area: Rect, menu: &Menu, hidden: bool) {
    if hidden {
        return;
    }
    if let Some(position) = cursor_position(area, menu) {
        frame.set_cursor_position(position);
    }
}

/// Caret cell for the active tab (`None` when the frame has no filter line).
///
/// Shared with `run`: after the image preview moves the terminal cursor, the
/// event loop re-parks it here so the caret never appears inside the pane.
#[must_use]
pub fn cursor_position(area: Rect, menu: &Menu) -> Option<(u16, u16)> {
    let filter_w = menu
        .app
        .active_tab()
        .map_or(0, |tab| width::str_width(&tab.state.filter));
    let tab_bar_h = TAB_BAR_HEIGHT;
    let hints_h = 1u16;
    let filter_h = 1u16;

    // Saturating: degenerate frames (1x1) must not underflow.
    let y = area
        .y
        .saturating_add(area.height)
        .saturating_sub(tab_bar_h)
        .saturating_sub(hints_h)
        .saturating_sub(filter_h);
    let x = area
        .x
        .saturating_add(2)
        .saturating_add(u16::try_from(filter_w).unwrap_or(0));

    if x < area.x + area.width && y < area.y + area.height {
        Some((x, y))
    } else {
        None
    }
}

/// Upstream-shaped row metrics, exposed for tests: `(target width, title
/// width)` for a header at `width_cells`.
#[must_use]
pub fn row_layout(meta: Option<&str>, bare: bool, width_cells: usize) -> (usize, usize) {
    let mut meta_w = if bare {
        0
    } else {
        meta.map_or(0, |m| width::str_width(&width::sanitize(m)))
    };
    if !bare && meta.is_some() && meta_w > width_cells.saturating_sub(4) {
        meta_w = width::ELLIPSIS_WIDTH;
    }
    let avail = width_cells
        .saturating_sub(2)
        .saturating_sub(1)
        .saturating_sub(meta_w)
        .saturating_sub(1);
    (meta_w, avail)
}
