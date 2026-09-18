//! `flex-core` — reusable Rust TUI menu engine.
//!
//! The engine selects rows and reports an [`Outcome`]; it never executes side
//! effects and knows nothing about any particular desktop setup. Callers (see
//! the `flex-rice` crate) build the rows, install a [`TickHook`] when those
//! rows need periodic refreshing, then print the `ACTION:` line themselves
//! (see [`backend`]).

pub mod backend;
pub mod charset;
pub mod diag;
pub mod filter;
pub mod keys;
pub mod meter;
pub mod preview;
pub mod render;
pub mod run;
pub mod theme;
pub mod width;

pub use charset::{CharSet, CharSetName};
pub use theme::{Theme, ThemeName};

use std::collections::BTreeSet;
use std::time::Instant;

/// Stable identifier for a [`Row`]'s action.
///
/// For the `clip` provider this is the content-hash hex (Q2) so wrappers can
/// round-trip history entries across runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowId(pub String);

impl RowId {
    /// Create an id from a precomputed hex string.
    #[must_use]
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    /// Borrow the inner hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// FNV-1a 64-bit hex of `content` (std-only, stable across runs).
///
/// Used for `clip` rows (Q2) until M2 wires the real provider parsing.
#[must_use]
pub fn content_hash_hex(content: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A row's selectable target (wiremix `view::Target` + its title).
///
/// Shown right-aligned in the header (with the `default_stream` marker `◇`
/// when [`Target::is_default`]) and listed in the row's dropdown
/// (`Enter`/`c`), mirroring upstream `node_targets` + `DropdownWidget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Opaque id reported on the `ACTION:TARGET` line.
    pub id: RowId,
    /// Display title (dropdown item + header right column).
    pub title: String,
    /// Upstream `Target::Default`: prefix the header title with `◇`.
    pub is_default: bool,
}

impl Target {
    /// Build a target.
    #[must_use]
    pub fn new(id: RowId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            is_default: false,
        }
    }

    /// Build the upstream `Target::Default` entry (`◇` in the header).
    #[must_use]
    pub fn default_target(id: RowId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            is_default: true,
        }
    }
}

/// Peak levels for one row, as upstream `node.peaks`/`node.positions` express
/// them: values are linear amplitudes, and the channel count picks the meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowPeaks {
    /// One averaged channel (upstream mono rendering).
    Mono(f32),
    /// Left/right channels (upstream stereo rendering).
    Stereo(f32, f32),
}

/// Peak-meter rendering mode (upstream `Peaks`, `config.rs:97-102`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Peaks {
    /// No meters at all (`--peaks off`).
    Off,
    /// Force mono meters even for stereo rows (`--peaks mono`).
    Mono,
    /// Stereo when the row has two channels, mono otherwise (upstream default).
    #[default]
    Auto,
}

