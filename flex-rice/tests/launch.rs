//! `flex launch` cutover tests (M2): provider fixtures, key-seq replays,
//! launch golden, `FLEX_TEST` determinism, and the bash-cache parity probe.
//!
//! Fixtures live in `tests/fixtures/launch/` (see `src/providers/launch.rs`
//! for the parse rules under test).

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::{backend, run, width, Menu, Row, RowId, Tab};
use flex_rice::providers::launch;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("launch")
}

fn fixture_entries() -> Vec<launch::DesktopEntry> {
    launch::scan_dirs(std::slice::from_ref(&fixtures_dir()))
}

fn fixture_menu() -> Menu {
    let tab = launch::tab_from_entries(&fixture_entries());
    flex_rice::menu(launch::PROVIDER, vec![tab])
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn rune(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

// --- Provider fixtures ------------------------------------------------------

#[test]
fn nodisplay_hidden_and_malformed_entries_are_skipped() {
    let entries = fixture_entries();
    let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "firefox.desktop",
            "terminal-app.desktop",
            "onlyshow-app.desktop",
            "percent-app.desktop",
        ],
        "sorted survivor set (labels: Firefox, Htop, OnlyShow, Percent App)"
    );
}

#[test]
fn action_ids_are_space_free_row_hashes_and_terminal_rows_are_marked() {
    let tab = launch::tab_from_entries(&fixture_entries());
    assert_eq!(tab.name, launch::TAB_NAME);
    assert!(tab.bare_rows, "launcher renders bare rows");
    assert!(!tab.deletable, "launcher rows are non-deletable");
    let firefox = tab.rows.first().expect("first row");
    assert_eq!(firefox.label, "Firefox");
    assert_eq!(firefox.meta, None);
    let htop = tab.rows.get(1).expect("second row");
    assert_eq!(htop.meta.as_deref(), Some(launch::TERMINAL_META));
    // The id is a hash, not the desktop-id: the `ACTION:` protocol splits
    // the id on whitespace, so the id must be one token that the wrapper
    // resolves back (B-021).
    let dirs = vec![fixtures_dir()];
    for (row, desktop_id) in tab.rows.iter().zip([
        "firefox.desktop",
        "terminal-app.desktop",
        "onlyshow-app.desktop",
        "percent-app.desktop",
    ]) {
        assert!(
            !row.id.as_str().contains(char::is_whitespace),
            "id is a single token: {:?}",
            row.id.as_str()
        );
        assert_eq!(
            launch::resolve_id_in(&dirs, row.id.as_str()).as_deref(),
            Some(desktop_id),
            "every row id resolves back to its desktop-id"
        );
    }
}

/// B-021: a `.desktop` file may be named with spaces; the row id must stay
/// one whitespace-free token and still resolve back to the real file name.
#[test]
fn space_bearing_desktop_ids_round_trip_through_resolve() {
    let dir = scratch("spaced-id");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let desktop_id = "My App.desktop";
    std::fs::write(
        dir.join(desktop_id),
        "[Desktop Entry]\nName=My App\nExec=myapp %U\n",
    )
    .expect("fixture entry");

    let entries = launch::scan_dirs(std::slice::from_ref(&dir));
    let rows = launch::rows(&entries);
    assert_eq!(rows.len(), 1);
    let id = rows[0].id.as_str();
    assert!(
        !id.contains(char::is_whitespace),
        "space-bearing name is hashed: {id:?}"
    );
    let resolved = launch::resolve_id_in(std::slice::from_ref(&dir), id);
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert_eq!(
        resolved.as_deref(),
        Some(desktop_id),
        "the hash resolves back to the real file name"
    );
}

#[test]
fn percent_codes_are_preserved_for_the_wrapper_to_strip() {
    let dirs = vec![fixtures_dir()];
    let (exec, terminal) = launch::find_exec_in(&dirs, "firefox.desktop").expect("firefox Exec");
    assert!(exec.contains("%U"), "raw %U preserved: {exec:?}");
    assert!(!terminal);
    let (exec, _) = launch::find_exec_in(&dirs, "percent-app.desktop").expect("percent Exec");
    assert_eq!(exec, "myapp %F --open");
    assert_eq!(launch::strip_field_codes(&exec), "myapp --open");
    assert!(launch::find_exec_in(&dirs, "missing.desktop").is_none());
    assert!(launch::find_exec_in(&dirs, "../escape.desktop").is_none());
}

