//! `flex clip` cutover tests (M4): provider ingest, key-seq replays, clip
//! golden, `FLEX_TEST` determinism, wrapper dispatch stubs, and the bash
//! parity probe.
//!
//! Parity is against the `pick()` path of
//! `scripts/.config/scripts/cliphist.sh` (only that path is cut over;
//! `add`/`pin`/`unpin` stay in bash — see `src/providers/clip.rs`).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::keys::{handle_key, KeyOutcome, EXIT_CANCELLED};
use flex_core::{backend, content_hash_hex, run, width, Menu, Mode};
use flex_rice::providers::clip;

/// Unique scratch dir per call (tests run in parallel; a shared name
/// would race `remove_dir_all` against a sibling's writes).
fn scratch(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "flex-clip-test-{}-{name}-{seq}",
        std::process::id()
    ))
}

/// Binary corpus exercising every cleaning rule:
/// NUL / `0x1F` / ESC stripping, tab expansion, invalid UTF-8 (lossy),
/// empty-line drops, glob characters (literal), `<NEWLINE>` multiline
/// entries, duplicates, and a 200-char long line (120-char preview).
fn write_corpus(dir: &Path) -> (PathBuf, PathBuf) {
    let hist = dir.join("cliphist");
    let pins = dir.join("cliphist.pins");
    let mut hist_bytes: Vec<u8> = Vec::new();
    hist_bytes.extend_from_slice(b"first entry\n");
    hist_bytes.extend_from_slice(b"\n"); // dropped: empty line
    hist_bytes.extend_from_slice("pinned one\n".as_bytes()); // dup of a pin
    hist_bytes.extend_from_slice(b"bin\x00a\x1fry\x1b\tend\xff\xfe\n");
    hist_bytes.extend_from_slice("glob *[abc]? {x,y}\n".as_bytes());
    hist_bytes.extend_from_slice("alpha<NEWLINE>beta gamma\n".as_bytes());
    hist_bytes.extend_from_slice(format!("{}\n", "L".repeat(200)).as_bytes());
    hist_bytes.extend_from_slice(b"terminal tools\n");
    hist_bytes.extend_from_slice(b"second entry\n");
    std::fs::write(&hist, hist_bytes).expect("write hist fixture");
    std::fs::write(&pins, "pinned one\nalpha<NEWLINE>beta gamma\n*zpinned*\n")
        .expect("write pins fixture");
    (hist, pins)
}

fn corpus_entries() -> (PathBuf, Vec<clip::ClipEntry>) {
    let dir = scratch("corpus");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let (hist, pins) = write_corpus(&dir);
    let entries = clip::load_entries_in(&hist, &pins);
    (dir, entries)
}