/// A single selectable menu entry.
///
/// The bool fields are independent row attributes (danger, offline, default,
/// muted), not a flag bag: they mirror distinct upstream `Node` properties.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Row {
    /// Opaque action id handed to wrappers via the `ACTION:` line.
    pub id: RowId,
    /// Human-readable label (may be truncated at render time).
    pub label: String,
    /// Right-aligned metadata (e.g. `87%`, `wlan0`), used when the row has no
    /// [`Row::targets`]. Rendered in the `node_target` style (terminal
    /// default, matching upstream) — not dimmed.
    pub meta: Option<String>,
    /// Danger rows (shutdown/reboot/…) are triple-coded at render time.
    pub confirmable: bool,
    /// Offline rows (e.g. center `— offline` when `nmcli` finds no Wi-Fi
    /// interface) render dim like [`Menu::offline`], but per-row: one
    /// offline tab must not dim its siblings. Wrappers treat them as
    /// no-ops (`noop` id, mirroring the bash `noop) :` arm).
    pub offline: bool,
    /// Default row: draws the `default_device` marker (`◇`) in the header's
    /// marker column (upstream `Node::is_default_sink`/`is_default_source`).
    pub is_default: bool,
    /// Cube-root volume in `0.0..` (upstream's mean channel volume after
    /// `cbrt`), e.g. `Some(0.85)` renders the label `85%`. `None` leaves the
    /// detail line without a volume bar.
    pub volume: Option<f32>,
    /// Muted rows show `muted` in the volume label area (upstream
    /// `Node::mute`).
    pub muted: bool,
    /// Peak levels for the detail line's meter (upstream `Node::peaks`).
    pub peaks: Option<RowPeaks>,
    /// Device-style config line (`▼ profile`) drawn on the detail line
    /// (upstream `DeviceWidget`, `device_widget.rs:141-153`).
    pub config: Option<String>,
    /// Plain text detail line (e.g. notification body) drawn on line 3 without `▼`.
    pub detail: Option<String>,
    /// Middle line text (e.g. notification summary) drawn on line 2 of a 3-line node.
    pub sublabel: Option<String>,
    /// Dropdown targets, in display order (upstream `node_targets`). Empty
    /// means the row has no dropdown and the header shows [`Row::meta`].
    pub targets: Vec<Target>,
    /// Highlighted target when the dropdown opens (upstream `node_targets`
    /// returns the current target's position); also the title shown in the
    /// header. Clamped to `targets.len() - 1`.
    pub target_index: usize,
    /// Absolute path of the image this row previews, when the provider shows
    /// one (`wallpaper`). Display-only: the preview pane
    /// ([`preview`](crate::preview)) reads it, while ids, labels, filtering
    /// and the `ACTION:` contract are unaffected.
    pub preview_image: Option<String>,
    /// When true, the row's targets are not drawn in the header right column,
    /// leaving space for [`Row::meta`] (e.g. notification timestamps).
    pub hide_target_in_header: bool,
    /// Compact 1-line node rendering with 0 spacing (e.g. child notification thread rows).
    pub compact: bool,
}

impl Row {
    /// Build a plain (non-danger, meta-less) row.
    #[must_use]
    pub fn new(id: RowId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            meta: None,
            confirmable: false,
            offline: false,
            is_default: false,
            volume: None,
            muted: false,
            peaks: None,
            config: None,
            detail: None,
            sublabel: None,
            targets: Vec::new(),
            target_index: 0,
            preview_image: None,
            hide_target_in_header: false,
            compact: false,
        }
    }

    /// Build a row with right-aligned metadata.
    #[must_use]
    pub fn with_meta(id: RowId, label: impl Into<String>, meta: impl Into<String>) -> Self {
        Self {
            meta: Some(meta.into()),
            ..Self::new(id, label)
        }
    }

    /// Build a confirmable row (requires armed confirm via `keys`).
    #[must_use]
    pub fn confirmable(id: RowId, label: impl Into<String>) -> Self {
        Self {
            confirmable: true,
            ..Self::new(id, label)
        }
    }

    /// Build an offline placeholder row (`— offline`, dim, wrapper no-op).
    #[must_use]
    pub fn offline_placeholder(id: RowId, label: impl Into<String>) -> Self {
        Self {
            offline: true,
            ..Self::new(id, label)
        }
    }

    /// Build a row with a volume bar (`volume` = cube-root volume, `1.0` =
    /// `100%`).
    #[must_use]
    pub fn with_volume(id: RowId, label: impl Into<String>, volume: f32) -> Self {
        Self {
            volume: Some(volume),
            ..Self::new(id, label)
        }
    }

    /// Build a row with dropdown targets and the current target index.
    #[must_use]
    pub fn with_targets(
        id: RowId,
        label: impl Into<String>,
        targets: Vec<Target>,
        current: usize,
    ) -> Self {
        Self {
            targets,
            target_index: current,
            ..Self::new(id, label)
        }
    }

    /// Build a row with a plain text detail line (no `▼` icon).
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Build a row with middle line text (sublabel).
    #[must_use]
    pub fn with_sublabel(mut self, sublabel: impl Into<String>) -> Self {
        self.sublabel = Some(sublabel.into());
        self
    }

    /// Build a device-style row whose detail line is `▼ config`.
    #[must_use]
    pub fn with_config(id: RowId, label: impl Into<String>, config: impl Into<String>) -> Self {
        Self {
            config: Some(config.into()),
            ..Self::new(id, label)
        }
    }

    /// The row's current target, if it has any.
    #[must_use]
    pub fn current_target(&self) -> Option<&Target> {
        if self.targets.is_empty() {
            return None;
        }
        let index = self.target_index.min(self.targets.len() - 1);
        self.targets.get(index)
    }

    /// Mark the row as the default one (`◇` marker).
    #[must_use]
    pub fn default_marked(mut self) -> Self {
        self.is_default = true;
        self
    }

    /// Mark the row muted (`muted` in the volume label area).
    #[must_use]
    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }

    /// Attach peak levels (`RowPeaks::Stereo`/`Mono`, linear amplitudes).
    #[must_use]
    pub fn with_peaks(mut self, peaks: RowPeaks) -> Self {
        self.peaks = Some(peaks);
        self
    }

    /// Attach the row's preview image path (see [`Row::preview_image`]).
    #[must_use]
    pub fn with_preview_image(mut self, path: impl Into<String>) -> Self {
        self.preview_image = Some(path.into());
        self
    }

    /// Hide targets from the header's right column so [`Row::meta`] is shown.
    #[must_use]
    pub fn hide_target_in_header(mut self) -> Self {
        self.hide_target_in_header = true;
        self
    }
}