#[test]
fn user_dir_overrides_system_dir_on_duplicate_ids() {
    let system = fixtures_dir().join("override-system");
    let user = fixtures_dir().join("override-user");
    std::fs::create_dir_all(&system).expect("system dir");
    std::fs::create_dir_all(&user).expect("user dir");
    std::fs::write(
        system.join("dup.desktop"),
        "[Desktop Entry]\nName=System Name\nExec=system-app\n",
    )
    .expect("system entry");
    std::fs::write(
        user.join("dup.desktop"),
        "[Desktop Entry]\nName=User Name\nExec=user-app\n",
    )
    .expect("user entry");
    let entries = launch::scan_dirs(&[system.clone(), user.clone()]);
    std::fs::remove_dir_all(&system).expect("cleanup");
    std::fs::remove_dir_all(&user).expect("cleanup");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "User Name");
    assert_eq!(entries[0].exec, "user-app");
}

// --- Diagnostic stream (B-020) --------------------------------------------------

/// Unique scratch dir per call: these tests run in parallel and must not
/// share a path (same class of bug as B-007/B-013).
fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("flex-launch-{}-{name}-{seq}", std::process::id()))
}

/// `NoDisplay`/`Hidden` entries are skipped *by design*, so the real binary
/// must warn only about genuinely malformed `.desktop` files.
///
/// Regression guard for B-020: before the fix this printed one "skipping
/// malformed entry" line per hidden entry — 202 false diagnostics on the
/// reference host, on every `flex launch` and every `flex center` open.
#[test]
fn only_malformed_entries_reach_stderr() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let home = scratch("diagnostics");
    let _ = std::fs::remove_dir_all(&home);
    let apps = home.join(".local/share/applications");
    std::fs::create_dir_all(&apps).expect("apps dir");
    for fixture in [
        "firefox.desktop",
        "nodisplay-app.desktop",
        "hidden-app.desktop",
        "noexec-app.desktop",
        "malformed.desktop",
    ] {
        std::fs::copy(fixtures_dir().join(fixture), apps.join(fixture)).expect("fixture copy");
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex"))
        .arg("launch")
        .env("HOME", &home)
        // In-popup half: the dispatcher re-execs `flex-launch`, which only
        // reaches the menu (and its diagnostics) inside a popup.
        .env("POPUP_KITTY", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output()
        .expect("run flex launch");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let prefix = home.display().to_string();
    std::fs::remove_dir_all(&home).expect("cleanup");

    let mut reported: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("flex: launch: skipping malformed entry"))
        .filter(|line| line.contains(&prefix))
        .map(|line| line.rsplit('/').next().expect("path segment"))
        .collect();
    reported.sort_unstable();
    assert_eq!(
        reported,
        vec!["malformed.desktop", "noexec-app.desktop"],
        "only the two broken fixtures are worth a diagnostic; stderr was:\n{stderr}"
    );
}

// --- Key-seq replays ----------------------------------------------------------

#[test]
fn keyseq_type_fir_enter_selects_firefox() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[rune('f'), rune('i'), rune('r'), press(KeyCode::Enter)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select);
    let row = menu.app.focused_row().expect("focused row");
    assert_eq!(
        launch::resolve_id_in(&[fixtures_dir()], row.id.as_str()).as_deref(),
        Some("firefox.desktop"),
        "the selected row id resolves to Firefox's desktop-id"
    );
    assert_eq!(row.label, "Firefox");
    // The wrapper's ACTION: line for this selection is
    // `ACTION: launch <row-hash> Firefox` (exit 0); `flex-launch.sh`
    // resolves the hash with `flex launch --resolve` before launching.
    assert_eq!(menu.provider, "launch");
}

#[test]
fn keyseq_esc_chain_clears_then_cancels() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    // Typing then Esc clears the filter (consumed, no exit).
    let outcome = run::replay_keys(&mut menu, &[rune('f'), press(KeyCode::Esc)], base);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.filter,
        "",
        "Esc clears the filter first"
    );
    // Empty-filter Esc quits 130 with no stdout.
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], base);
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
}

#[test]
fn delete_never_fires_on_launcher_rows() {
    let mut menu = fixture_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[press(KeyCode::Delete), press(KeyCode::Delete)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Consumed, "Delete is dead on launch");
    assert!(
        !menu.app.active_state().expect("state").confirm_pending,
        "no confirm arms on non-deletable tabs"
    );
}

