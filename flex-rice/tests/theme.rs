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

// --- Executor (`flex-rice/src/exec/theme.rs`) ----------------------------------
//
// The theme port of the `exec::shot` template: `ThemeAction::parse` validates
// the id, `execute` resolves the hash in-process (no `flex theme --resolve`
// subprocess) and runs `<switcher> activate <name>` through the
// `THEME_SWITCHER` seam. No TUI, no pty: stub `PATH` + scratch `HOME`.
//
// Like the shot suite, the popup guard itself lives in the shared runner
// (covered by `popup.rs`), with one binary-level re-exec probe below.

/// Write an executable stub script.
fn write_exe(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).expect("stub script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// Save/restore one process env var around an executor call (tests run in
/// parallel; only the theme executor reads these two keys).
struct EnvGuard {
    key: &'static str,
    old: Option<String>,
}

/// Serialises the executor tests below: each mutates `HOME`/`THEME_SWITCHER`
/// (the seams `execute` reads) and the harness runs tests in parallel, so
/// the mutations are held under one lock with restores in [`EnvGuard`].
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn set_env(key: &'static str, value: &std::path::Path) -> EnvGuard {
    let old = std::env::var(key).ok();
    std::env::set_var(key, value);
    EnvGuard { key, old }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Scratch `HOME` with `available/<theme>/` dirs (one with metadata, one
/// space-bearing per B-021, one bare).
fn install_theme_home(tag: &str) -> std::path::PathBuf {
    let home = scratch(&format!("exec-home-{tag}"));
    let available = home.join(".config/themes/available");
    for theme in ["alpha", "Tokyo Night", "plain"] {
        std::fs::create_dir_all(available.join(theme)).expect("theme dir");
    }
    std::fs::write(
        available.join("alpha").join("metadata.json"),
        "{\"theme_name\": \"alpha\", \"wallpaper\": \"/walls/alpha.png\"}",
    )
    .expect("metadata");
    home
}

/// `THEME_SWITCHER` stub logging every argv element bracketed (the existing
/// dispatch idiom: a split name cannot masquerade as one argument).
fn install_switcher(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let switcher = dir.join(name);
    let log = dir.join(format!("{name}.log"));
    write_exe(
        &switcher,
        &format!(
            "#!/usr/bin/env bash\n{{ for arg in \"$@\"; do printf '[%s]' \"$arg\"; done; printf '\\n'; }} >> \"{}\"\n",
            log.display()
        ),
    );
    switcher
}

fn stub_path_env(dir: &std::path::Path) -> String {
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// Full action-id matrix: every theme (including the space-bearing B-021
/// name) activates through the switcher seam as one `activate` call.
#[test]
fn executor_matrix_activates_every_theme_through_the_switcher_seam() {
    let _env = ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-matrix");
    std::fs::create_dir_all(&dir).expect("stub dir");
    let home = install_theme_home("matrix");
    let switcher = install_switcher(&dir, "theme-switcher.sh");
    let _home = set_env("HOME", &home);
    let _switcher_env = set_env("THEME_SWITCHER", &switcher);
    let path_env = stub_path_env(&dir);
    for name in ["alpha", "Tokyo Night", "plain"] {
        let id = theme_::entry_id(name);
        let report =
            flex_rice::exec::theme::execute(&id, Some(&path_env)).expect("activate succeeds");
        assert_eq!(report.name.as_deref(), Some(name));
        assert_eq!(report.switcher.as_deref(), Some(switcher.as_path()));
    }
    let log = std::fs::read_to_string(dir.join("theme-switcher.sh.log")).expect("call log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(
        lines,
        vec![
            "[activate][alpha]",
            "[activate][Tokyo Night]",
            "[activate][plain]",
        ],
        "one activate call per id, names intact (describe: `<switcher> activate <name>`)",
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// The `noop` placeholder exits 0 before resolving or calling (B-026): even
/// a missing `HOME` cannot fail it, like the wrapper's early exit.
#[test]
fn executor_noop_short_circuits_before_the_switcher() {
    let _env = ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-noop");
    std::fs::create_dir_all(&dir).expect("stub dir");
    let missing_home = dir.join("no-such-home");
    let switcher = install_switcher(&dir, "theme-switcher.sh");
    let _home = set_env("HOME", &missing_home);
    let _switcher_env = set_env("THEME_SWITCHER", &switcher);
    let report =
        flex_rice::exec::theme::execute("noop", Some(&stub_path_env(&dir))).expect("noop is Ok");
    assert_eq!(
        report,
        flex_rice::exec::theme::ExecuteReport {
            switcher: None,
            name: None,
        }
    );
    assert!(
        !dir.join("theme-switcher.sh.log").exists(),
        "theme-switcher.sh is never called"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Malformed ids (`bad id`, mirroring the wrapper check) and well-formed but
/// unresolvable hashes (`unknown id`) are errors with no `flex:` prefix of
/// their own — the runner adds the single prefix at the binary boundary.
#[test]
fn executor_rejects_bad_and_unknown_ids_without_its_own_prefix() {
    let _env = ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-bad");
    std::fs::create_dir_all(&dir).expect("stub dir");
    let home = install_theme_home("bad");
    let switcher = install_switcher(&dir, "theme-switcher.sh");
    let _home = set_env("HOME", &home);
    let _switcher_env = set_env("THEME_SWITCHER", &switcher);
    let path_env = stub_path_env(&dir);
    let ghost_id = theme_::entry_id("ghost");
    let cases = [
        ("", "theme: bad id ''"),
        ("a/b", "theme: bad id 'a/b'"),
        ("a\nb", "theme: bad id 'a\nb'"),
        (ghost_id.as_str(), "theme: unknown id"),
    ];
    for (id, expected) in cases {
        let err = flex_rice::exec::theme::execute(id, Some(&path_env))
            .expect_err("bad/unknown id must fail");
        let message = format!("{err:#}");
        assert!(
            message.starts_with(expected),
            "unexpected message for {id:?}: {message:?}"
        );
        assert!(
            !message.contains("flex:"),
            "no runner prefix below the runner: {message:?}"
        );
    }
    assert!(
        !dir.join("theme-switcher.sh.log").exists(),
        "no id failure reaches the switcher"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// A failing switcher is a loud error, never a quiet cancel: theme has no
/// `slurp`-like user-cancellable step, so every tool failure exits non-zero
/// through the runner (the menu-cancel 130 lives in the binary's
/// `Outcome::Cancelled` arm, shared with shot, and needs a pty).
#[test]
fn executor_tool_failure_is_a_loud_error_not_a_quiet_cancel() {
    let _env = ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-fail");
    std::fs::create_dir_all(&dir).expect("stub dir");
    let home = install_theme_home("fail");
    let switcher = dir.join("theme-switcher.sh");
    write_exe(&switcher, "#!/usr/bin/env bash\nexit 3\n");
    let _home = set_env("HOME", &home);
    let _switcher_env = set_env("THEME_SWITCHER", &switcher);
    let id = theme_::entry_id("alpha");
    let err = flex_rice::exec::theme::execute(&id, Some(&stub_path_env(&dir)))
        .expect_err("a failing switcher must fail");
    assert_eq!(format!("{err:#}"), "theme: activate alpha failed");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// Binary level without a pty: the menu cannot start, so the binary exits 1
/// with exactly one `flex: error:` prefix (the runner owns it; executor
/// errors carry none). `setsid` detaches the controlling terminal so no
/// harness tty can satisfy the TUI init.
#[test]
fn binary_errors_carry_a_single_prefix() {
    let output = std::process::Command::new("setsid")
        .arg(env!("CARGO_BIN_EXE_flex-theme"))
        .env("POPUP_KITTY", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-theme without a pty");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "no stdout on error");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.starts_with("flex: error: "),
        "runner prefix first: {stderr:?}"
    );
    assert_eq!(
        stderr.matches("flex: error:").count(),
        1,
        "exactly one prefix: {stderr:?}"
    );
}

/// Outside a popup the binary re-execs into the `menu` popup (the shared
/// runner guard); the kitty spawn is asserted against a stub `PATH`.
#[test]
fn binary_outside_a_popup_reexecs_into_the_menu_popup() {
    let dir = scratch("exec-guard");
    std::fs::create_dir_all(&dir).expect("stub dir");
    let kitty_log = dir.join("kitty.log");
    write_exe(&dir.join("pgrep"), "#!/usr/bin/env bash\nexit 1\n");
    write_exe(
        &dir.join("kitty"),
        &format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > '{}.tmp'\nmv '{}.tmp' '{}'\n",
            kitty_log.display(),
            kitty_log.display(),
            kitty_log.display(),
        ),
    );
    let path_env = stub_path_env(&dir);
    let saved_path = std::env::var("PATH").ok();
    std::env::set_var("PATH", &path_env);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("scratch HOME");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex-theme"))
        .env("HOME", &home)
        .env_remove("POPUP_KITTY")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-theme outside a popup");
    match saved_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    assert!(
        output.status.success(),
        "toggle exits 0: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let logged = loop {
        if let Ok(body) = std::fs::read_to_string(&kitty_log) {
            break body.lines().map(str::to_string).collect::<Vec<_>>();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "kitty spawn never logged"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(
        logged.contains(&String::from("--class"))
            && logged.contains(&String::from("flex-menu"))
            && logged.contains(&String::from("POPUP_KITTY=1")),
        "re-exec uses the menu popup template: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Parity with the untouched wrapper --------------------------------------
//
// For every action the wrapper supports, the wrapper (under stub `PATH`)
// and the executor must produce byte-identical switcher call sequences.
// The one deliberate departure: the wrapper's `flex theme --resolve`
// subprocess is resolved in-process (same `theme_::resolve_name` scan the
// hidden `--resolve` lookup uses), so the wrapper makes one extra `flex`
// call the executor never spawns — asserted below.

/// `flex` stub answering both wrapper calls: the `ACTION:` line (`$STUB_ACTION`)
/// and the `--resolve` lookup over the test themes. Every invocation is
/// logged so the elided resolve subprocess stays visible.
fn write_parity_flex_stub(dir: &std::path::Path, themes: &[(&str, &str)]) {
    use std::fmt::Write as _;
    let mut arms = String::new();
    for (hash, name) in themes {
        let _ = writeln!(arms, "    \"{hash}\") printf '%s\\n' \"{name}\" ;;");
    }
    write_exe(
        &dir.join("flex"),
        &format!(
            "#!/usr/bin/env bash\necho \"flex $@\" >> \"{}/flex.log\"\nif [[ \"${{2:-}}\" == \"--resolve\" ]]; then\n  case \"${{3:-}}\" in\n{arms}    *) exit 1 ;;\n  esac\n  exit 0\nfi\nprintf '%s\\n' \"$STUB_ACTION\"\n",
            dir.display()
        ),
    );
}
/// Fixed parity inputs: stub dirs, the wrapper, and both switcher stubs.
/// One parity case runs the wrapper under stub `PATH`, then the executor
/// with the scratch `HOME`.
struct ParityHarness {
    dir: std::path::PathBuf,
    wrapper: std::path::PathBuf,
    path_env: String,
    wrap_switcher: std::path::PathBuf,
    exec_switcher: std::path::PathBuf,
    home: std::path::PathBuf,
}

impl ParityHarness {
    /// Returns wrapper success, the wrapper switcher log, the `flex` stub
    /// log, the executor switcher log, and executor success.
    fn run(&self, action_line: &str, exec_id: &str) -> (bool, String, String, String, bool) {
        for log in ["wrap-switcher.sh.log", "exec-switcher.sh.log", "flex.log"] {
            let _ = std::fs::remove_file(self.dir.join(log));
        }
        let wrapper_out = std::process::Command::new("bash")
            .arg(&self.wrapper)
            .env("PATH", &self.path_env)
            .env("THEME_SWITCHER", &self.wrap_switcher)
            .env("STUB_ACTION", action_line)
            .env("POPUP_KITTY", "1")
            .output()
            .expect("run wrapper");
        let wrap_log =
            std::fs::read_to_string(self.dir.join("wrap-switcher.sh.log")).unwrap_or_default();
        let flex_log = std::fs::read_to_string(self.dir.join("flex.log")).unwrap_or_default();
        let exec_result = {
            let _home = set_env("HOME", &self.home);
            let _switcher_env = set_env("THEME_SWITCHER", &self.exec_switcher);
            flex_rice::exec::theme::execute(exec_id, Some(&self.path_env))
        };
        let exec_log =
            std::fs::read_to_string(self.dir.join("exec-switcher.sh.log")).unwrap_or_default();
        (
            wrapper_out.status.success(),
            wrap_log,
            flex_log,
            exec_log,
            exec_result.is_ok(),
        )
    }
}
/// Assert one parity case: same success, byte-identical switcher calls, and
/// the expected `flex` subprocess shape (the resolve the executor elides).
fn check_parity_case(
    case: &str,
    wrapper_ok: bool,
    exec_ok: bool,
    wrap_log: &str,
    exec_log: &str,
    flex_log: &str,
) {
    assert_eq!(
        wrapper_ok, exec_ok,
        "{case}: wrapper and executor must agree on success"
    );
    assert_eq!(
        wrap_log, exec_log,
        "{case}: switcher call sequences must be byte-identical"
    );
    // The wrapper resolves through a `flex theme --resolve` subprocess; the
    // executor resolves in-process, so only the wrapper logs it.
    let flex_calls: Vec<&str> = flex_log.lines().collect();
    match case {
        "activate" | "activate-space-bearing" => {
            let name = if case == "activate" {
                "alpha"
            } else {
                "Tokyo Night"
            };
            assert_eq!(
                wrap_log.trim(),
                format!("[activate][{name}]"),
                "{case}: resolved name reaches activate as ONE argument"
            );
            assert_eq!(
                flex_calls.len(),
                2,
                "{case}: wrapper spawns list + resolve: {flex_calls:?}"
            );
            assert!(
                flex_calls[1].contains("--resolve"),
                "{case}: second flex call is the resolve: {flex_calls:?}"
            );
        }
        "noop" | "bad-id" => {
            assert!(
                wrap_log.is_empty() && exec_log.is_empty(),
                "{case}: nothing reaches the switcher"
            );
            assert_eq!(
                flex_calls.len(),
                1,
                "{case}: short-circuits before resolving: {flex_calls:?}"
            );
        }
        "unknown-id" => {
            assert!(
                wrap_log.is_empty() && exec_log.is_empty(),
                "{case}: unresolvable id never activates"
            );
            assert_eq!(
                flex_calls.len(),
                2,
                "{case}: wrapper attempts the resolve and fails: {flex_calls:?}"
            );
        }
        _ => panic!("unexpected case {case}"),
    }
}

#[test]
fn parity_executor_matches_wrapper_switcher_calls() {
    let _env = ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-parity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    let home = install_theme_home("parity");
    let alpha_id = theme_::entry_id("alpha");
    let tokyo_id = theme_::entry_id("Tokyo Night");
    let ghost_id = theme_::entry_id("ghost");
    write_parity_flex_stub(&dir, &[(&alpha_id, "alpha"), (&tokyo_id, "Tokyo Night")]);
    let path_env = stub_path_env(&dir);
    let wrap_switcher = install_switcher(&dir, "wrap-switcher.sh");
    let exec_switcher = install_switcher(&dir, "exec-switcher.sh");
    let harness = ParityHarness {
        dir,
        wrapper: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("wrappers")
            .join("flex-theme.sh"),
        path_env,
        wrap_switcher,
        exec_switcher,
        home,
    };

    // (case, ACTION line the stubbed `flex theme` prints, executor id)
    let cases = [
        (
            "activate",
            format!("ACTION: theme {alpha_id} alpha"),
            alpha_id.clone(),
        ),
        (
            "activate-space-bearing",
            format!("ACTION: theme {tokyo_id} Tokyo Night"),
            tokyo_id.clone(),
        ),
        (
            "noop",
            String::from("ACTION: theme noop (No themes found)"),
            String::from("noop"),
        ),
        (
            "bad-id",
            String::from("ACTION: theme a/b label"),
            String::from("a/b"),
        ),
        (
            "unknown-id",
            format!("ACTION: theme {ghost_id} ghost"),
            ghost_id.clone(),
        ),
    ];
    for (case, action_line, exec_id) in &cases {
        let (wrapper_ok, wrap_log, flex_log, exec_log, exec_ok) = harness.run(action_line, exec_id);
        check_parity_case(case, wrapper_ok, exec_ok, &wrap_log, &exec_log, &flex_log);
    }
    let _ = std::fs::remove_dir_all(&harness.home);
    let _ = std::fs::remove_dir_all(&harness.dir);
}