/// An open target dropdown (upstream `ObjectList::dropdown_state`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DropdownState {
    /// Provider-row index the dropdown is open for.
    pub row: usize,
    /// Highlighted target index within that row's `targets`.
    pub selected: usize,
    /// First visible target index (upstream ratatui `ListState` offset),
    /// maintained by the renderer as the highlight moves.
    pub top: usize,
}

/// Independent per-tab interaction state.
///
/// Tab switching preserves (and restores) all of this; nothing here leaks
/// across tabs.
#[derive(Debug, Default)]
pub struct TabState {
    /// Focused row index within the tab's current filtered view.
    pub focus: usize,
    /// First visible row of the filtered view (scroll offset).
    pub scroll: usize,
    /// Current filter text for this tab.
    pub filter: String,
    /// Marked rows, as provider-row indices (stable across refilters).
    pub marked: BTreeSet<usize>,
    /// When the danger flow was armed (`keys::handle_key`), if armed.
    pub armed_at: Option<Instant>,
    /// `Delete` was pressed once; `Delete`/`Enter` deletes, `Esc` cancels.
    pub confirm_pending: bool,
    /// Open target dropdown, if any (upstream `dropdown_state`).
    pub dropdown: Option<DropdownState>,
}

impl TabState {
    /// Whether the danger flow is currently armed.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// Arm the danger flow at `now`.
    pub fn arm(&mut self, now: Instant) {
        self.armed_at = Some(now);
    }

    /// Disarm the danger flow; returns whether it was armed.
    pub fn disarm(&mut self) -> bool {
        self.armed_at.take().is_some()
    }
}

/// A named tab (one provider view) holding its rows plus state.
#[derive(Debug, Default)]
pub struct Tab {
    /// Tab title shown in the tab bar.
    pub name: String,
    /// Rows owned by this tab; filtering happens in-process.
    pub rows: Vec<Row>,
    /// Launcher-like lists set this to hide tab bar / meta / gauge.
    pub bare_rows: bool,
    /// Whether the tab shows a filter line and accepts type-to-filter.
    /// Fixed-choice menus (`power`) opt out: five rows need no search.
    pub filterable: bool,
    /// Whether the `Delete` key arms the delete-confirm flow on this tab.
    /// Launch/power rows are never deletable (`false`); `clip` opts in.
    pub deletable: bool,
    /// Independent interaction state (preserved across tab switches).
    pub state: TabState,
}

impl Tab {
    /// Build an empty tab with the given name.
    #[must_use]
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rows: Vec::new(),
            bare_rows: true,
            filterable: true,
            deletable: false,
            state: TabState::default(),
        }
    }

    /// Build a tab with rows.
    #[must_use]
    pub fn with_rows(name: impl Into<String>, rows: Vec<Row>) -> Self {
        Self {
            name: name.into(),
            rows,
            bare_rows: true,
            filterable: true,
            deletable: false,
            state: TabState::default(),
        }
    }
}

/// Input mode: filter editing vs pure navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Keystrokes edit the filter (default).
    #[default]
    Normal,
    /// Keystrokes navigate (`j`/`k`, etc.); runes are ignored.
    Navigate,
}

/// Gauge/toggle widget state (e.g. volume in `center`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gauge {
    /// Widget label (`"Volume"`, …).
    pub label: String,
    /// Fill level 0–100.
    pub value: u8,
    /// Mute toggle (`[●]` unmuted / `[○]` muted).
    pub muted: bool,
    /// Online fill vs dim `— offline` (Q7).
    pub online: bool,
}