#[test]
fn legacy_filter_mode_preserves_provider_order() {
    // Anti-score provider order: spec ranks Firefox (100) > My Firm
    // (prefix-word 65) > Confirm (run 45).
    let tab = Tab::with_rows(
        "launch",
        vec![
            Row::new(RowId::new("confirm"), "Confirm"),
            Row::new(RowId::new("firefox"), "Firefox"),
            Row::new(RowId::new("firm"), "My Firm"),
        ],
    );
    let mut menu = flex_rice::menu("launch", vec![tab]);
    menu.app.active_tab_mut().expect("tab").state.filter = "fir".to_string();
    assert_eq!(
        menu.app.visible_rows(),
        vec![1, 2, 0],
        "spec reorders by tier"
    );
    menu.app.filter_mode = flex_core::filter::FilterMode::Legacy;
    assert_eq!(
        menu.app.visible_rows(),
        vec![0, 1, 2],
        "legacy preserves provider order"
    );
}

// --- Launch golden --------------------------------------------------------------

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
fn launch_default_view_golden_at_80x24() {
    let mut menu = fixture_menu();
    // Default focus: row 0 (Firefox), empty filter, provider order.
    assert_eq!(
        menu.app.active_tab().expect("tab").state.focus,
        0,
        "default focus is row 0"
    );
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
    // Row 0: selector + Firefox label (bare mode: no filter/hints chrome).
    // The first list row sits below the reserved `•••` indicator line.
    let first_y = flex_core::render::LIST_INDICATOR_ROWS / 2;
    let bar = buf.cell((0, first_y)).expect("bar cell");
    assert_eq!(bar.symbol(), "░");
    assert_eq!(
        bar.fg,
        flex_core::theme::Theme::DEFAULT
            .selector
            .fg
            .expect("selector sets a fg")
    );
    assert!(
        row_text(&buf, first_y, 80).contains("Firefox"),
        "row 0 shows Firefox"
    );
    let mut all = String::new();
    for y in 0..24 {
        all.push_str(&row_text(&buf, y, 80));
    }
    assert!(!all.contains('›'), "bare mode has no filter line");
    assert!(!all.contains("navigate"), "bare mode has no hints line");
}

// --- FLEX_TEST seed extension -----------------------------------------------------

