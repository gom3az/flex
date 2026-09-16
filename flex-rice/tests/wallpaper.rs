//! `flex wallpaper` cutover tests: provider rows, scan parity with the bash
//! `find … | sort -u` pipeline, preview-pane geometry, key-seq replays,
//! goldens at 80x24, `FLEX_TEST` determinism and wrapper dispatch stubs.
//!
//! Parity is against `scripts/.config/scripts/wallpaper-picker.sh` (the fzf +
//! kitty-icat picker this replaces): same roots, same `-maxdepth 2 -iname`
//! filter, same label (`basename`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::atomic::{AtomicU64, Ordering};

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::preview;
use flex_core::{backend, render, run, width, Menu, Row};
use flex_rice::providers::wallpaper;

/// Long name: wider than the list column once the preview pane is reserved,
/// so truncation proves where the list ends.
const LONG_NAME: &str = "a-very-long-wallpaper-name-that-crosses-the-pane-column.jpg";

/// Per-call sequence: cargo runs tests in parallel, so a fixed scratch name
/// would let sibling tests delete each other's fixtures (B-007 class).
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Unique scratch directory for one test.
fn scratch(name: &str) -> std::path::PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "flex-wallpaper-test-{}-{seq}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Two wallpapers in `Wallpapers`, one in `Screenshots` (bash root order).
fn fixture_entries(root: &std::path::Path) -> Vec<wallpaper::WallpaperEntry> {
    let walls = root.join("Pictures/Wallpapers");
    let shots = root.join("Pictures/Screenshots");
    std::fs::create_dir_all(&walls).expect("walls dir");
    std::fs::create_dir_all(&shots).expect("shots dir");
    let active = walls.join("sunset.jpg");
    wallpaper::scan(&[walls, shots], Some(&active))
}

fn write_image(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, [0xff, 0xd8, 0xff, 0xe0]).expect("write image");
}