impl Gauge {
    /// Build an online unmuted gauge, clamping `value` to 0–100.
    #[must_use]
    pub fn new(label: impl Into<String>, value: u8) -> Self {
        Self {
            label: label.into(),
            value: value.min(100),
            muted: false,
            online: true,
        }
    }

    /// Flip the mute toggle.
    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
    }
}

/// What the binary reports to its wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A row was chosen: `(provider, action_id, label)`.
    Chosen {
        /// Provider name (`power`, `launch`, …).
        provider: String,
        /// Opaque action id (`RowId` hex).
        action_id: String,
        /// Human label (escaped on the `ACTION:` line).
        label: String,
    },
    /// User cancelled (`Esc`, `q` in NORMAL + empty filter).
    Cancelled,
    /// A deletable row was confirmed for deletion: `(provider, action_id, label)`.
    ///
    /// Reported on the `ACTION:DELETE` line; the wrapper performs the removal.
    Delete {
        /// Provider name (`clip`, …).
        provider: String,
        /// Opaque action id (`RowId` hex).
        action_id: String,
        /// Human label (escaped on the `ACTION:DELETE` line).
        label: String,
    },
    /// A pin was toggled on a deletable row: `(provider, action_id, label)`.
    ///
    /// Reported on the `ACTION:TOGGLE` line; the wrapper flips the pin.
    Toggle {
        /// Provider name (`clip`, …).
        provider: String,
        /// Opaque action id (`RowId` hex).
        action_id: String,
        /// Human label (escaped on the `ACTION:TOGGLE` line).
        label: String,
    },
    /// A dropdown target was chosen: `(provider, row, target, title)`.
    ///
    /// Reported on the `ACTION:TARGET` line; the wrapper applies the routing
    /// change.
    Target {
        /// Provider name (`center`, …).
        provider: String,
        /// Opaque action id of the row the dropdown belongs to.
        row: String,
        /// Opaque id of the chosen target.
        target: String,
        /// Human title of the chosen target (escaped on the line).
        title: String,
    },
    /// Quit the menu with an exit code (no output).
    ///
    /// `130` is the user-cancelled code (`Esc`, `q` in NORMAL + empty
    /// filter, `Ctrl-c`); other codes are runtime quits.
    Quit {
        /// Process exit code to quit with.
        code: i32,
    },
}

/// Runtime application state (tabs + mode + help overlay).
///
/// Selection, scroll, filter, marks, and arming live per tab
/// ([`TabState`]); this struct only holds what is shared.
#[derive(Debug, Default)]
pub struct App {
    /// All tabs; the active tab is `tabs[active]`.
    pub tabs: Vec<Tab>,
    /// Index into `tabs` of the active tab.
    pub active: usize,
    /// Input mode.
    pub mode: Mode,
    /// Whether the help overlay is open.
    pub help_open: bool,
    /// First visible help line (upstream `help_position`); the help overlay
    /// scrolls with the movement keys while open.
    pub help_scroll: usize,
    /// Ranking engine for the filtered view (`Spec` default; `Legacy`
    /// preserves provider order via `--filter-mode=legacy`).
    pub filter_mode: filter::FilterMode,
}

impl App {
    /// Create an app with no tabs.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an app with tabs, activating the first one.
    #[must_use]
    pub fn with_tabs(tabs: Vec<Tab>) -> Self {
        Self {
            tabs,
            active: 0,
            mode: Mode::default(),
            help_open: false,
            help_scroll: 0,
            filter_mode: filter::FilterMode::default(),
        }
    }

    /// Borrow the active tab, if any.
    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// Mutably borrow the active tab, if any.
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// Borrow the active tab's state, if any.
    #[must_use]
    pub fn active_state(&self) -> Option<&TabState> {
        self.active_tab().map(|tab| &tab.state)
    }

