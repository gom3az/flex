//! `flex theme` cutover tests (M3): provider rows, key-seq replays, theme
//! golden, `FLEX_TEST` determinism, and wrapper dispatch stubs.
//!
//! Parity is against the `pick()` path of
//! `scripts/.config/scripts/theme-switcher.sh` (only that path is cut
//! over; `list`/`current`/`activate`/`delete` stay in bash — see
//! `src/providers/theme_.rs`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::{backend, run, width, Menu};
use flex_rice::providers::theme_;

fn fixture_entries() -> Vec<theme_::ThemeEntry> {
    vec![
        theme_::ThemeEntry {
            name: "catppuccin-mocha".to_string(),
            wallpaper: "mocha-wall.png".to_string(),
            active: true,
        },
        theme_::ThemeEntry {
            name: "tokyo-night".to_string(),
            wallpaper: "tokyo.jpg".to_string(),
            active: false,
        },
        theme_::ThemeEntry {
            name: "bare".to_string(),
            wallpaper: "(no metadata)".to_string(),
            active: false,
        },
    ]
}

fn fixture_menu() -> Menu {
    flex_rice::menu(
        theme_::PROVIDER,
        vec![theme_::tab_from_entries(&fixture_entries())],
    )
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

// --- Provider fixtures ------------------------------------------------------

#[test]
fn rows_carry_theme_names_wallpapers_and_active_marker() {
    let tab = theme_::tab_from_entries(&fixture_entries());
    assert_eq!(tab.name, theme_::TAB_NAME);
    assert!(tab.bare_rows, "themes use bare rows");
    assert!(!tab.deletable, "theme rows are non-deletable");
    let labels: Vec<(&str, Option<&str>)> = tab
        .rows
        .iter()
        .map(|row| (row.label.as_str(), row.meta.as_deref()))
        .collect();
    assert_eq!(
        labels,
        vec![
            ("catppuccin-mocha", Some("mocha-wall.png  Active")),
            ("tokyo-night", Some("tokyo.jpg")),
            ("bare", Some("(no metadata)")),
        ]
    );
    // The id is a space-free hash of the theme name (B-021), and every row
    // id resolves back to its own label through the same directory scan
    // `flex-theme.sh` performs before calling `activate`.
    let available = scratch("row-ids");
    for row in &tab.rows {
        std::fs::create_dir_all(available.join(row.label.as_str())).expect("theme dir");
    }
    for row in &tab.rows {
        assert!(
            !row.id.as_str().contains(char::is_whitespace),
            "id is a single token: {:?}",
            row.id.as_str()
        );
        assert_eq!(
            theme_::resolve_name_in(&available, row.id.as_str()).as_deref(),
            Some(row.label.as_str()),
            "row id resolves back to the theme name"
        );
    }
    std::fs::remove_dir_all(&available).expect("cleanup");
}

/// Unique scratch dir per call (tests run in parallel; B-007/B-013).
fn scratch(name: &str) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "flex-theme-test-{}-{name}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn scan_reads_sorted_dirs_and_current_marker() {
    let root = std::env::temp_dir().join(format!("flex-theme-test-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let available = root.join("available");
    let current = root.join("current");
    for theme in ["zulu", "alpha"] {
        let dir = available.join(theme);
        std::fs::create_dir_all(&dir).expect("theme dir");
        std::fs::write(
            dir.join("metadata.json"),
            format!("{{\"theme_name\": \"{theme}\", \"wallpaper\": \"/walls/{theme}.png\"}}"),
        )
        .expect("metadata");
    }
    // Theme without metadata degrades to the bash fallback label.
    std::fs::create_dir_all(available.join("plain")).expect("plain dir");
    std::fs::create_dir_all(&current).expect("current dir");
    std::fs::write(current.join("metadata.json"), "{\"theme_name\": \"alpha\"}").expect("current");
    let entries = theme_::scan_available(&available, &theme_::current_name_in(&current));
    let _ = std::fs::remove_dir_all(&root);
    let rows: Vec<(&str, &str, bool)> = entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.wallpaper.as_str(), entry.active))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("alpha", "alpha.png", true),
            ("plain", "(no metadata)", false),
            ("zulu", "zulu.png", false),
        ],
        "sorted dirs (bash find|sort), wallpaper basenames, active mark"
    );
}