fn fixture_menu(preview_on: bool) -> Menu {
    let root = scratch(if preview_on {
        "fixture-on"
    } else {
        "fixture-off"
    });
    write_image(&root.join("Pictures/Wallpapers/sunset.jpg"));
    write_image(&root.join("Pictures/Wallpapers").join(LONG_NAME));
    write_image(&root.join("Pictures/Screenshots/grim-2026-09-15.png"));
    let entries = fixture_entries(&root);
    let mut menu = flex_rice::menu(
        wallpaper::PROVIDER,
        vec![wallpaper::tab_from_entries(&entries)],
    );
    menu.preview = preview_on;
    // Rows keep absolute paths as strings; rendering and key handling never
    // touch the files, so the fixture tree can go away with the menu built.
    let _ = std::fs::remove_dir_all(&root);
    menu
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Filesystem-free entries (paths need not exist for row/key tests), sorted
/// the way [`wallpaper::scan`] sorts.
fn synthetic_entries() -> Vec<wallpaper::WallpaperEntry> {
    [
        ("/screens/grim-2026-09-15.png", "Screenshots", false),
        ("/walls/night.png", "Wallpapers", false),
        ("/walls/sunset.jpg", "Wallpapers", true),
    ]
    .into_iter()
    .map(|(path, dir, active)| wallpaper::WallpaperEntry {
        path: std::path::PathBuf::from(path),
        name: std::path::Path::new(path)
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned(),
        dir: dir.to_string(),
        active,
    })
    .collect()
}

/// Symbols of row `y`, skipping ratatui's continuation cells.
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

fn draw(menu: &mut Menu, w: u16, h: u16) -> (Buffer, Option<Rect>) {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    let mut area = Rect::default();
    terminal
        .draw(|frame| {
            area = frame.area();
            render::render(frame, menu);
        })
        .expect("render frame");
    let pane = render::preview_area(area, menu);
    (terminal.backend().buffer().clone(), pane)
}

// --- Provider rows ----------------------------------------------------------

#[test]
fn rows_carry_hash_ids_labels_meta_and_preview_paths() {
    let root = scratch("rows");
    write_image(&root.join("Pictures/Wallpapers/sunset.jpg"));
    write_image(&root.join("Pictures/Screenshots/grim.png"));
    let walls = root.join("Pictures/Wallpapers");
    let shots = root.join("Pictures/Screenshots");
    let active = walls.join("sunset.jpg");
    let entries = wallpaper::scan(&[walls.clone(), shots.clone()], Some(&active));
    let rows = wallpaper::rows(&entries);
    let _ = std::fs::remove_dir_all(&root);

    let shapes: Vec<(&str, &str, Option<&str>, Option<&str>)> = rows
        .iter()
        .map(|row| {
            (
                row.id.as_str(),
                row.label.as_str(),
                row.meta.as_deref(),
                row.preview_image.as_deref(),
            )
        })
        .collect();
    assert_eq!(shapes.len(), 2);
    // Sorted by full path, like the bash `sort -u` (`Screenshots` < `Wallpapers`).
    assert_eq!(
        shapes[0],
        (
            wallpaper::entry_id(&shots.join("grim.png")).as_str(),
            "grim.png",
            Some("Screenshots"),
            Some(shots.join("grim.png").to_string_lossy().as_ref()),
        )
    );
    assert_eq!(
        shapes[1],
        (
            wallpaper::entry_id(&walls.join("sunset.jpg")).as_str(),
            "sunset.jpg",
            Some("Wallpapers  Active"),
            Some(walls.join("sunset.jpg").to_string_lossy().as_ref()),
        ),
        "the wallpaper in use is marked like the theme provider, and previews"
    );
    // Ids are 16-char hex: safe as space-delimited `ACTION:` fields.
    for row in &rows {
        assert_eq!(row.id.as_str().len(), 16);
        assert!(row.id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }
}

#[test]
fn tab_is_standard_non_deletable_and_preview_carrying() {
    let menu = fixture_menu(false);
    let tab = menu.app.active_tab().expect("tab");
    assert_eq!(tab.name, wallpaper::TAB_NAME);
    assert_eq!(wallpaper::TAB_NAME, "Wallpapers");
    assert!(
        !tab.bare_rows,
        "meta column + tab bar are part of the design"
    );
    assert!(tab.filterable, "type-to-filter like every list provider");
    assert!(
        !tab.deletable,
        "wallpapers are never deleted from the picker"
    );
    assert!(
        tab.rows.iter().all(|row| row.preview_image.is_some()),
        "every row can drive the preview pane"
    );
}

#[test]
fn scan_matches_the_bash_find_pipeline() {
    let root = scratch("scan");
    let walls = root.join("Wallpapers");
    let nested = walls.join("nature");
    std::fs::create_dir_all(&nested).expect("nested dir");
    std::fs::create_dir_all(walls.join("deeper/still")).expect("deep dir");
    // `-iname` is case-insensitive and covers four suffixes.
    for name in [
        "b.PNG",
        "a.jpg",
        "c.jpeg",
        "d.WebP",
        "notes.txt",
        "e.gif",
        "noext",
    ] {
        std::fs::write(walls.join(name), b"x").expect("write");
    }
    std::fs::write(nested.join("deep.png"), b"x").expect("write");
    std::fs::write(walls.join("deeper/still/too-deep.png"), b"x").expect("write");
    // `find -type f` does not follow symlinks: a linked image is skipped.
    std::os::unix::fs::symlink(nested.join("deep.png"), walls.join("link.jpg")).expect("symlink");

    let entries = wallpaper::scan(std::slice::from_ref(&walls), None);
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        names,
        vec!["a.jpg", "b.PNG", "c.jpeg", "d.WebP", "deep.png"],
        "sorted by path, depth 1 + 2 only, symlinks and non-images skipped"
    );
}

#[test]
fn duplicate_roots_and_repeated_files_are_deduped() {
    let root = scratch("dedup");
    let walls = root.join("Wallpapers");
    write_image(&walls.join("only.png"));
    let entries = wallpaper::scan(&[walls.clone(), walls.clone()], None);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(entries.len(), 1, "bash `sort -u` semantics");
}

#[test]
fn missing_roots_yield_no_rows_without_panicking() {
    let missing = scratch("missing").join("nope");
    assert!(wallpaper::scan(std::slice::from_ref(&missing), None).is_empty());
    assert!(wallpaper::resolve_in(&[missing], "0123456789abcdef").is_none());
}

#[test]
fn resolve_round_trips_ids_to_paths() {
    let root = scratch("resolve");
    let walls = root.join("Wallpapers");
    let image = walls.join("with space.jpg");
    write_image(&image);
    let entries = wallpaper::scan(std::slice::from_ref(&walls), None);
    let id = wallpaper::entry_id(&entries[0].path);
    assert_eq!(
        wallpaper::resolve_in(std::slice::from_ref(&walls), &id).as_deref(),
        Some(image.as_path()),
        "the wrapper's hidden lookup returns the absolute path"
    );
    assert!(wallpaper::resolve_in(&[walls], "deadbeefdeadbeef").is_none());

    // A file that vanished since the menu was drawn resolves to nothing.
    std::fs::remove_file(&image).expect("remove");
    assert!(wallpaper::resolve_in(std::slice::from_ref(&root), &id).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn current_wallpaper_falls_back_to_hyprpaper_conf() {
    // The state file is exercised through `WALLPAPER_STATE` in production
    // only; the conf parser is the pure fallback and is pinned here.
    assert_eq!(
        wallpaper::parse_hyprpaper_conf("preload = /p/a.jpg\nwallpaper = ,/w/b.png\n"),
        Some(std::path::PathBuf::from("/w/b.png"))
    );
    assert_eq!(wallpaper::parse_hyprpaper_conf(""), None);
}

#[test]
fn default_focus_is_row_zero() {
    let menu = fixture_menu(false);
    assert_eq!(menu.app.active_tab().expect("tab").state.focus, 0);
}

// --- Preview pane plumbing --------------------------------------------------

#[test]
fn render_reports_the_reserved_pane_only_when_asked() {
    let mut with = fixture_menu(true);
    let (_, pane) = draw(&mut with, 80, 24);
    assert_eq!(
        pane,
        Some(Rect::new(44, 0, 36, 21)),
        "80 wide: 45% pane right-aligned, sharing the 21-row list area"
    );

    let mut without = fixture_menu(false);
    let (_, pane) = draw(&mut without, 80, 24);
    assert_eq!(pane, None, "non-wallpaper menus keep the full-width list");
}

#[test]
fn pane_region_stays_blank_and_the_list_narrows() {
    let mut menu = fixture_menu(true);
    let (buf, pane) = draw(&mut menu, 80, 24);
    let pane = pane.expect("pane");
    for y in 0..pane.height {
        let cells = row_text(&buf, y, 80);
        assert!(
            cells[pane.x as usize..].trim().is_empty(),
            "row {y} must leave the image pane empty: {cells:?}"
        );
    }
    // The chrome below keeps the full frame width.
    assert!(
        row_text(&buf, 23, 80).contains("[Wallpapers]"),
        "tab bar spans the frame"
    );

    // The long name is truncated inside the narrowed list…
    let all: String = (0..21).map(|y| row_text(&buf, y, 80)).collect();
    assert!(
        !all.contains(LONG_NAME),
        "label does not cross into the pane"
    );

    // …and fits when no pane is reserved.
    let mut wide = fixture_menu(false);
    let (buf, pane) = draw(&mut wide, 80, 24);
    assert_eq!(pane, None);
    let all: String = (0..21).map(|y| row_text(&buf, y, 80)).collect();
    assert!(
        all.contains(LONG_NAME),
        "without a pane the list uses the whole width"
    );
}

#[test]
fn pane_needs_a_wide_enough_frame() {
    let mut menu = fixture_menu(true);
    let (_, pane) = draw(&mut menu, 40, 24);
    assert_eq!(pane, None, "40 columns: list keeps priority");
    let mut menu = fixture_menu(true);
    let (_, pane) = draw(&mut menu, 125, 30);
    assert_eq!(pane, Some(Rect::new(69, 0, 56, 27)));
}

#[test]
fn preview_split_is_pure_geometry() {
    // No terminal involved: the same math `render` uses.
    let area = Rect::new(0, 0, 125, 30);
    assert_eq!(
        preview::pane(area, 27, true),
        Some(Rect::new(69, 0, 56, 27))
    );
    assert_eq!(preview::pane(area, 27, false), None);
    // The gutter column sits between the rows and the image.
    let pane = preview::pane(area, 27, true).expect("pane");
    assert_eq!(pane.x - 1, 68, "one gutter column");
}

// --- Key-seq replays --------------------------------------------------------

#[test]
fn keyseq_down_enter_selects_the_second_wallpaper() {
    let mut menu = fixture_menu(true);
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Down), press(KeyCode::Enter)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select);
    let row = menu.app.focused_row().expect("focused row");
    assert!(std::path::Path::new(&row.label)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg")));
    assert_eq!(menu.provider, "wallpaper");
    // The focus moved, so the pane follows the new image.
    assert!(row.preview_image.is_some());
}

#[test]
fn keyseq_filter_then_enter_selects_the_screenshot() {
    let mut menu = fixture_menu(true);
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[
            press(KeyCode::Char('g')),
            press(KeyCode::Char('r')),
            press(KeyCode::Char('i')),
            press(KeyCode::Char('m')),
            press(KeyCode::Enter),
        ],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select);
    assert_eq!(
        menu.app.focused_row().expect("row").label,
        "grim-2026-09-15.png"
    );
}

#[test]
fn keyseq_esc_cancels_with_no_action() {
    let mut menu = fixture_menu(true);
    let base = run::test_base();
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], base);
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
}