#[test]
fn flex_test_replays_are_deterministic_across_bases() {
    // Same script, different wall-clock bases: identical outcomes and
    // state, because stamps derive from the base + seeded step only.
    assert_ne!(backend::FLEX_TEST_SEED, 0);
    let script = [rune('f'), rune('i'), rune('r'), press(KeyCode::Enter)];
    let run_script = |menu: &mut Menu| {
        let base = run::test_base();
        let mut outcomes = Vec::new();
        for (index, key) in script.iter().enumerate() {
            // Same stamping as `replay_keys`, spelled out for the audit.
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

// --- Bash-cache parity probe (manual; needs the live cache) -----------------------

/// Compare the Rust scan against `~/.cache/app-launcher.list`.
///
/// Ignored by default (needs live system state); run explicitly:
/// `cargo test --test launch parity_against_app_cache -- --ignored --nocapture`.
/// Prints both counts plus any name-set drift for the cutover record.
#[test]
#[ignore = "needs live ~/.cache/app-launcher.list + system .desktop dirs"]
fn parity_against_app_cache() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let home = std::env::var("HOME").expect("HOME set");
    let cache = PathBuf::from(home).join(".cache/app-launcher.list");
    let text = std::fs::read_to_string(&cache).expect("app-launcher.list readable");
    let mut bash_names: Vec<String> = text
        .lines()
        .filter_map(|line| line.split('\t').next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    bash_names.sort();
    let mut rust_names: Vec<String> = launch::load().iter().map(|row| row.label.clone()).collect();
    rust_names.sort();
    println!("bash cache rows: {}", bash_names.len());
    println!("rust scan rows:  {}", rust_names.len());
    let bash_set: std::collections::BTreeSet<&str> =
        bash_names.iter().map(String::as_str).collect();
    let rust_set: std::collections::BTreeSet<&str> =
        rust_names.iter().map(String::as_str).collect();
    let bash_only: Vec<&&str> = bash_set.difference(&rust_set).collect();
    let rust_only: Vec<&&str> = rust_set.difference(&bash_set).collect();
    println!("bash-only names: {bash_only:?}");
    println!("rust-only names: {rust_only:?}");
    assert_eq!(bash_names.len(), rust_names.len(), "row counts must match");
    assert!(
        bash_only.is_empty() && rust_only.is_empty(),
        "name sets must match"
    );
}

// --- Wrapper dispatch (B-021) -------------------------------------------------

fn wrapper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-launch.sh")
}

/// Write an executable stub script.
fn write_exe(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).expect("stub script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// `flex-launch.sh` must treat the `ACTION:` id as a row hash: resolve it
/// with `flex launch --resolve`, then launch from the real desktop-id. The
/// fixture file is named with a space, which is the case the hash exists
/// for (B-021).
#[test]
fn wrapper_resolves_the_row_hash_before_launching() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let stub = scratch("wrapper");
    let _ = std::fs::remove_dir_all(&stub);
    std::fs::create_dir_all(&stub).expect("stub dir");
    let home = stub.join("home");
    let apps = home.join(".local/share/applications");
    std::fs::create_dir_all(&apps).expect("apps dir");
    std::fs::write(
        apps.join("My App.desktop"),
        "[Desktop Entry]\nName=My App\nExec=myapp %U\nTerminal=false\n",
    )
    .expect("desktop entry");
    let resolve_log = stub.join("resolve.log");
    let call_log = stub.join("calls.log");
    write_exe(
        &stub.join("flex"),
        &format!(
            "#!/usr/bin/env bash\nif [[ \"${{2:-}}\" == \"--resolve\" ]]; then\nprintf 'resolve %s\\n' \"${{3:-}}\" >> '{}'\nprintf '%s\\n' 'My App.desktop'\nexit 0\nfi\nprintf '%s\\n' 'ACTION: launch 0123456789abcdef My App'\n",
            resolve_log.display()
        ),
    );
    write_exe(
        &stub.join("setsid"),
        &format!(
            "#!/usr/bin/env bash\nprintf 'setsid %s\\n' \"$*\" >> '{}'\n",
            call_log.display()
        ),
    );

    let output = std::process::Command::new("bash")
        .arg(wrapper_path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                stub.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", &home)
        // The popup re-exec is bind-path behavior; this emulates the
        // in-popup half.
        .env("POPUP_KITTY", "1")
        .output()
        .expect("run wrapper");
    assert!(
        output.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&resolve_log)
            .expect("resolve log")
            .trim(),
        "resolve 0123456789abcdef",
        "the hash — not the file name — is resolved through the binary"
    );
    let calls = std::fs::read_to_string(&call_log).expect("call log");
    assert_eq!(
        calls.trim(),
        "setsid -f myapp",
        "field codes stripped, space-bearing desktop-id launched: {calls:?}"
    );

    // An id the provider cannot resolve must abort without running anything.
    write_exe(
        &stub.join("flex"),
        "#!/usr/bin/env bash\nif [[ \"${2:-}\" == \"--resolve\" ]]; then\necho 'flex: error: launch: unknown id' >&2\nexit 1\nfi\nprintf '%s\\n' 'ACTION: launch 0123456789abcdef My App'\n",
    );
    std::fs::remove_file(&call_log).expect("reset log");
    let output = std::process::Command::new("bash")
        .arg(wrapper_path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                stub.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", &home)
        .env("POPUP_KITTY", "1")
        .output()
        .expect("run wrapper");
    assert!(!output.status.success(), "unresolvable id must fail");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown id: 0123456789abcdef"),
        "wrapper names the id it could not resolve"
    );
    assert!(!call_log.exists(), "nothing launches on an unresolvable id");
    let _ = std::fs::remove_dir_all(&stub);
}

// --- Empty-scan placeholder (B-026) -------------------------------------------

/// With nothing to launch the menu must not be blank: `center` already
/// showed `(No applications found)`, and the standalone picker now shows the
/// same `noop` row instead of an empty list.
#[test]
fn empty_scan_shows_the_noop_placeholder() {
    let tab = launch::tab_from_entries(&[]);
    assert_eq!(tab.name, launch::TAB_NAME);
    assert_eq!(tab.rows.len(), 1, "placeholder instead of a blank menu");
    assert_eq!(tab.rows[0].id.as_str(), flex_rice::providers::NOOP_ID);
    assert_eq!(tab.rows[0].label, launch::NO_APPS_LABEL);
    assert_eq!(
        tab.rows[0].label, "(No applications found)",
        "bash-exact text"
    );

    // `Enter` on the placeholder selects it (a no-op for the wrapper), it
    // does not quit or panic.
    let mut menu = flex_rice::menu(launch::PROVIDER, vec![tab]);
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Enter)], run::test_base());
    assert_eq!(outcome, KeyOutcome::Select);
    let focused = menu.app.focused_row().expect("focused row");
    assert_eq!(focused.id.as_str(), flex_rice::providers::NOOP_ID);
}