#[test]
fn missing_dirs_yield_no_rows_without_panicking() {
    let missing =
        std::env::temp_dir().join(format!("flex-theme-test-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    assert!(theme_::scan_available(&missing, "").is_empty());
    assert_eq!(theme_::current_name_in(&missing), "");
}

#[test]
fn default_focus_is_row_zero() {
    let menu = fixture_menu();
    assert_eq!(menu.app.active_tab().expect("tab").state.focus, 0);
}

// --- Key-seq replays ----------------------------------------------------------

#[test]
fn keyseq_down_enter_selects_second_theme() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Down), press(KeyCode::Enter)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select);
    let row = menu.app.focused_row().expect("focused row");
    assert_eq!(row.label, "tokyo-night");
    assert!(
        !row.id.as_str().contains(char::is_whitespace),
        "row id is a single hash token: {:?}",
        row.id.as_str()
    );
    assert_ne!(row.id.as_str(), row.label, "the name is not the id (B-021)");
    // The wrapper's ACTION: line for this selection is
    // `ACTION: theme <row-hash> tokyo-night` (exit 0); `flex-theme.sh`
    // resolves the hash with `flex theme --resolve` before activating.
    assert_eq!(menu.provider, "theme");
}

#[test]
fn keyseq_filter_then_enter_selects_active_theme() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[
            press(KeyCode::Char('c')),
            press(KeyCode::Char('a')),
            press(KeyCode::Char('t')),
            press(KeyCode::Enter),
        ],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select);
    let row = menu.app.focused_row().expect("focused row");
    assert_eq!(row.label, "catppuccin-mocha");
    assert_ne!(row.id.as_str(), row.label, "the name is not the id (B-021)");
}

#[test]
fn keyseq_esc_cancels_with_no_action() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], base);
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
}

#[test]
fn delete_never_fires_on_theme_rows() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Delete), press(KeyCode::Delete)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Consumed, "Delete is dead on theme");
    assert!(
        !menu.app.active_state().expect("state").confirm_pending,
        "no confirm arms on non-deletable tabs"
    );
}

// --- Theme golden --------------------------------------------------------------

/// Concatenate non-skip symbols of row `y` (exact cell content, no ANSI).
fn row_text(buf: &ratatui::buffer::Buffer, y: u16, w: u16) -> String {
    let mut out = String::new();
    for x in 0..w {
        let cell = buf.cell((x, y)).expect("cell in frame");
        if !cell.skip {
            out.push_str(cell.symbol());
        }
    }
    out
}

#[test]
fn theme_default_view_golden_at_80x24() {
    let mut menu = fixture_menu();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    terminal
        .draw(|frame| flex_core::render::render(frame, &mut menu))
        .expect("render frame");
    let buf = terminal.backend().buffer().clone();
    // Every row is exactly the frame width in display cells.
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
    // Bare mode: list rows start below the reserved `•••` indicator line
    // (`LIST_INDICATOR_ROWS`), no tab bar, no filter, no meta.
    let first_y = flex_core::render::LIST_INDICATOR_ROWS / 2;
    assert!(
        row_text(&buf, first_y, 80).starts_with('░'),
        "row 0 is the first list row"
    );
    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(!all.contains('›'), "no filter line");
    assert!(
        all.contains("catppuccin-mocha"),
        "theme label visible: {all:?}"
    );
}

// --- FLEX_TEST seed extension -----------------------------------------------------

#[test]
fn flex_test_replays_are_deterministic_across_bases() {
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
    let mut first = fixture_menu();
    let mut second = fixture_menu();
    assert_eq!(run_script(&mut first), run_script(&mut second));
    assert_eq!(
        first.app.focused_row().expect("row").id,
        second.app.focused_row().expect("row").id
    );
}

// --- Wrapper stub tests -----------------------------------------------------------

