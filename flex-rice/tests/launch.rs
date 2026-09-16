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