/// The placeholder's `noop` id must never reach the launcher: `flex-launch.sh`
/// exits 0 without resolving or running anything (B-026).
#[test]
fn wrapper_treats_the_noop_placeholder_as_a_noop() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let stub = scratch("wrapper-noop");
    let _ = std::fs::remove_dir_all(&stub);
    std::fs::create_dir_all(&stub).expect("stub dir");
    let resolve_log = stub.join("resolve.log");
    let call_log = stub.join("calls.log");
    write_exe(
        &stub.join("flex"),
        &format!(
            "#!/usr/bin/env bash\nif [[ \"${{2:-}}\" == \"--resolve\" ]]; then\nprintf 'resolve %s\\n' \"${{3:-}}\" >> '{}'\nprintf '%s\\n' 'My App.desktop'\nexit 0\nfi\nprintf '%s\\n' 'ACTION: launch noop (No applications found)'\n",
            resolve_log.display()
        ),
    );
    write_exe(
        &stub.join("setsid"),
        &format!(
            "#!/usr/bin/env bash\nprintf 'setsid %s\\n' \"$*\" >> '{}'\n",
            call_log.display()
        ),
    );
    let output = std::process::Command::new("bash")
        .arg(wrapper_path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                stub.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", stub.join("home"))
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
        "noop short-circuits before the id lookup"
    );
    assert!(!call_log.exists(), "nothing is launched");
    let _ = std::fs::remove_dir_all(&stub);
}

// --- Engine error format (B-027) -----------------------------------------------

/// Engine-raised errors carry no `flex:` prefix of their own: `main` adds
/// `flex: error:` exactly once, so the message must not double it the way
/// `flex: error: flex: cannot open /dev/tty …` used to.
///
/// `setsid(1)` detaches the controlling terminal, so `backend::init` fails
/// deterministically whether or not `cargo test` itself runs under a tty
/// (without it, a ctty-inheriting child would open the real TUI and hang).
/// The trailing `(os error N)` text is environment-dependent, so the test
/// pins the stable prefix plus the single-`flex:` count instead of the
/// whole line.
#[test]
fn engine_errors_are_reported_with_a_single_prefix() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let output = std::process::Command::new("setsid")
        .arg(env!("CARGO_BIN_EXE_flex"))
        .arg("launch")
        // In-popup half: the dispatcher re-execs `flex-launch`, which only
        // reaches the TTY probe inside a popup.
        .env("POPUP_KITTY", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output()
        .expect("run flex launch without a controlling terminal");
    assert!(!output.status.success(), "tty failure exits non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("flex:").count(),
        1,
        "exactly one `flex:` prefix: {stderr:?}"
    );
    assert!(
        stderr.starts_with("flex: error: cannot open /dev/tty (needs a controlling terminal): "),
        "engine message carries no prefix of its own: {stderr:?}"
    );
}

// --- Executor (`flex-rice/src/exec/launch.rs`) ---------------------------------
//
// The launch port of the `exec::shot` template: `LaunchAction::parse`
// validates the id, `execute` resolves the hash in-process (no
// `flex launch --resolve` subprocess) and detaches through the `setsid`
// seam — `kitty -e` prefixed verbatim for `Terminal=true` apps (the
// deferred behaviour change). No TUI, no pty: stub `PATH` + scratch `HOME`.
//
// Like the theme suite, the popup guard itself lives in the shared runner
// (covered by `popup.rs`), with one binary-level re-exec probe below.

/// Serialises the executor tests below: each mutates `HOME` (the seam the
/// id lookup reads through `app_dirs`) and the harness runs tests in
/// parallel, so the mutations are held under one lock with restores in
/// [`ExecEnvGuard`].
static EXEC_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Save/restore one process env var around an executor call.
struct ExecEnvGuard {
    key: &'static str,
    old: Option<String>,
}

fn set_exec_env(key: &'static str, value: &std::path::Path) -> ExecEnvGuard {
    let old = std::env::var(key).ok();
    std::env::set_var(key, value);
    ExecEnvGuard { key, old }
}

impl Drop for ExecEnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Scratch `HOME` with an applications dir: a plain app, a `Terminal=true`
/// app (the hardcoded kitty branch), and a space-bearing desktop-id whose
/// `Exec` carries field codes plus real flags (B-021 + strip in one row).
fn install_launch_home(tag: &str) -> PathBuf {
    let home = scratch(&format!("exec-home-{tag}"));
    let apps = home.join(".local/share/applications");
    std::fs::create_dir_all(&apps).expect("apps dir");
    std::fs::write(
        apps.join("firefox.desktop"),
        "[Desktop Entry]\nName=Firefox\nExec=firefox %U\nTerminal=false\n",
    )
    .expect("plain entry");
    std::fs::write(
        apps.join("termapp.desktop"),
        "[Desktop Entry]\nName=Htop\nExec=htop\nTerminal=true\n",
    )
    .expect("terminal entry");
    std::fs::write(
        apps.join("My App.desktop"),
        "[Desktop Entry]\nName=My App\nExec=myapp %F --open\nTerminal=false\n",
    )
    .expect("spaced entry");
    home
}