    /// Switch tabs, preserving all per-tab state (focus/scroll/filter/
    /// marked/armed). Out-of-range indices are ignored; the newly shown
    /// focus is clamped into its filtered view.
    pub fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        self.clamp_focus();
    }

    /// The active tab's open dropdown, if any.
    #[must_use]
    pub fn dropdown(&self) -> Option<&DropdownState> {
        self.active_state()
            .and_then(|state| state.dropdown.as_ref())
    }

    /// Mutable access to the active tab's open dropdown.
    pub fn dropdown_mut(&mut self) -> Option<&mut DropdownState> {
        self.active_tab_mut()?.state.dropdown.as_mut()
    }

    /// Open the focused row's dropdown (upstream `Action::ActivateDropdown`).
    ///
    /// No-op when the view is empty or the focused row has no
    /// [`Row::targets`]. The highlight starts on the row's current target,
    /// like upstream's `node_targets` position. Returns whether a dropdown
    /// is now open.
    pub fn open_dropdown(&mut self) -> bool {
        let Some(row_index) = self.focused_original_index() else {
            return false;
        };
        let Some(row) = self.active_tab().and_then(|tab| tab.rows.get(row_index)) else {
            return false;
        };
        if row.targets.is_empty() {
            return false;
        }
        let selected = row.target_index.min(row.targets.len() - 1);
        if let Some(tab) = self.active_tab_mut() {
            tab.state.dropdown = Some(DropdownState {
                row: row_index,
                selected,
                top: 0,
            });
        }
        true
    }

    /// Close the active tab's dropdown; returns whether one was open.
    pub fn close_dropdown(&mut self) -> bool {
        let Some(tab) = self.active_tab_mut() else {
            return false;
        };
        tab.state.dropdown.take().is_some()
    }

    /// Move the open dropdown's highlight by `delta`, clamped to the list.
    ///
    /// Returns whether a dropdown is open (the highlight may be unchanged at
    /// either end).
    pub fn move_dropdown(&mut self, delta: isize) -> bool {
        let Some(tab) = self.active_tab_mut() else {
            return false;
        };
        let Some(dropdown) = tab.state.dropdown.as_mut() else {
            return false;
        };
        let Some(row) = tab.rows.get(dropdown.row) else {
            return false;
        };
        let len = row.targets.len();
        if len > 0 {
            #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
            let next = (dropdown.selected as isize + delta).clamp(0, len as isize - 1);
            dropdown.selected = next.cast_unsigned();
        }
        true
    }

    /// The highlighted target of the open dropdown, plus its index.
    #[must_use]
    pub fn highlighted_target(&self) -> Option<(usize, &Target)> {
        let dropdown = self.dropdown()?;
        let row = self.active_tab()?.rows.get(dropdown.row)?;
        if row.targets.is_empty() {
            return None;
        }
        let index = dropdown.selected.min(row.targets.len() - 1);
        Some((index, &row.targets[index]))
    }

    /// Commit the highlighted target on the row the dropdown is open for and
    /// close the dropdown; returns the chosen target.
    ///
    /// The row's [`Row::target_index`] is updated so the header shows the new
    /// target immediately (upstream re-reads it from `PipeWire` state instead).
    pub fn commit_dropdown(&mut self) -> Option<(RowId, RowId, String)> {
        let dropdown = self.dropdown()?.clone();
        let tab = self.active_tab_mut()?;
        let row = tab.rows.get_mut(dropdown.row)?;
        if row.targets.is_empty() {
            return None;
        }
        let index = dropdown.selected.min(row.targets.len() - 1);
        row.target_index = index;
        let target = &row.targets[index];
        let chosen = (row.id.clone(), target.id.clone(), target.title.clone());
        tab.state.dropdown = None;
        Some(chosen)
    }

    /// Cycle to the next (`delta = +1`) or previous tab, wrapping.
    ///
    /// Tab/row counts are far below `isize::MAX`, so the index casts below
    /// cannot wrap or lose sign in practice.
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub fn cycle_tab(&mut self, delta: isize) {
        if self.tabs.is_empty() {
            return;
        }
        let count = self.tabs.len() as isize;
        let next = (self.active as isize + delta).rem_euclid(count);
        self.switch_tab(next as usize);
    }

    /// Ranked provider-row indices for the active tab's filter.
    ///
    /// Empty filter preserves provider order; otherwise score-descending
    /// with original-index tie-break (see [`filter::rank_all`]). In
    /// [`filter::FilterMode::Legacy`] every hit scores equally, so the
    /// tie-break keeps provider order (no score reordering).
    #[must_use]
    pub fn visible_rows(&self) -> Vec<usize> {
        match self.active_tab() {
            None => Vec::new(),
            Some(tab) => {
                let hits = match self.filter_mode {
                    filter::FilterMode::Spec => filter::rank_all(&tab.state.filter, &tab.rows),
                    filter::FilterMode::Legacy => {
                        filter::rank_all_legacy(&tab.state.filter, &tab.rows)
                    }
                };
                hits.into_iter().map(|hit| hit.index).collect()
            }
        }
    }

    /// Number of rows in the active filtered view.
    #[must_use]
    pub fn visible_len(&self) -> usize {
        self.visible_rows().len()
    }

    /// Provider-row index under focus, if the view is non-empty.
    #[must_use]
    pub fn focused_original_index(&self) -> Option<usize> {
        let visible = self.visible_rows();
        if visible.is_empty() {
            return None;
        }
        let focus = self
            .active_state()
            .map_or(0, |s| s.focus.min(visible.len() - 1));
        visible.get(focus).copied()
    }

    /// Row under focus, if any.
    #[must_use]
    pub fn focused_row(&self) -> Option<&Row> {
        let index = self.focused_original_index()?;
        self.active_tab()?.rows.get(index)
    }

    /// Clamp the active focus into its filtered view.
    pub fn clamp_focus(&mut self) {
        let len = self.visible_len();
        if let Some(state) = self.active_tab_mut().map(|tab| &mut tab.state) {
            state.focus = if len == 0 {
                0
            } else {
                state.focus.min(len - 1)
            };
        }
    }

    /// Move focus by `delta` rows, wrapping around the filtered view.
    ///
    /// Counts are far below `isize::MAX` and `rem_euclid` is non-negative,
    /// so the index casts below cannot wrap or lose sign in practice.
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub fn move_focus(&mut self, delta: isize) {
        let len = self.visible_len();
        if len == 0 {
            if let Some(state) = self.active_tab_mut().map(|tab| &mut tab.state) {
                state.focus = 0;
            }
            return;
        }
        if let Some(state) = self.active_tab_mut().map(|tab| &mut tab.state) {
            let next = (state.focus as isize + delta).rem_euclid(len as isize);
            state.focus = next as usize;
        }
    }

    /// Move focus by `delta` rows, clamping at the ends (page keys).
    ///
    /// Counts are far below `isize::MAX` and the result is clamped to
    /// `[0, len - 1]`, so the casts below cannot wrap or lose sign.
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub fn move_focus_clamped(&mut self, delta: isize) {
        let len = self.visible_len();
        if let Some(state) = self.active_tab_mut().map(|tab| &mut tab.state) {
            if len == 0 {
                state.focus = 0;
                return;
            }
            let next = state.focus as isize + delta;
            state.focus = next.clamp(0, len as isize - 1) as usize;
        }
    }

    /// Scroll the active tab so focus is visible.
    /// `entries_visible` is the number of entries that fit in the viewport.
    pub fn ensure_visible(&mut self, entries_visible: usize) {
        self.clamp_focus();
        if let Some(state) = self.active_tab_mut().map(|tab| &mut tab.state) {
            if entries_visible == 0 {
                state.scroll = 0;
                return;
            }
            if state.focus < state.scroll {
                state.scroll = state.focus;
            } else if state.focus >= state.scroll + entries_visible {
                state.scroll = state.focus + 1 - entries_visible;
            }
        }
    }

    /// Whether the active tab's danger flow is armed.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.active_state().is_some_and(TabState::is_armed)
    }

    /// Disarm the active tab; returns whether it was armed.
    pub fn disarm(&mut self) -> bool {
        self.active_tab_mut().is_some_and(|tab| tab.state.disarm())
    }

    /// Periodic tick (1 s gauge cadence, Q7): expire a stale arm
    /// ([`keys::ARM_EXPIRE`]).
    ///
    /// Never touches focus/scroll/filter/marks — ticks must not steal focus.
    /// Returns whether an arm expired on this tick.
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(tab) = self.active_tab_mut() else {
            return false;
        };
        let expired = tab.state.armed_at.is_some_and(|since| {
            now.checked_duration_since(since)
                .is_some_and(|age| age >= keys::ARM_EXPIRE)
        });
        if expired {
            tab.state.disarm();
            true
        } else {
            false
        }
    }
}