fn corpus_menu() -> (PathBuf, Menu) {
    let (dir, entries) = corpus_entries();
    let menu = flex_rice::menu(clip::PROVIDER, vec![clip::tab_from_entries(&entries)]);
    (dir, menu)
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn rune(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

fn nav(menu: &mut Menu) {
    menu.app.mode = Mode::Navigate;
}

// --- Provider ingest --------------------------------------------------------

#[test]
fn binary_garbage_is_cleaned_without_panicking() {
    let (dir, entries) = corpus_entries();
    let fulls: Vec<&str> = entries.iter().map(|entry| entry.full.as_str()).collect();
    assert!(
        fulls.contains(&"binary end��"),
        "NUL/0x1F/ESC stripped, tab expanded, invalid bytes lossy: {fulls:?}"
    );
    assert!(
        !fulls.iter().any(|line| line.is_empty()),
        "empty lines dropped"
    );
    assert!(
        !fulls.iter().any(|line| line.contains('\0')),
        "no NUL survives"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn glob_labels_render_literally_and_resolve_losslessly() {
    let (dir, entries) = corpus_entries();
    let glob = entries
        .iter()
        .find(|entry| entry.full.contains("*[abc]?"))
        .expect("glob entry ingested");
    assert!(
        glob.row.label.contains("*[abc]?"),
        "glob chars literal in label: {:?}",
        glob.row.label
    );
    let (hist, pins) = (dir.join("cliphist"), dir.join("cliphist.pins"));
    assert_eq!(
        clip::resolve_in(&hist, &pins, glob.row.id.as_str()).as_deref(),
        Some(glob.full.as_str()),
        "hash id resolves back to the exact stored line"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_truncates_at_120_chars_but_full_is_kept() {
    let (dir, entries) = corpus_entries();
    let long = entries
        .iter()
        .find(|entry| entry.full.len() > clip::PREVIEW_CHARS)
        .expect("long entry ingested");
    assert_eq!(
        long.row.label.chars().count(),
        clip::PREVIEW_CHARS,
        "preview is exactly 120 chars"
    );
    assert_eq!(long.full.chars().count(), 200, "full text preserved");
    assert_eq!(
        long.row.id.as_str(),
        content_hash_hex(&long.full).as_str(),
        "Q2: id is the content-hash hex"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pins_first_with_meta_then_history_newest_first() {
    let (dir, entries) = corpus_entries();
    let labels: Vec<&str> = entries
        .iter()
        .map(|entry| entry.row.label.as_str())
        .collect();
    assert_eq!(
        &labels[..3],
        &["pinned one", "alpha<NEWLINE>beta gamma", "*zpinned*"],
        "pins first in file order: {labels:?}"
    );
    for entry in &entries[..3] {
        assert!(entry.pinned);
        assert_eq!(
            entry.row.meta.as_deref(),
            Some(clip::PINNED_META),
            "pins carry the bash 📌 mark"
        );
    }
    assert_eq!(
        &labels[3..],
        &[
            "second entry",
            "terminal tools",
            &"L".repeat(clip::PREVIEW_CHARS),
            "glob *[abc]? {x,y}",
            "binary end��",
            "first entry",
        ],
        "history newest-first (tac), deduped against pins: {labels:?}"
    );
    assert!(
        entries[3..].iter().all(|entry| !entry.pinned),
        "history rows are unpinned"
    );
    assert!(
        entries[3..].iter().all(|entry| entry.row.meta.is_none()),
        "history rows carry no meta"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn action_ids_are_content_hashes_and_widths_are_precomputed() {
    let (dir, entries) = corpus_entries();
    for entry in &entries {
        assert_eq!(
            entry.row.id.as_str(),
            content_hash_hex(&entry.full).as_str(),
            "Q2 hash id for {:?}",
            entry.full
        );
        assert_eq!(
            entry.measured.width,
            width::str_width(&entry.row.label),
            "precomputed width matches label for {:?}",
            entry.row.label
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_files_yield_no_rows_without_panicking() {
    let missing = scratch("missing");
    let _ = std::fs::remove_dir_all(&missing);
    assert!(clip::load_entries_in(&missing.join("h"), &missing.join("p")).is_empty());
    assert!(clip::tab_from_entries(&[]).rows.is_empty());
}

#[test]
fn clip_tab_is_standard_deletable() {
    let (_, entries) = corpus_entries();
    let tab = clip::tab_from_entries(&entries);
    assert_eq!(tab.name, clip::TAB_NAME);
    assert!(tab.bare_rows, "clip uses bare rows");
    assert!(tab.deletable, "clip opts into the Delete+confirm flow");
}

#[test]
fn env_overrides_select_the_store() {
    // Serialised with the executor tests below (shared process env).
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = scratch("env");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let (hist, pins) = write_corpus(&dir);
    std::env::set_var(clip::HIST_ENV, &hist);
    std::env::set_var(clip::PINS_ENV, &pins);
    let entries = clip::load_entries();
    std::env::remove_var(clip::HIST_ENV);
    std::env::remove_var(clip::PINS_ENV);
    assert_eq!(entries.len(), 9, "env store loads 3 pins + 6 history");
    assert_eq!(entries[0].full, "pinned one");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn decode_restores_newlines_for_copy() {
    assert_eq!(
        clip::decode("alpha<NEWLINE>beta gamma"),
        "alpha\nbeta gamma"
    );
}

// --- Key-seq replays ----------------------------------------------------------

#[test]
fn keyseq_filter_then_enter_selects_with_hash_id() {
    let (dir, mut menu) = corpus_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(
        &mut menu,
        &[
            rune('t'),
            rune('e'),
            rune('r'),
            rune('m'),
            press(KeyCode::Enter),
        ],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Select);
    let row = menu.app.focused_row().expect("focused row");
    assert_eq!(row.label, "terminal tools");
    assert_eq!(
        row.id.as_str(),
        content_hash_hex("terminal tools").as_str(),
        "ACTION: clip <hash> carries the content hash"
    );
    assert_eq!(menu.provider, "clip");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn normal_m_types_into_filter_nav_m_toggles_pin() {
    let (dir, mut menu) = corpus_menu();
    let base = run::test_base();
    // NORMAL `m` must keep editing the filter (clipboard text has m's).
    let outcome = run::replay_keys(&mut menu, &[rune('m')], base);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(
        menu.app.active_tab().expect("tab").state.filter,
        "m",
        "NORMAL m types"
    );
    menu.app.active_tab_mut().expect("tab").state.filter.clear();
    // NAVIGATE `m` on the deletable clip tab exits with TOGGLE.
    nav(&mut menu);
    let outcome = run::replay_keys(&mut menu, &[rune('m')], base);
    assert_eq!(outcome, KeyOutcome::Toggle, "NAVIGATE m toggles pin");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nav_m_still_marks_on_non_deletable_tabs() {
    let mut menu = flex_rice::menu(
        "theme",
        vec![flex_core::Tab::with_rows(
            "t",
            vec![flex_core::Row::new(flex_core::RowId::new("a"), "alpha")],
        )],
    );
    nav(&mut menu);
    let base = run::test_base();
    let outcome = run::replay_keys(&mut menu, &[rune('m')], base);
    assert_eq!(outcome, KeyOutcome::Consumed, "legacy mark path kept");
    assert!(
        menu.app.active_state().expect("state").marked.contains(&0),
        "row marked in place"
    );
}

#[test]
fn delete_arms_confirm_then_deletes_esc_cancels() {
    let (dir, mut menu) = corpus_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Delete)], base);
    assert_eq!(outcome, KeyOutcome::Consumed, "first Delete arms");
    assert!(
        menu.app.active_state().expect("state").confirm_pending,
        "confirm armed in-TUI (bash y/N parity)"
    );
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Delete)], base);
    assert_eq!(outcome, KeyOutcome::Delete, "second Delete confirms");
    // Fresh menu: Delete then Esc cancels the pending confirm.
    let (dir_b, mut menu_b) = corpus_menu();
    let outcome = run::replay_keys(
        &mut menu_b,
        &[press(KeyCode::Delete), press(KeyCode::Esc)],
        base,
    );
    assert_eq!(outcome, KeyOutcome::Consumed, "Esc cancels, no delete");
    assert!(
        !menu_b.app.active_state().expect("state").confirm_pending,
        "pending cleared"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir_b);
}

#[test]
fn keyseq_esc_cancels_with_no_action() {
    let (dir, mut menu) = corpus_menu();
    let base = run::test_base();
    let outcome = run::replay_keys(&mut menu, &[press(KeyCode::Esc)], base);
    assert_eq!(outcome, KeyOutcome::Quit(EXIT_CANCELLED));
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Clip golden ---------------------------------------------------------------

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
fn clip_default_view_golden_at_80x24() {
    let (dir, mut menu) = corpus_menu();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    terminal
        .draw(|frame| flex_core::render::render(frame, &mut menu))
        .expect("render frame");
    let buf = terminal.backend().buffer().clone();
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
    assert!(!all.contains("Pinned"), "bare rows hide meta");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- FLEX_TEST seed extension ---------------------------------------------------

#[test]
fn flex_test_replays_are_deterministic_across_bases() {
    assert_ne!(backend::FLEX_TEST_SEED, 0);
    let script = [rune('t'), rune('e'), press(KeyCode::Enter)];
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
    let (dir_a, mut first) = corpus_menu();
    let (dir_b, mut second) = corpus_menu();
    assert_eq!(run_script(&mut first), run_script(&mut second));
    assert_eq!(
        first.app.focused_row().expect("row").id,
        second.app.focused_row().expect("row").id
    );
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

// --- Wrapper stub tests ---------------------------------------------------------

fn wrapper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("wrappers")
        .join("flex-clip.sh")
}

/// Install stub `flex` (canned `ACTION:` + `--resolve`), stub
/// `wl-copy`/`notify-send` logging to the dir, write a real store, then run
/// the wrapper with `CLIPHIST_FILE`/`CLIPHIST_PINS` pointed at it.
fn run_wrapper_with_stubs(
    name: &str,
    action_line: &str,
    raw_line: &str,
    hist: &[u8],
    pins: &[u8],
) -> (PathBuf, std::process::Output) {
    let dir = scratch(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    std::fs::write(dir.join("cliphist"), hist).expect("hist");
    std::fs::write(dir.join("cliphist.pins"), pins).expect("pins");
    let flex = dir.join("flex");
    std::fs::write(
        &flex,
        format!(
            "#!/usr/bin/env bash\nif [[ \"${{2:-}}\" == \"--resolve\" ]]; then\nprintf '%s\\n' \"{raw_line}\"\nexit 0\nfi\nprintf '%s\\n' \"{action_line}\"\n"
        ),
    )
    .expect("flex stub");
    let copy = dir.join("wl-copy");
    std::fs::write(
        &copy,
        format!(
            "#!/usr/bin/env bash\ncat > \"{}/copied.bin\"\necho \"wl-copy $@\" >> \"{}/calls.log\"\n",
            dir.display(),
            dir.display()
        ),
    )
    .expect("wl-copy stub");
    let notify = dir.join("notify-send");
    std::fs::write(
        &notify,
        format!(
            "#!/usr/bin/env bash\necho \"notify-send $@\" >> \"{}/calls.log\"\n",
            dir.display()
        ),
    )
    .expect("notify stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for path in [&flex, &copy, &notify] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    let path_env = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = std::process::Command::new("bash")
        .arg(wrapper_path())
        .env("PATH", path_env)
        .env("CLIPHIST_FILE", dir.join("cliphist"))
        .env("CLIPHIST_PINS", dir.join("cliphist.pins"))
        // The popup re-exec is bind-path behavior; wrapper tests emulate
        // the in-popup half (stubbed HOME has no popup.sh).
        .env("POPUP_KITTY", "1")
        .output()
        .expect("run wrapper");
    (dir, output)
}

fn ok(output: &std::process::Output) -> String {
    assert!(
        output.status.success(),
        "wrapper failed, stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn wrapper_select_copies_decoded_content_after_exit() {
    let raw = "alpha<NEWLINE>beta gamma";
    let hash = content_hash_hex(raw);
    let action = format!("ACTION: clip {hash} alpha<NEWLINE>beta gamma");
    let (dir, output) = run_wrapper_with_stubs(
        "select",
        &action,
        raw,
        b"alpha<NEWLINE>beta gamma\nother\n",
        b"",
    );
    ok(&output);
    let copied = std::fs::read(dir.join("copied.bin")).expect("wl-copy stdin");
    assert_eq!(
        copied, b"alpha\nbeta gamma\n",
        "placeholder decoded on copy"
    );
    let log = std::fs::read_to_string(dir.join("calls.log")).expect("call log");
    assert!(log.contains("wl-copy"), "wl-copy runs post-TUI: {log:?}");
    assert!(
        log.contains("notify-send -a Cliphist Copied to clipboard"),
        "copy notifies: {log:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_select_round_trips_glob_content_literally() {
    let raw = "glob *[abc]? {x,y}";
    let hash = content_hash_hex(raw);
    let action = format!("ACTION: clip {hash} {raw}");
    let (dir, output) =
        run_wrapper_with_stubs("glob", &action, raw, format!("{raw}\n").as_bytes(), b"");
    ok(&output);
    let copied = std::fs::read(dir.join("copied.bin")).expect("wl-copy stdin");
    assert_eq!(
        copied,
        format!("{raw}\n").as_bytes(),
        "glob chars never expand: {copied:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_delete_removes_from_both_files() {
    let raw = "doomed entry";
    let hash = content_hash_hex(raw);
    let action = format!("ACTION:DELETE clip {hash} doomed entry");
    let (dir, output) = run_wrapper_with_stubs(
        "delete",
        &action,
        raw,
        b"doomed entry\nkeep me\n",
        b"doomed entry\nkeep pin\n",
    );
    ok(&output);
    assert_eq!(
        std::fs::read_to_string(dir.join("cliphist")).expect("hist"),
        "keep me\n",
        "history line removed, rest kept"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("cliphist.pins")).expect("pins"),
        "keep pin\n",
        "pin line removed, rest kept"
    );
    let log = std::fs::read_to_string(dir.join("calls.log")).expect("call log");
    assert!(log.contains("Deleted"), "delete notifies: {log:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_toggle_pins_then_unpins() {
    let raw = "flip me";
    let hash = content_hash_hex(raw);
    let action = format!("ACTION:TOGGLE clip {hash} flip me");
    // Unpinned -> pinned (bash `Pinned to history` parity).
    let (dir, output) = run_wrapper_with_stubs(
        "toggle-pin",
        &action,
        raw,
        b"flip me\nother\n",
        b"existing pin\n",
    );
    ok(&output);
    assert_eq!(
        std::fs::read_to_string(dir.join("cliphist.pins")).expect("pins"),
        "existing pin\nflip me\n",
        "toggle appends the pin"
    );
    let log = std::fs::read_to_string(dir.join("calls.log")).expect("call log");
    assert!(log.contains("Pinned to history"), "pin notifies: {log:?}");
    // Pinned -> unpinned (bash `Unpinned` parity).
    let (dir2, output2) = run_wrapper_with_stubs(
        "toggle-unpin",
        &action,
        raw,
        b"flip me\nother\n",
        b"flip me\nexisting pin\n",
    );
    ok(&output2);
    assert_eq!(
        std::fs::read_to_string(dir2.join("cliphist.pins")).expect("pins"),
        "existing pin\n",
        "toggle removes the pin"
    );
    let log2 = std::fs::read_to_string(dir2.join("calls.log")).expect("call log");
    assert!(log2.contains("Unpinned"), "unpin notifies: {log2:?}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[test]
fn wrapper_rejects_malformed_action_lines_and_ids() {
    // Garbage line.
    let (dir, output) = run_wrapper_with_stubs("bad", "GARBAGE LINE", "x", b"x\n", b"");
    assert!(!output.status.success(), "malformed ACTION: must fail");
    // Non-hex id.
    let (dir2, output2) =
        run_wrapper_with_stubs("badid", "ACTION: clip ../../escape x", "x", b"x\n", b"");
    assert!(!output2.status.success(), "non-hex id must fail");
    // Well-formed hex but unknown to the store (stale selection).
    let (dir3, output3) =
        run_wrapper_with_stubs("stale", "ACTION: clip deadbeefdeadbeef x", "", b"x\n", b"");
    assert!(!output3.status.success(), "stale id must fail");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
    let _ = std::fs::remove_dir_all(&dir3);
}

// --- Bash parity probe (live store; skips when absent) ---------------------------

/// Compare the Rust ingest against the retired bash `build_clip_rows`
/// cleaning pipeline on reference data (`$CLIPHIST_FILE` or
/// `~/.cache/cliphist`).
///
/// Bash assoc-array pin order is unspecified, so sets are compared sorted
/// while the Rust side additionally asserts pins-first + history order.
#[test]
fn parity_with_bash_pipeline_on_reference_data() {
    let home = std::env::var("HOME").unwrap_or_default();
    let hist = std::env::var(clip::HIST_ENV).unwrap_or_else(|_| format!("{home}/.cache/cliphist"));
    let pins =
        std::env::var(clip::PINS_ENV).unwrap_or_else(|_| format!("{home}/.cache/cliphist.pins"));
    if !Path::new(&hist).is_file() {
        eprintln!("parity probe: no history file at {hist}, skipping");
        return;
    }
    let script = format!(
        "{{ if [[ -s \"{pins}\" ]]; then tr -d '\\000\\037\\033' < \"{pins}\" | tr '\\t' ' '; fi; \
           if [[ -s \"{hist}\" ]]; then tac \"{hist}\"; fi; }} \
         | tr -d '\\000\\037\\033' | tr '\\t' ' ' | LC_ALL=C grep -av '^$' || true"
    );
    let bash_out = std::process::Command::new("bash")
        .arg("-c")
        .arg(&script)
        .output()
        .expect("bash pipeline runs");
    assert!(bash_out.status.success());
    // Normalize through the same lossy UTF-8 step the provider applies:
    // bash preserves invalid bytes raw while Rust surfaces U+FFFD
    // (documented lossy contract, same as the launch provider). What the
    // probe pins is everything else: cleaning, dedup, and ordering.
    let mut bash_lines: Vec<String> = bash_out
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect();
    bash_lines.sort();
    bash_lines.dedup();
    let entries = clip::load_entries_in(Path::new(&hist), Path::new(&pins));
    let mut rust_lines: Vec<String> = entries.iter().map(|entry| entry.full.clone()).collect();
    rust_lines.sort();
    rust_lines.dedup();
    eprintln!(
        "parity probe: bash {} unique lines, rust {} entries",
        bash_lines.len(),
        rust_lines.len()
    );
    assert_eq!(rust_lines, bash_lines, "cleaned line sets must match bash");
    // Rust-side order invariants (bash pins-first + tac history).
    let pin_count = entries.iter().take_while(|entry| entry.pinned).count();
    assert!(
        entries.iter().skip(pin_count).all(|entry| !entry.pinned),
        "all pins precede history"
    );
}

// --- Resolve error format (B-022) -----------------------------------------------

/// An unknown `--resolve` id is one complete diagnostic line: `main` adds
/// `flex: error:` exactly once, so the message must not carry a second
/// `flex:` prefix of its own.
#[test]
fn unknown_resolve_id_is_reported_with_a_single_prefix() {
    let home = scratch("resolve-error");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex"))
        .args(["clip", "--resolve", "deadbeef"])
        .env("HOME", &home)
        .env_remove("CLIPHIST_FILE")
        .env_remove("CLIPHIST_PINS")
        .output()
        .expect("run flex clip --resolve");
    let _ = std::fs::remove_dir_all(&home);
    assert!(!output.status.success(), "unknown id exits non-zero");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "flex: error: clip: unknown id 'deadbeef'\n"
    );
}

// --- Executor (`flex-rice/src/exec/clip.rs`) ----------------------------------
//
// The clip port of the `exec::shot` template: `ClipAction::parse` validates
// the id (`^[0-9a-f]+$`, plus the shared `noop` short-circuit), `execute`
// resolves the hash in-process (no `flex clip --resolve` subprocess) and
// runs the copy/delete/toggle steps through the stub-`PATH` tool seam
// (`wl-copy` piped, `notify-send` inherited) plus exact-line store edits
// (`CLIPHIST_FILE`/`CLIPHIST_PINS`, empty values fall back like the
// wrapper's `${VAR:-default}`). No TUI, no pty: stub `PATH` + scratch
// stores. New process-env mutations are held under `EXEC_ENV_LOCK` (shared
// with the one pre-existing env-mutating test above).
//
// Like the shot suite, the popup guard itself lives in the shared runner
// (covered by `popup.rs`), with one binary-level re-exec probe below.

/// Serialises the executor tests below: each mutates `CLIPHIST_FILE` /
/// `CLIPHIST_PINS` (the seams `execute` reads) and the harness runs tests in
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

/// Write an executable stub script.
fn write_exe(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).expect("stub script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

fn exec_stub_path(dir: &std::path::Path) -> String {
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// `wl-copy` stub capturing stdin to `copied.bin` plus `notify-send` stub,
/// both appending `argv`-joined call lines (the wrapper-parity shape).
fn install_clip_tool_stubs(dir: &std::path::Path) {
    write_exe(
        &dir.join("wl-copy"),
        &format!(
            "#!/usr/bin/env bash\ncat > \"{}/copied.bin\"\necho \"wl-copy $@\" >> \"{}/calls.log\"\n",
            dir.display(),
            dir.display()
        ),
    );
    write_exe(
        &dir.join("notify-send"),
        &format!(
            "#!/usr/bin/env bash\necho \"notify-send $@\" >> \"{}/calls.log\"\n",
            dir.display()
        ),
    );
}

/// Scratch store + stubs for one executor case: returns the dir, the store
/// paths, and the stub `PATH`. Callers hold `EXEC_ENV_LOCK` and set the two
/// store env vars themselves (so the test body shows the seam wiring).
fn setup_exec_case(name: &str, hist: &[u8], pins: &[u8]) -> (PathBuf, PathBuf, PathBuf, String) {
    let dir = scratch(&format!("exec-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    install_clip_tool_stubs(&dir);
    let hist_path = dir.join("cliphist");
    let pins_path = dir.join("cliphist.pins");
    std::fs::write(&hist_path, hist).expect("hist");
    std::fs::write(&pins_path, pins).expect("pins");
    let path_env = exec_stub_path(&dir);
    (dir, hist_path, pins_path, path_env)
}

fn read_call_log(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

/// Full copy matrix: placeholder, glob, and long entries all copy decoded
/// bytes through the `wl-copy` seam with one notify each, stores untouched.
#[test]
fn executor_copy_matrix_copies_decoded_bytes_through_wl_copy() {
    use flex_rice::exec::clip::ClipOp;
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let long = "L".repeat(200);
    let raws = [
        "alpha<NEWLINE>beta gamma".to_string(),
        "glob *[abc]? {x,y}".to_string(),
        long.clone(),
    ];
    let mut hist = String::new();
    for raw in &raws {
        hist.push_str(raw);
        hist.push('\n');
    }
    let (dir, hist_path, pins_path, path_env) = setup_exec_case("copy", hist.as_bytes(), b"");
    let _hist_env = set_exec_env(clip::HIST_ENV, &hist_path);
    let _pins_env = set_exec_env(clip::PINS_ENV, &pins_path);
    for raw in &raws {
        let id = content_hash_hex(raw);
        let report = flex_rice::exec::clip::execute(ClipOp::Copy, &id, Some(&path_env))
            .expect("copy succeeds");
        assert_eq!(report.op, Some(ClipOp::Copy));
        assert_eq!(report.raw.as_deref(), Some(raw.as_str()));
        assert_eq!(report.pinned, None, "copy reports no pin state");
        let copied = std::fs::read(dir.join("copied.bin")).expect("wl-copy stdin");
        let mut expected = clip::decode(raw).into_bytes();
        expected.push(b'\n');
        assert_eq!(
            copied, expected,
            "placeholder decoded, trailing newline kept"
        );
    }
    assert_eq!(
        read_call_log(&dir),
        format!(
            "wl-copy \n\
             notify-send -a Cliphist Copied to clipboard alpha<NEWLINE>beta gamma\n\
             wl-copy \n\
             notify-send -a Cliphist Copied to clipboard glob *[abc]? {{x,y}}\n\
             wl-copy \n\
             notify-send -a Cliphist Copied to clipboard {}\n",
            "L".repeat(50),
        ),
        "one piped wl-copy plus one notify per entry; 50-byte preview on the long one \
         (describe: `wl-copy`, `notify-send -a Cliphist <summary> <preview>`)",
    );
    assert_eq!(
        std::fs::read(&hist_path).expect("hist"),
        hist.as_bytes(),
        "copy never touches the history file"
    );
    assert!(
        std::fs::read(&pins_path).expect("pins").is_empty(),
        "copy never touches the pins file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Delete removes the exact line from both files and notifies once.
#[test]
fn executor_delete_scrubs_both_store_files() {
    use flex_rice::exec::clip::ClipOp;
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let (dir, hist_path, pins_path, path_env) = setup_exec_case(
        "delete",
        b"doomed entry\nkeep me\ndoomextended\n",
        b"doomed entry\nkeep pin\n",
    );
    let _hist_env = set_exec_env(clip::HIST_ENV, &hist_path);
    let _pins_env = set_exec_env(clip::PINS_ENV, &pins_path);
    let id = content_hash_hex("doomed entry");
    let report = flex_rice::exec::clip::execute(ClipOp::Delete, &id, Some(&path_env))
        .expect("delete succeeds");
    assert_eq!(report.op, Some(ClipOp::Delete));
    assert_eq!(report.raw.as_deref(), Some("doomed entry"));
    assert_eq!(report.pinned, None, "delete reports no pin state");
    assert_eq!(
        std::fs::read(&hist_path).expect("hist"),
        b"keep me\ndoomextended\n",
        "exact-line match only, rest kept byte-identical"
    );
    assert_eq!(
        std::fs::read(&pins_path).expect("pins"),
        b"keep pin\n",
        "pins scrubbed too"
    );
    assert_eq!(
        read_call_log(&dir),
        "notify-send -a Cliphist Deleted doomed entry\n",
        "delete is one notify, no wl-copy \
         (describe: `remove <raw> from history and pins`)"
    );
    assert!(
        !dir.join("cliphist.tmp").exists() && !dir.join("cliphist.pins.tmp").exists(),
        "tmp rewrites are renamed away"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Delete skips a missing file instead of creating it (the wrapper's
/// `[[ -f … ]]` guard).
#[test]
fn executor_delete_skips_a_missing_history_file() {
    use flex_rice::exec::clip::ClipOp;
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let (dir, hist_path, pins_path, path_env) =
        setup_exec_case("delete-missing", b"doomed entry\n", b"doomed entry\n");
    std::fs::remove_file(&hist_path).expect("remove hist");
    let _hist_env = set_exec_env(clip::HIST_ENV, &hist_path);
    let _pins_env = set_exec_env(clip::PINS_ENV, &pins_path);
    let id = content_hash_hex("doomed entry");
    flex_rice::exec::clip::execute(ClipOp::Delete, &id, Some(&path_env)).expect("delete succeeds");
    assert!(!hist_path.exists(), "missing history stays missing");
    assert_eq!(
        std::fs::read(&pins_path).expect("pins"),
        b"",
        "pins still scrubbed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Toggle appends an unpinned entry, then removes it again; reports carry
/// the post-run pin state.
#[test]
fn executor_toggle_pins_then_unpins() {
    use flex_rice::exec::clip::ClipOp;
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let (dir, hist_path, pins_path, path_env) =
        setup_exec_case("toggle", b"flip me\nother\n", b"existing pin\n");
    let _hist_env = set_exec_env(clip::HIST_ENV, &hist_path);
    let _pins_env = set_exec_env(clip::PINS_ENV, &pins_path);
    let id = content_hash_hex("flip me");
    let report =
        flex_rice::exec::clip::execute(ClipOp::Toggle, &id, Some(&path_env)).expect("pin succeeds");
    assert_eq!(report.pinned, Some(true), "now pinned");
    assert_eq!(
        std::fs::read(&pins_path).expect("pins"),
        b"existing pin\nflip me\n",
        "toggle appends the pin"
    );
    let report = flex_rice::exec::clip::execute(ClipOp::Toggle, &id, Some(&path_env))
        .expect("unpin succeeds");
    assert_eq!(report.pinned, Some(false), "now unpinned");
    assert_eq!(
        std::fs::read(&pins_path).expect("pins"),
        b"existing pin\n",
        "toggle removes the pin"
    );
    assert_eq!(
        read_call_log(&dir),
        "notify-send -a Cliphist Pinned to history flip me\n\
         notify-send -a Cliphist Unpinned flip me\n",
        "pin then unpin notifies each way (describe: `append <raw> to pins` / \
         `remove <raw> from pins`)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Toggle creates a missing pins file including parent dirs (the wrapper's
/// `mkdir -p` + `touch`).
#[test]
fn executor_toggle_creates_a_missing_pins_file() {
    use flex_rice::exec::clip::ClipOp;
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let (dir, hist_path, _pins_path, path_env) =
        setup_exec_case("toggle-new", b"flip me\n", b"existing pin\n");
    let nested = dir.join("nested").join("cliphist.pins");
    let _hist_env = set_exec_env(clip::HIST_ENV, &hist_path);
    let _pins_env = set_exec_env(clip::PINS_ENV, &nested);
    let id = content_hash_hex("flip me");
    let report =
        flex_rice::exec::clip::execute(ClipOp::Toggle, &id, Some(&path_env)).expect("pin succeeds");
    assert_eq!(report.pinned, Some(true));
    assert_eq!(
        std::fs::read(&nested).expect("pins"),
        b"flip me\n",
        "missing pins file created with the entry"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `noop` placeholder exits 0 before resolving or touching anything
/// (B-026): even unusable store paths cannot fail it, for every op.
#[test]
fn executor_noop_short_circuits_before_store_and_tools() {
    use flex_rice::exec::clip::ClipOp;
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let dir = scratch("exec-noop");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("stub dir");
    let missing_hist = dir.join("no-such-hist");
    let missing_pins = dir.join("no-such-pins");
    let _hist_env = set_exec_env(clip::HIST_ENV, &missing_hist);
    let _pins_env = set_exec_env(clip::PINS_ENV, &missing_pins);
    for op in [ClipOp::Copy, ClipOp::Delete, ClipOp::Toggle] {
        let report = flex_rice::exec::clip::execute(op, "noop", Some(&exec_stub_path(&dir)))
            .expect("noop is Ok");
        assert_eq!(report.op, None, "{op:?}: no op ran");
        assert_eq!(report.raw, None, "{op:?}: nothing resolved");
        assert_eq!(report.pinned, None, "{op:?}: no pin state");
    }
    assert!(
        !missing_hist.exists() && !missing_pins.exists(),
        "noop creates no store files"
    );
    assert!(!dir.join("calls.log").exists(), "noop spawns no tools");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Malformed ids (`bad id`, mirroring the wrapper check) and well-formed but
/// unresolvable hashes (`unknown id`) are errors with no `flex:` prefix of
/// their own — the runner adds the single prefix at the binary boundary.
#[test]
fn executor_rejects_bad_and_unknown_ids_without_its_own_prefix() {
    use flex_rice::exec::clip::ClipOp;
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let (dir, hist_path, pins_path, path_env) = setup_exec_case("bad-ids", b"real entry\n", b"");
    let _hist_env = set_exec_env(clip::HIST_ENV, &hist_path);
    let _pins_env = set_exec_env(clip::PINS_ENV, &pins_path);
    let ghost_id = content_hash_hex("ghost entry");
    let cases = [
        ("", "clip: bad id ''"),
        ("a/b", "clip: bad id 'a/b'"),
        ("a\nb", "clip: bad id 'a\nb'"),
        ("ABCDEF", "clip: bad id 'ABCDEF'"),
        ("deadbeef!", "clip: bad id 'deadbeef!'"),
        (ghost_id.as_str(), "clip: unknown id"),
    ];
    for op in [ClipOp::Copy, ClipOp::Delete, ClipOp::Toggle] {
        for (id, expected) in &cases {
            let err = flex_rice::exec::clip::execute(op, id, Some(&path_env))
                .expect_err("bad/unknown id must fail");
            let message = format!("{err:#}");
            assert!(
                message.starts_with(expected),
                "{op:?} {id:?}: unexpected message: {message:?}"
            );
            assert!(
                !message.contains("flex:"),
                "no runner prefix below the runner: {message:?}"
            );
        }
    }
    assert!(
        !dir.join("calls.log").exists(),
        "no id failure reaches the tools"
    );
    assert_eq!(
        std::fs::read(&hist_path).expect("hist"),
        b"real entry\n",
        "no id failure touches the store"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A failing or missing tool is a loud error, never a quiet cancel: clip
/// has no `slurp`-like user-cancellable step, so every tool failure exits
/// non-zero through the runner (the menu-cancel 130 lives in the binary's
/// `Outcome::Cancelled` arm, shared with shot, and needs a pty).
#[test]
fn executor_tool_failure_is_a_loud_error_not_a_quiet_cancel() {
    use flex_rice::exec::clip::ClipOp;
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let (dir, hist_path, pins_path, path_env) = setup_exec_case("tool-fail", b"entry one\n", b"");
    write_exe(&dir.join("wl-copy"), "#!/usr/bin/env bash\nexit 3\n");
    let _hist_env = set_exec_env(clip::HIST_ENV, &hist_path);
    let _pins_env = set_exec_env(clip::PINS_ENV, &pins_path);
    let id = content_hash_hex("entry one");
    let err = flex_rice::exec::clip::execute(ClipOp::Copy, &id, Some(&path_env))
        .expect_err("a failing wl-copy must fail");
    assert_eq!(format!("{err:#}"), "clip: wl-copy failed");
    // Missing tool with a tool-less PATH: resolution fails cleanly.
    let bare = dir.join("bare");
    std::fs::create_dir_all(&bare).expect("bare dir");
    let err = flex_rice::exec::clip::execute(ClipOp::Copy, &id, Some(&bare.display().to_string()))
        .expect_err("a missing wl-copy must fail");
    assert_eq!(format!("{err:#}"), "clip: wl-copy not found on PATH");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Binary level without a pty: the menu builds from the scratch store, then
/// the select loop cannot start, so the binary exits 1 with exactly one
/// `flex: error:` prefix (the runner owns it; executor errors carry none).
/// `setsid` detaches the controlling terminal so no harness tty can satisfy
/// the TUI init.
#[test]
fn binary_errors_carry_a_single_prefix() {
    let home = scratch("clip-binary-error");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".cache")).expect("cache dir");
    std::fs::write(home.join(".cache/cliphist"), b"entry one\n").expect("hist");
    let output = std::process::Command::new("setsid")
        .arg(env!("CARGO_BIN_EXE_flex-clip"))
        .env("HOME", &home)
        .env("POPUP_KITTY", "1")
        .env_remove("CLIPHIST_FILE")
        .env_remove("CLIPHIST_PINS")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-clip without a pty");
    let _ = std::fs::remove_dir_all(&home);
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
#[test]
fn binary_outside_a_popup_reexecs_into_the_wide_popup() {
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
    let path_env = exec_stub_path(&dir);
    let saved_path = std::env::var("PATH").ok();
    std::env::set_var("PATH", &path_env);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("scratch HOME");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flex-clip"))
        .env("HOME", &home)
        .env_remove("POPUP_KITTY")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run flex-clip outside a popup");
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
            && logged.contains(&String::from("flex-menu-wide"))
            && logged.contains(&String::from("POPUP_KITTY=1")),
        "re-exec uses the menu-wide popup template: {logged:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Parity with the untouched wrapper --------------------------------------
//
// For every action the wrapper supports (copy, delete, toggle both ways),
// the wrapper (under stub `PATH` with a canned `flex`) and the executor
// (over an identical store) must produce byte-identical tool-call sequences
// and byte-identical store files. The one deliberate departure: the
// wrapper's `flex clip --resolve` subprocess is resolved in-process (same
// `clip::resolve` scan the hidden `--resolve` lookup uses), so the wrapper
// makes one extra `flex` call the executor never spawns — asserted below.

/// `flex` stub answering both wrapper calls: the `ACTION:` line and the
/// `--resolve` lookup with the canned raw line.
fn write_parity_flex_stub(dir: &std::path::Path, action_line: &str, raw: &str) {
    write_exe(
        &dir.join("flex"),
        &format!(
            "#!/usr/bin/env bash\necho \"flex $@\" >> \"{}/flex.log\"\nif [[ \"${{2:-}}\" == \"--resolve\" ]]; then\nprintf '%s\\n' \"{raw}\"\nexit 0\nfi\nprintf '%s\\n' \"{action_line}\"\n",
            dir.display()
        ),
    );
}

/// Run one parity case: identical stores under `wrap/` (wrapper, canned
/// `flex`) and `exec/` (the executor), same tool stubs both sides; then
/// byte-compare the call logs, the copied bytes (copy only), and the store
/// files.
fn run_clip_parity(
    name: &str,
    op: flex_rice::exec::clip::ClipOp,
    action_word: &str,
    raw: &str,
    hist: &[u8],
    pins: &[u8],
) {
    let _env = EXEC_ENV_LOCK.lock().expect("env lock");
    let base = scratch(&format!("parity-{name}"));
    let _ = std::fs::remove_dir_all(&base);
    let wrap = base.join("wrap");
    let exec = base.join("exec");
    for dir in [&wrap, &exec] {
        std::fs::create_dir_all(dir).expect("parity dir");
        std::fs::write(dir.join("hist"), hist).expect("hist");
        std::fs::write(dir.join("pins"), pins).expect("pins");
        install_clip_tool_stubs(dir);
    }
    let hash = content_hash_hex(raw);
    write_parity_flex_stub(&wrap, &format!("{action_word} clip {hash} {raw}"), raw);
    let system_path = std::env::var("PATH").unwrap_or_default();
    let wrapper_out = std::process::Command::new("bash")
        .arg(wrapper_path())
        .env("PATH", format!("{}:{system_path}", wrap.display()))
        .env("CLIPHIST_FILE", wrap.join("hist"))
        .env("CLIPHIST_PINS", wrap.join("pins"))
        .env("POPUP_KITTY", "1")
        .output()
        .expect("run wrapper");
    assert!(
        wrapper_out.status.success(),
        "{name}: wrapper failed, stderr: {:?}",
        String::from_utf8_lossy(&wrapper_out.stderr)
    );
    let wrap_hist = wrap.join("hist");
    let wrap_pins = wrap.join("pins");
    let exec_hist = exec.join("hist");
    let exec_pins = exec.join("pins");
    let _hist_env = set_exec_env(clip::HIST_ENV, &exec_hist);
    let _pins_env = set_exec_env(clip::PINS_ENV, &exec_pins);
    let exec_result = flex_rice::exec::clip::execute(
        op,
        &hash,
        Some(&format!("{}:{system_path}", exec.display())),
    );
    assert!(
        exec_result.is_ok(),
        "{name}: executor failed: {exec_result:?}"
    );
    assert_eq!(
        std::fs::read(wrap.join("calls.log")).unwrap_or_default(),
        std::fs::read(exec.join("calls.log")).unwrap_or_default(),
        "{name}: tool-call sequences must be byte-identical"
    );
    assert_eq!(
        std::fs::read(wrap.join("copied.bin")).unwrap_or_default(),
        std::fs::read(exec.join("copied.bin")).unwrap_or_default(),
        "{name}: wl-copy stdin bytes must be byte-identical"
    );
    assert_eq!(
        std::fs::read(&wrap_hist).unwrap_or_default(),
        std::fs::read(&exec_hist).unwrap_or_default(),
        "{name}: history files must be byte-identical"
    );
    assert_eq!(
        std::fs::read(wrap_pins).unwrap_or_default(),
        std::fs::read(exec_pins).unwrap_or_default(),
        "{name}: pins files must be byte-identical"
    );
    for dir in [&wrap, &exec] {
        let tmps: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("list dir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(
            tmps.is_empty(),
            "{name}: no .tmp files left behind: {tmps:?}"
        );
    }
    // The wrapper resolves through a `flex clip --resolve` subprocess; the
    // executor resolves in-process, so only the wrapper logs it.
    let flex_log = std::fs::read_to_string(wrap.join("flex.log")).unwrap_or_default();
    assert!(
        flex_log.lines().any(|line| line.contains("--resolve")),
        "{name}: wrapper must show the elided resolve subprocess: {flex_log:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn parity_copy_matches_the_wrapper_byte_for_byte() {
    use flex_rice::exec::clip::ClipOp;
    run_clip_parity(
        "copy",
        ClipOp::Copy,
        "ACTION:",
        "alpha<NEWLINE>beta gamma",
        b"alpha<NEWLINE>beta gamma\nother\n",
        b"",
    );
}

#[test]
fn parity_delete_matches_the_wrapper_byte_for_byte() {
    use flex_rice::exec::clip::ClipOp;
    run_clip_parity(
        "delete",
        ClipOp::Delete,
        "ACTION:DELETE",
        "doomed entry",
        b"doomed entry\nkeep me\n",
        b"doomed entry\nkeep pin\n",
    );
}

#[test]
fn parity_toggle_pin_matches_the_wrapper_byte_for_byte() {
    use flex_rice::exec::clip::ClipOp;
    run_clip_parity(
        "toggle-pin",
        ClipOp::Toggle,
        "ACTION:TOGGLE",
        "flip me",
        b"flip me\nother\n",
        b"existing pin\n",
    );
}

#[test]
fn parity_toggle_unpin_matches_the_wrapper_byte_for_byte() {
    use flex_rice::exec::clip::ClipOp;
    run_clip_parity(
        "toggle-unpin",
        ClipOp::Toggle,
        "ACTION:TOGGLE",
        "flip me",
        b"flip me\nother\n",
        b"flip me\nexisting pin\n",
    );
}