fn exec_stub_path(dir: &std::path::Path) -> String {
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// `setsid` stub appending `setsid <argv…>` per call (the wrapper-parity
/// shape: `$*` joins with spaces, exactly the [`describe`] lines).
fn install_setsid(dir: &std::path::Path) {
    let log = dir.join("setsid.log");
    write_exe(
        &dir.join("setsid"),
        &format!(
            "#!/usr/bin/env bash\nprintf 'setsid %s\\n' \"$*\" >> '{}'\n",
            log.display()
        ),
    );
}

fn read_setsid_log(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("setsid.log")).unwrap_or_default()
}

/// Full action-id matrix: every installed app (plain, `Terminal=true`
/// kitty, space-bearing id with field codes) launches through the `setsid`
/// seam as one detached call.
#[test]
fn executor_matrix_launches_every_app_through_the_setsid_seam() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-matrix");
    std::fs::create_dir_all(&dir).expect("stub dir");
    install_setsid(&dir);
    let home = install_launch_home("matrix");
    let _home = set_exec_env("HOME", &home);
    let path_env = exec_stub_path(&dir);
    for desktop_id in ["firefox.desktop", "termapp.desktop", "My App.desktop"] {
        let id = launch::entry_id(desktop_id);
        let report =
            flex_rice::exec::launch::execute(&id, Some(&path_env)).expect("launch succeeds");
        assert_eq!(report.desktop_id.as_deref(), Some(desktop_id));
    }
    let log = read_setsid_log(&dir);
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        vec![
            "setsid -f firefox",
            "setsid -f kitty -e htop",
            "setsid -f myapp --open",
        ],
        "one detached call per id: field codes stripped, kitty prefix verbatim \
         (describe: `setsid -f [kitty -e ]<program> [args…]`)",
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// B-021 through the executor: the desktop-id bears a space, the row id is
/// one whitespace-free token, and the launch still reaches the right
/// program with its flags intact.
#[test]
fn executor_space_bearing_desktop_id_round_trip() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-spaced");
    std::fs::create_dir_all(&dir).expect("stub dir");
    install_setsid(&dir);
    let home = install_launch_home("spaced");
    let _home = set_exec_env("HOME", &home);
    let id = launch::entry_id("My App.desktop");
    assert!(
        !id.contains(char::is_whitespace),
        "space-bearing name is hashed: {id:?}"
    );
    let report = flex_rice::exec::launch::execute(&id, Some(&exec_stub_path(&dir)))
        .expect("launch succeeds");
    assert_eq!(report.desktop_id.as_deref(), Some("My App.desktop"));
    assert_eq!(report.program.as_deref(), Some("myapp"));
    assert!(!report.terminal);
    assert_eq!(
        read_setsid_log(&dir).trim(),
        "setsid -f myapp --open",
        "field codes stripped, flags kept as separate args"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// The `noop` placeholder exits 0 before resolving or launching (B-026):
/// even a missing `HOME` cannot fail it, like the wrapper's early exit.
#[test]
fn executor_noop_short_circuits_before_resolve() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-noop");
    std::fs::create_dir_all(&dir).expect("stub dir");
    install_setsid(&dir);
    let _home = set_exec_env("HOME", &dir.join("no-such-home"));
    let report =
        flex_rice::exec::launch::execute("noop", Some(&exec_stub_path(&dir))).expect("noop is Ok");
    assert_eq!(
        report,
        flex_rice::exec::launch::ExecuteReport {
            desktop_id: None,
            program: None,
            terminal: false,
        }
    );
    assert!(
        !dir.join("setsid.log").exists(),
        "nothing is launched for the placeholder"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Malformed ids (`bad id`, mirroring the wrapper check) and well-formed but
/// unresolvable hashes (`unknown id`) are errors with no `flex:` prefix of
/// their own — the runner adds the single prefix at the binary boundary.
#[test]
fn executor_rejects_bad_and_unknown_ids_without_its_own_prefix() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-bad");
    std::fs::create_dir_all(&dir).expect("stub dir");
    install_setsid(&dir);
    let home = install_launch_home("bad");
    let _home = set_exec_env("HOME", &home);
    let path_env = exec_stub_path(&dir);
    let ghost_id = launch::entry_id("ghost.desktop");
    let cases = [
        ("", "launch: bad id ''"),
        ("a/b", "launch: bad id 'a/b'"),
        ("a\nb", "launch: bad id 'a\nb'"),
        (ghost_id.as_str(), "launch: unknown id"),
    ];
    for (id, expected) in cases {
        let err = flex_rice::exec::launch::execute(id, Some(&path_env))
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
        !dir.join("setsid.log").exists(),
        "no id failure launches anything"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&home);
}

/// A failing `setsid` is a loud error, never a quiet cancel: launch has no
/// `slurp`-like user-cancellable step, so every tool failure exits non-zero
/// through the runner (the menu-cancel 130 lives in the binary's
/// `Outcome::Cancelled` arm, shared with theme, and needs a pty).
#[test]
fn executor_tool_failure_is_a_loud_error_not_a_quiet_cancel() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-fail");
    std::fs::create_dir_all(&dir).expect("stub dir");
    write_exe(&dir.join("setsid"), "#!/usr/bin/env bash\nexit 3\n");
    let home = install_launch_home("fail");
    let _home = set_exec_env("HOME", &home);
    let id = launch::entry_id("firefox.desktop");
    let err = flex_rice::exec::launch::execute(&id, Some(&exec_stub_path(&dir)))
        .expect_err("a failing setsid must fail");
    assert_eq!(
        format!("{err:#}"),
        "launch: failed to launch firefox.desktop"
    );
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
        .arg(env!("CARGO_BIN_EXE_flex-launch"))
        .env("POPUP_KITTY", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-launch without a pty");
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

/// Outside a popup the binary re-execs into the `menu-wide` popup (the
/// shared runner guard); the kitty spawn is asserted against a stub `PATH`.
/// The stub `PATH` rides on the child env only — no process-env mutation,
// hence no lock.
#[test]
fn binary_outside_a_popup_reexecs_into_the_menu_wide_popup() {
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
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("scratch HOME");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex-launch"))
        .env("PATH", exec_stub_path(&dir))
        .env("HOME", &home)
        .env_remove("POPUP_KITTY")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-launch outside a popup");
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
            && logged.contains(&String::from("flex-menu-wide"))
            && logged.contains(&String::from("POPUP_KITTY=1")),
        "re-exec uses the menu-wide popup template: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Parity with the untouched wrapper --------------------------------------
//
// For every action the wrapper supports, the wrapper (under stub `PATH`)
// and the executor must produce byte-identical `setsid` call sequences.
// Two deliberate departures, both asserted below:
//
// - The wrapper's `flex launch --resolve` subprocess is resolved in-process
//   (same `launch::resolve_id` scan the hidden `--resolve` lookup uses), so
//   the wrapper makes one extra `flex` call the executor never spawns.
// - The wrapper `eval`s the stripped `Exec` line (shell quotes group args);
//   the executor splits on whitespace. All parity inputs are plain
//   `prog --flag` lines, where the two agree byte-for-byte.

/// `flex` stub answering both wrapper calls: the `ACTION:` line
/// (`$STUB_ACTION`) and the `--resolve` lookup over the test apps. Every
/// invocation is logged so the elided resolve subprocess stays visible.
fn write_parity_flex_stub(dir: &std::path::Path, apps: &[(&str, &str)]) {
    use std::fmt::Write as _;
    let mut arms = String::new();
    for (hash, desktop_id) in apps {
        let _ = writeln!(arms, "    \"{hash}\") printf '%s\\n' \"{desktop_id}\" ;;");
    }
    write_exe(
        &dir.join("flex"),
        &format!(
            "#!/usr/bin/env bash\necho \"flex $@\" >> \"{}/flex.log\"\nif [[ \"${{2:-}}\" == \"--resolve\" ]]; then\n  case \"${{3:-}}\" in\n{arms}    *) exit 1 ;;\n  esac\n  exit 0\nfi\nprintf '%s\\n' \"$STUB_ACTION\"\n",
            dir.display()
        ),
    );
}

/// Fixed parity inputs: stub dirs, the wrapper, and the scratch `HOME` both
/// sides resolve against. One parity case runs the wrapper under stub
/// `PATH`, then the executor with the scratch `HOME`.
struct LaunchParityHarness {
    dir: std::path::PathBuf,
    wrapper: std::path::PathBuf,
    path_env: String,
    home: std::path::PathBuf,
}

impl LaunchParityHarness {
    /// Returns wrapper success, the wrapper `setsid` log, the `flex` stub
    /// log, the executor `setsid` log, and executor success.
    fn run(&self, action_line: &str, exec_id: &str) -> (bool, String, String, String, bool) {
        for log in ["setsid-wrap.log", "setsid-exec.log", "flex.log"] {
            let _ = std::fs::remove_file(self.dir.join(log));
        }
        let wrap_setsids = self.dir.join("setsid-wrap.log");
        let exec_setsids = self.dir.join("setsid-exec.log");
        write_exe(
            &self.dir.join("setsid"),
            "#!/usr/bin/env bash\nprintf 'setsid %s\\n' \"$*\" >> \"$STUB_SETSID_LOG\"\n",
        );
        let wrapper_out = std::process::Command::new("bash")
            .arg(&self.wrapper)
            .env("PATH", &self.path_env)
            .env("HOME", &self.home)
            .env("STUB_ACTION", action_line)
            .env("STUB_SETSID_LOG", &wrap_setsids)
            .env("POPUP_KITTY", "1")
            .output()
            .expect("run wrapper");
        let wrap_log = std::fs::read_to_string(&wrap_setsids).unwrap_or_default();
        let flex_log = std::fs::read_to_string(self.dir.join("flex.log")).unwrap_or_default();
        let exec_result = {
            let _home = set_exec_env("HOME", &self.home);
            write_exe(
                &self.dir.join("setsid"),
                &format!(
                    "#!/usr/bin/env bash\nprintf 'setsid %s\\n' \"$*\" >> '{}'\n",
                    exec_setsids.display()
                ),
            );
            flex_rice::exec::launch::execute(exec_id, Some(&self.path_env))
        };
        let exec_log = std::fs::read_to_string(&exec_setsids).unwrap_or_default();
        (
            wrapper_out.status.success(),
            wrap_log,
            flex_log,
            exec_log,
            exec_result.is_ok(),
        )
    }
}

/// Assert one parity case: same success, byte-identical `setsid` calls, and
/// the expected `flex` subprocess shape (the resolve the executor elides).
fn check_launch_parity_case(
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
        "{case}: setsid call sequences must be byte-identical"
    );
    // The wrapper resolves through a `flex launch --resolve` subprocess; the
    // executor resolves in-process, so only the wrapper logs it.
    let flex_calls: Vec<&str> = flex_log.lines().collect();
    match case {
        "plain" | "terminal" | "space-bearing" => {
            let expected = match case {
                "plain" => "setsid -f firefox",
                "terminal" => "setsid -f kitty -e htop",
                _ => "setsid -f myapp --open",
            };
            assert_eq!(
                wrap_log.trim(),
                expected,
                "{case}: the launched command must match"
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
                "{case}: nothing launches"
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
                "{case}: unresolvable id never launches"
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
fn parity_executor_matches_wrapper_setsid_calls() {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-parity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    let home = install_launch_home("parity");
    let plain_id = launch::entry_id("firefox.desktop");
    let term_id = launch::entry_id("termapp.desktop");
    let spaced_id = launch::entry_id("My App.desktop");
    let ghost_id = launch::entry_id("ghost.desktop");
    write_parity_flex_stub(
        &dir,
        &[
            (plain_id.as_str(), "firefox.desktop"),
            (term_id.as_str(), "termapp.desktop"),
            (spaced_id.as_str(), "My App.desktop"),
        ],
    );
    let harness = LaunchParityHarness {
        path_env: exec_stub_path(&dir),
        wrapper: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("wrappers")
            .join("flex-launch.sh"),
        dir,
        home,
    };

    // (case, ACTION line the stubbed `flex launch` prints, executor id)
    let cases = [
        (
            "plain",
            format!("ACTION: launch {plain_id} Firefox"),
            plain_id.clone(),
        ),
        (
            "terminal",
            format!("ACTION: launch {term_id} Htop"),
            term_id.clone(),
        ),
        (
            "space-bearing",
            format!("ACTION: launch {spaced_id} My App"),
            spaced_id.clone(),
        ),
        (
            "noop",
            String::from("ACTION: launch noop (No applications found)"),
            String::from("noop"),
        ),
        (
            "bad-id",
            String::from("ACTION: launch a/b label"),
            String::from("a/b"),
        ),
        (
            "unknown-id",
            format!("ACTION: launch {ghost_id} ghost"),
            ghost_id.clone(),
        ),
    ];
    for (case, action_line, exec_id) in &cases {
        let (wrapper_ok, wrap_log, flex_log, exec_log, exec_ok) = harness.run(action_line, exec_id);
        check_launch_parity_case(case, wrapper_ok, exec_ok, &wrap_log, &exec_log, &flex_log);
    }
    let _ = std::fs::remove_dir_all(&harness.home);
    let _ = std::fs::remove_dir_all(&harness.dir);
}
