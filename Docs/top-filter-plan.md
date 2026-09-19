# Plan: Filter Input on Top (Filterable Screens Only)

**Status:** Implemented & Verified
**Target:** `flex-core/src/render.rs` — single-file geometry change, zero provider edits

---

## Objective

Move the filter/search line from the bottom chrome to the top of the frame on every tab where `filterable == true`. Fixed-choice tabs (`filterable == false`: power, shot capture, profile, notify center, rec-option submenus) remain unchanged — no filter line, no caret, typing still consumed.

Visual result on filterable tabs (e.g. `flex-launch`, `flex-clip`, `flex-wifi`, `flex-bt`, `flex-proc`, `flex-theme`, `flex-wallpaper`, `flex-net` bandwidth/interfaces):

```
+---------------------------+
| › Search                  |
+---------------------------+
| ░ App One                 |
| ▒ App Two                 |
| ░ App Three               |
|                           |
| •••                       |
+---------------------------+
| j/k move  h/l switch tab  |
+---------------------------+
| [Launch]  Clip  Wifi  ... |
+---------------------------+
```

Placeholder text: **"Search"** (replaces `"filter…"`).

---

## Scope

**Files changed:** `flex-core/src/render.rs` only.

**No changes to:** `filter.rs`, `keys.rs`, `lib.rs`, `run.rs`, any provider, any binary.

---

## Geometry Changes

### `FrameLayout` struct (new field)

```rust
struct FrameLayout {
    list_h: u16,
    chrome_top: u16,      // NEW: 1 on filterable tabs, 0 otherwise
    bare: bool,
    filterable: bool,
    gauge_on: bool,
    has_tabs: bool,
}
```

### `frame_layout()` — split chrome into top + bottom

```rust
fn frame_layout(area: Rect, menu: &Menu) -> FrameLayout {
    let bare = menu.app.active_tab().is_some_and(|tab| tab.bare_rows);
    let filterable = menu.app.active_tab().is_some_and(|tab| tab.filterable);
    let gauge_on = !bare && menu.gauge.is_some();
    let has_tabs = menu.app.tabs.len() > 1;

    // Top chrome: filter line only on filterable, non-bare tabs
    let chrome_top: u16 = if bare || !filterable { 0 } else { 1 };

    // Bottom chrome: hints + tab bar (+ gauge if present)
    let chrome_bottom: u16 = if bare {
        if has_tabs { TAB_BAR_HEIGHT } else { 0 }
    } else if gauge_on {
        3 + TAB_BAR_HEIGHT       // gauge(1) + hints(1) + tab_bar(1)
    } else if filterable {
        2 + TAB_BAR_HEIGHT       // hints(1) + tab_bar(1)  ← filter line moved up
    } else {
        1 + TAB_BAR_HEIGHT       // hints(1) + tab_bar(1)
    };

    let list_h = usize::from(area.height).saturating_sub(usize::from(chrome_top + chrome_bottom));
    FrameLayout { list_h: u16::try_from(list_h).unwrap_or(u16::MAX), chrome_top, bare, filterable, gauge_on, has_tabs }
}
```

**Invariant:** `chrome_top + list_h + chrome_bottom == area.height` (saturating). Total list entry capacity unchanged.

### `render()` — draw order

1. `draw_filter` at `y = area.y` (top line) when `filterable && !bare`
2. `list_area` origin shifts from `area.y` → `area.y + chrome_top`; height = `list_h`
3. `draw_list` receives shifted `list_area` (top `•••` indicator moves with it)
4. `draw_gauge` / `draw_hints` / `draw_tab_bar` at bottom — unchanged y-math (they already compute from `area.y + area.height - …`)
5. `draw_dropdown` / `draw_help` use `list_area` — automatically shifted

### `preview_area()` — pane follows list

```rust
pub fn preview_area(area: Rect, menu: &Menu) -> Option<Rect> {
    let layout = frame_layout(area, menu);
    let mut list_area = Rect::new(area.x, area.y + layout.chrome_top, area.width, layout.list_h);
    preview::pane(list_area, layout.list_h, menu.preview)
}
```

The pane shares the list's vertical extent (now starting at `chrome_top`), keeping the 1-cell gutter and width math identical.

### `cursor_position()` — caret on top line

```rust
pub fn cursor_position(area: Rect, menu: &Menu) -> Option<(u16, u16)> {
    let filter_w = menu.app.active_tab().map_or(0, |tab| width::str_width(&tab.state.filter));
    let chrome_top = frame_layout(area, menu).chrome_top;  // 1 or 0
    let y = area.y + chrome_top;  // filter line lives here
    let x = area.x.saturating_add(2).saturating_add(u16::try_from(filter_w).unwrap_or(0));
    if x < area.x + area.width && y < area.y + area.height {
        Some((x, y))
    } else { None }
}
```

Hidden condition `bare || !filterable` (caller) unchanged.

---

## Constant Change

`FILTER_EMPTY_TEXT` (render.rs:138): `"filter…"` → `"Search"`. Single use site (`draw_filter:1466`).

---

## Tests to Update

| File | Test | Change |
|------|------|--------|
| `flex-core/tests/compliance.rs:91` | `entry_pitch_matches_the_node_metrics` | First row header `y=1` → `y=2` (filter line at `y=0`, `LIST_INDICATOR_ROWS=1` above list) |
| `flex-core/tests/compliance.rs:115-116` | `viewport_counts_entries` | Comment: "list height 21 (filter + hints + tab bar chrome) minus 2 indicator rows = 19" → unchanged total, origin shifted |
| `flex-rice/tests/wallpaper.rs:304` | `render_reports_the_reserved_pane_only_when_asked` | `Rect::new(44, 0, 36, 21)` → `Rect::new(44, 1, 36, 21)` (pane `y` follows `chrome_top=1`) |
| `flex-rice/tests/wallpaper.rs:356` | `pane_needs_a_wide_enough_frame` | `Rect::new(69, 0, 56, 27)` → `Rect::new(69, 1, 56, 27)` |
| `flex-core/tests/keys.rs` | `filter_of` / `cursor_position` helpers | No logic change; assertions on `filter` string content unaffected |

**New test** (compliance.rs): filterable tab draws `› Search` at `y=0`; non-filterable tab (power) draws no `›` anywhere.

---

## Gates

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./setup.sh --check   # 15/15 relinked
```

Live eyeball: `flex-launch` → filter line at top, `› Search` placeholder, type to filter works, `Esc` clears, `q` quits only when filter empty. `flex-power` → no filter line, `q` quits immediately.

---

## Rollback

Single commit on `flex-core/src/render.rs` + test expectation updates. `git revert` restores prior geometry exactly.