#[test]
fn delete_never_fires_on_wallpaper_rows() {
    let mut menu = fixture_menu(false);
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Delete), press(KeyCode::Delete)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Consumed, "Delete is dead here");
    assert!(!menu.app.active_state().expect("state").confirm_pending);
}

#[test]
fn flex_test_replays_are_deterministic() {
    assert_ne!(backend::FLEX_TEST_SEED, 0);
    let script = [press(KeyCode::Down), press(KeyCode::Enter)];
    let run_script = |menu: &mut Menu| {
        let base = run::test_base();
        let mut outcomes = Vec::new();
        for (index, key) in script.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let step = index as u32;
            outcomes.push(handle_key(menu, *key, base + run::FLEX_TEST_STEP * step));
        }
        outcomes
    };
    // Same entries, two menus: ids and outcomes must match exactly.
    let entries = synthetic_entries();
    let mut first = flex_rice::menu(
        wallpaper::PROVIDER,
        vec![wallpaper::tab_from_entries(&entries)],
    );
    let mut second = flex_rice::menu(
        wallpaper::PROVIDER,
        vec![wallpaper::tab_from_entries(&entries)],
    );
    assert_eq!(run_script(&mut first), run_script(&mut second));
    assert_eq!(
        first.app.focused_row().expect("row").id,
        second.app.focused_row().expect("row").id
    );
}