#[test]
fn wrapper_dispatches_activate_to_theme_switcher() {
    let dir = std::env::temp_dir().join(format!("flex-theme-test-wrap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    let flex = dir.join("flex");
    // The stub answers both calls the wrapper makes: the `ACTION:` line and
    // the `--resolve` lookup that turns the row hash back into the theme
    // name. The name deliberately contains a space, which is exactly what
    // hashing the id makes survivable (B-021).
    std::fs::write(
        &flex,
        "#!/usr/bin/env bash\nif [[ \"${2:-}\" == \"--resolve\" ]]; then\nprintf '%s\\n' 'Tokyo Night'\nexit 0\nfi\nprintf '%s\\n' 'ACTION: theme 0123456789abcdef Tokyo Night'\n",
    )
    .expect("flex stub");
    let switcher = dir.join("theme-switcher.sh");
    let log = dir.join("calls.log");
    // Bracket every argv element so a name split across two arguments
    // cannot masquerade as one.
    std::fs::write(
        &switcher,
        format!(
            "#!/usr/bin/env bash\n{{ for arg in \"$@\"; do printf '[%s]' \"$arg\"; done; printf '\\n'; }} >> \"{}\"\n",
            log.display()
        ),
    )
    .expect("switcher stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for path in [&flex, &switcher] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    let wrapper = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-theme.sh");
    let path_env = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = std::process::Command::new("bash")
        .arg(&wrapper)
        .env("PATH", path_env)
        .env("THEME_SWITCHER", &switcher)
        // The popup re-exec is bind-path behavior; wrapper tests emulate
        // the in-popup half.
        .env("POPUP_KITTY", "1")
        .output()
        .expect("run wrapper");
    assert!(
        output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let logged = std::fs::read_to_string(&log).expect("call log");
    assert_eq!(
        logged.trim(),
        "[activate][Tokyo Night]",
        "the resolved name reaches `activate` as ONE argument"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_rejects_malformed_action_lines() {
    let dir = std::env::temp_dir().join(format!("flex-theme-test-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    let flex = dir.join("flex");
    std::fs::write(&flex, "#!/usr/bin/env bash\necho 'GARBAGE LINE'\n").expect("flex stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&flex, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let wrapper = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-theme.sh");
    let path_env = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = std::process::Command::new("bash")
        .arg(&wrapper)
        .env("PATH", path_env)
        .env("THEME_SWITCHER", dir.join("theme-switcher.sh"))
        .env("POPUP_KITTY", "1")
        .output()
        .expect("run wrapper");
    assert!(!output.status.success(), "malformed ACTION: must fail");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Empty-scan placeholder (B-026) -------------------------------------------

/// Same policy as `launch`: an empty theme directory shows a `noop`
/// placeholder row instead of a blank menu.
#[test]
fn empty_scan_shows_the_noop_placeholder() {
    let tab = theme_::tab_from_entries(&[]);
    assert_eq!(tab.name, theme_::TAB_NAME);
    assert_eq!(tab.rows.len(), 1, "placeholder instead of a blank menu");
    assert_eq!(tab.rows[0].id.as_str(), flex_rice::providers::NOOP_ID);
    assert_eq!(tab.rows[0].label, theme_::NO_THEMES_LABEL);

    let mut menu = flex_rice::menu(theme_::PROVIDER, vec![tab]);
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Enter)], run::test_base());
    assert_eq!(outcome, KeyOutcome::Select);
    assert_eq!(
        menu.app.focused_row().expect("focused row").id.as_str(),
        flex_rice::providers::NOOP_ID
    );
}

/// The placeholder's `noop` id must never reach `theme-switcher.sh`
/// (B-026): the wrapper exits 0 before resolving.
#[test]
fn wrapper_treats_the_noop_placeholder_as_a_noop() {
    let dir = scratch("wrapper-noop");
    std::fs::create_dir_all(&dir).expect("stub dir");
    let flex = dir.join("flex");
    std::fs::write(
        &flex,
        "#!/usr/bin/env bash\nif [[ \"${2:-}\" == \"--resolve\" ]]; then\nprintf 'resolved\\n' >> \"$STUB_RESOLVE_LOG\"\nprintf '%s\\n' 'Tokyo Night'\nexit 0\nfi\nprintf '%s\\n' 'ACTION: theme noop (No themes found)'\n",
    )
    .expect("flex stub");
    let switcher = dir.join("theme-switcher.sh");
    let log = dir.join("calls.log");
    std::fs::write(
        &switcher,
        format!(
            "#!/usr/bin/env bash\nprintf 'called\\n' >> '{}'\n",
            log.display()
        ),
    )
    .expect("switcher stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for path in [&flex, &switcher] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    let wrapper = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-theme.sh");
    let resolve_log = dir.join("resolve.log");
    let output = std::process::Command::new("bash")
        .arg(&wrapper)
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("THEME_SWITCHER", &switcher)
        .env("STUB_RESOLVE_LOG", &resolve_log)
        .env("POPUP_KITTY", "1")
        .output()
        .expect("run wrapper");
    assert!(
        output.status.success(),
        "placeholder selection is not an error: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !resolve_log.exists(),
        "noop short-circuits before resolving"
    );
    assert!(!log.exists(), "theme-switcher.sh is never called");
    let _ = std::fs::remove_dir_all(&dir);
}