/// Per-tick refresh hook, installed by the caller with [`Menu::on_tick`].
///
/// The engine cannot know what a given screen needs refreshed — a volume
/// gauge, a background Wi-Fi scan — so the caller supplies a function that
/// does. It runs after every [`App::tick`], must leave filter, focus and
/// scroll state intact, and defaults to doing nothing.
pub type TickHook = fn(&mut Menu);

/// A configured menu session: provider + [`App`] + widgets.
#[derive(Debug)]
pub struct Menu {
    /// Provider name (`power`, `launch`, …) for the `ACTION:` line.
    pub provider: String,
    /// Tabs + shared UI state.
    pub app: App,
    /// Gauge widget, when the provider shows one (`center`).
    pub gauge: Option<Gauge>,
    /// Offline mode dims labels and renders `— offline` (Q7).
    pub offline: bool,
    /// Glyph set (upstream `char_set`; CLI `--char-set`).
    pub char_set: CharSet,
    /// Style tokens (upstream `theme`; CLI `--theme`).
    pub theme: Theme,
    /// Peak-meter mode (upstream `peaks`; CLI `--peaks`).
    pub peaks: Peaks,
    /// Volume slider ceiling in percent (upstream `max_volume_percent`,
    /// default 150). The volume bar fills `volume / (max/100)` of its width.
    pub max_volume_percent: f32,
    /// Reserve the image preview pane on the right of the list
    /// ([`preview::pane`]). Set by providers whose rows carry
    /// [`Row::preview_image`] and only when the terminal can draw one.
    pub preview: bool,
    /// Per-tick refresh hook; private so it can only be set through
    /// [`Menu::on_tick`].
    on_tick: Option<TickHook>,
}