// --- Goldens ----------------------------------------------------------------

/// Every row is exactly the frame width in display cells, and the layout is
/// the standard one: `•••` line, entries, filter line, hints, tab bar.
#[test]
fn wallpaper_default_view_golden_at_80x24() {
    let mut menu = fixture_menu(true);
    let (buf, _) = draw(&mut menu, 80, 24);
    for y in 0..24 {
        let mut x = 0_u16;
        let mut total = 0_usize;
        while x < 80 {
            let cell = buf.cell((x, y)).expect("cell in frame");
            assert!(!cell.skip, "dangling skip cell at ({x}, {y})");
            let symbol_w = width::str_width(cell.symbol());
            total += symbol_w;
            x += u16::try_from(symbol_w.max(1)).expect("row width fits u16");
        }
        assert_eq!(total, 80, "row {y} must be exactly 80 cells");
    }
    let first_y = render::LIST_INDICATOR_ROWS / 2;
    assert!(
        row_text(&buf, first_y, 80).starts_with('░'),
        "selected entry carries the selector at column 0"
    );
    let meta_row = row_text(&buf, first_y, 80);
    assert!(
        meta_row.contains("Wallpapers") || meta_row.contains("Screenshots"),
        "meta column shows the source directory: {meta_row:?}"
    );
    // Standard chrome is present (unlike the bare-rows providers).
    let all: String = (0..24).map(|y| row_text(&buf, y, 80)).collect();
    assert!(all.contains('›'), "filter line rendered");
    assert!(all.contains("[Wallpapers]"), "tab bar rendered");
}

#[test]
fn row_without_a_utf8_path_loses_only_its_preview() {
    let mut entries = vec![wallpaper::WallpaperEntry {
        path: std::path::PathBuf::from("/tmp/\u{fffd}.jpg"),
        name: "x.jpg".to_string(),
        dir: "Wallpapers".to_string(),
        active: false,
    }];
    let rows = wallpaper::rows(&entries);
    assert_eq!(rows[0].preview_image.as_deref(), Some("/tmp/\u{fffd}.jpg"));
    // A path that cannot be UTF-8 at all keeps its row, minus the preview.
    entries[0].path =
        std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/\xff.jpg".to_vec()));
    let rows: Vec<Row> = wallpaper::rows(&entries);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "x.jpg");
    assert!(rows[0].preview_image.is_none());
}

// --- Wrapper dispatch stubs -------------------------------------------------

/// Scratch dir with a `flex` stub, a `set-wallpaper.sh` stub and a real image.
fn wrapper_harness(
    name: &str,
    action: &str,
    resolve_ok: bool,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let dir = scratch(name);
    let image = dir.join("Pictures/Wallpapers/pick me.jpg");
    write_image(&image);
    let log = dir.join("calls.log");
    let flex = dir.join("flex");
    let resolve = if resolve_ok {
        format!("echo '{}'", image.display())
    } else {
        "exit 1".to_string()
    };
    std::fs::write(
        &flex,
        format!(
            "#!/usr/bin/env bash\nif [[ \"${{2:-}}\" == \"--resolve\" ]]; then {resolve}; exit 0; fi\n\
             echo '{action}'\n"
        ),
    )
    .expect("flex stub");
    let setter = dir.join("set-wallpaper.sh");
    std::fs::write(
        &setter,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> \"{}\"\n",
            log.display()
        ),
    )
    .expect("setter stub");
    for path in [&flex, &setter] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    (dir, setter, log)
}

fn run_wrapper(dir: &std::path::Path, setter: &std::path::Path) -> std::process::Output {
    let wrapper = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-wallpaper.sh");
    let path_env = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    std::process::Command::new("bash")
        .arg(&wrapper)
        .env("PATH", path_env)
        .env("SET_WALLPAPER", setter)
        // The popup re-exec is bind-path behavior; these tests emulate the
        // in-popup half (same pattern as the other wrapper suites).
        .env("POPUP_KITTY", "1")
        .output()
        .expect("run wrapper")
}

#[test]
fn wrapper_resolves_the_id_and_sets_the_wallpaper() {
    let id = wallpaper::entry_id(std::path::Path::new("/x/Pictures/Wallpapers/pick me.jpg"));
    let (dir, setter, log) =
        wrapper_harness("wrap", &format!("ACTION: wallpaper {id} pick me.jpg"), true);
    let output = run_wrapper(&dir, &setter);
    assert!(
        output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let logged = std::fs::read_to_string(&log).expect("call log");
    assert_eq!(
        logged.trim(),
        dir.join("Pictures/Wallpapers/pick me.jpg")
            .display()
            .to_string(),
        "the resolved path reaches set-wallpaper.sh, spaces and all"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_rejects_malformed_action_lines_and_unresolvable_ids() {
    let (dir, setter, log) = wrapper_harness("wrap-bad", "GARBAGE LINE", true);
    let output = run_wrapper(&dir, &setter);
    assert!(!output.status.success(), "malformed ACTION: must fail");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unexpected output"),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!log.exists(), "nothing ran");
    let _ = std::fs::remove_dir_all(&dir);

    // A path-shaped id (space free but not a hash) is refused before the
    // resolve lookup: the wrapper never guesses.
    let (dir, setter, log) =
        wrapper_harness("wrap-id", "ACTION: wallpaper ../../etc/passwd x", true);
    let output = run_wrapper(&dir, &setter);
    assert!(!output.status.success(), "bad id must fail");
    assert!(String::from_utf8_lossy(&output.stderr).contains("bad id"));
    assert!(!log.exists());
    let _ = std::fs::remove_dir_all(&dir);

    // A well-formed id the provider cannot resolve is refused too.
    let (dir, setter, log) = wrapper_harness(
        "wrap-unknown",
        "ACTION: wallpaper 0123456789abcdef gone.jpg",
        false,
    );
    let output = run_wrapper(&dir, &setter);
    assert!(!output.status.success(), "unknown id must fail");
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown id"));
    assert!(!log.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Resolve error format (B-022) -----------------------------------------------

/// Same contract as `clip`: `main` owns the single `flex: error:` prefix,
/// so an unknown wallpaper id is reported without repeating it.
#[test]
fn unknown_resolve_id_is_reported_with_a_single_prefix() {
    let home = scratch("resolve-error");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex"))
        .args(["wallpaper", "--resolve", "deadbeef"])
        .env("HOME", &home)
        .env_remove("WALLPAPER_DIRS")
        .output()
        .expect("run flex wallpaper --resolve");
    let _ = std::fs::remove_dir_all(&home);
    assert!(!output.status.success(), "unknown id exits non-zero");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "flex: error: wallpaper: unknown id 'deadbeef'\n"
    );
}