/// Upstream `max_volume_percent` default (`config.rs:261-263`).
pub const DEFAULT_MAX_VOLUME_PERCENT: f32 = 150.0;

impl Default for Menu {
    fn default() -> Self {
        Self {
            provider: String::new(),
            app: App::default(),
            gauge: None,
            offline: false,
            char_set: CharSet::default(),
            theme: Theme::default(),
            peaks: Peaks::default(),
            max_volume_percent: DEFAULT_MAX_VOLUME_PERCENT,
            preview: false,
            on_tick: None,
        }
    }
}

impl Menu {
    /// Create a menu for `provider` with the given tabs.
    #[must_use]
    pub fn new(provider: impl Into<String>, tabs: Vec<Tab>) -> Self {
        Self {
            provider: provider.into(),
            app: App::with_tabs(tabs),
            ..Self::default()
        }
    }

    /// Install the per-tick refresh hook (builder style).
    ///
    /// See [`TickHook`]; `flex-rice` passes a dispatcher that refreshes the
    /// `center` gauges and picks up a finished `wifi` scan.
    #[must_use]
    pub fn on_tick(mut self, hook: TickHook) -> Self {
        self.on_tick = Some(hook);
        self
    }

    /// Periodic tick; delegates to [`App::tick`] (never steals focus), then
    /// runs the caller's [`TickHook`] if one was installed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let expired = self.app.tick(now);
        if let Some(hook) = self.on_tick {
            hook(self);
        }
        expired
    }
}

/// Deterministic synthetic clipboard corpus (M1 perf spike seed).
///
/// `count` rows of `word word #i`-style labels from an LCG seeded by `seed`
/// (std-only, no RNG dep). Shared by `benches/rerank.rs` and
/// `tests/clip_perf.rs` so both measure the same fixture shape.
///
/// # Panics
///
/// Never panics: word-table indices are always reduced modulo its length.
#[must_use]
pub fn synthetic_clip_corpus(count: usize, seed: u64) -> Vec<Row> {
    const WORDS: &[&str] = &[
        "firefox",
        "terminal",
        "clipboard",
        "screenshot",
        "volume",
        "network",
        "bluetooth",
        "calendar",
        "password",
        "config",
        "deploy",
        "branch",
        "merge",
        "review",
        "issue",
        "window",
        "theme",
        "icon",
        "launcher",
        "session",
    ];
    let mut state = seed | 1;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    (0..count)
        .map(|i| {
            let first = WORDS[next() % WORDS.len()];
            let second = WORDS[next() % WORDS.len()];
            let label = format!("{first} {second} #{i}");
            Row::new(RowId::new(content_hash_hex(&label)), label)
        })
        .collect()
}
