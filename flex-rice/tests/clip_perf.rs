//! M4 clip perf gate: cold 3200-row file ingest → first frame ≤ 0.5 s.
//!
//! Unlike the M1 spike (in-memory synthetic corpus), this exercises the
//! real provider path end to end: temp `CLIPHIST_FILE` / `CLIPHIST_PINS`
//! files on disk (with NUL / invalid-UTF-8 / ESC / tab bytes spliced in,
//! like the reference store) → [`clip::load_entries`] → tab → first-frame
//! render @80x24 through the real renderer.
//!
//! The gate runs in debug AND release (`cargo test` and
//! `cargo test --release`); the release run is the M4 cutover gate. The M1
//! spike measured ~1.8 ms for the in-memory path — file IO adds only a
//! small constant, so 0.5 s still leaves two orders of margin.

use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use flex_core::backend::FLEX_TEST_SEED;
use flex_core::{render, synthetic_clip_corpus, width};
use flex_rice::providers::clip;

/// Cold ingest → first frame must fit the 0.5 s popup-open budget.
#[test]
fn clip_cold_ingest_to_first_frame_under_budget() {
    let dir = std::env::temp_dir().join(format!("flex-clip-perf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("perf dir");
    let hist = dir.join("cliphist");
    let pins = dir.join("cliphist.pins");

    // 3200-row history file shaped like the reference store, with binary
    // hazards spliced in (row 42 carries invalid UTF-8; row 7 a NUL/ESC).
    let corpus = synthetic_clip_corpus(3200, FLEX_TEST_SEED);
    let mut blob: Vec<u8> = Vec::new();
    for (i, row) in corpus.iter().enumerate() {
        blob.extend_from_slice(row.label.as_bytes());
        if i == 42 {
            blob.extend_from_slice(&[0xff, 0xfe]);
        }
        if i == 7 {
            blob.extend_from_slice(b"\x00\x1b\t");
        }
        blob.push(b'\n');
    }
    std::fs::write(&hist, blob).expect("hist fixture");
    std::fs::write(&pins, "pinned perf entry\n").expect("pins fixture");

    let previous_hist = std::env::var(clip::HIST_ENV).ok();
    let previous_pins = std::env::var(clip::PINS_ENV).ok();
    std::env::set_var(clip::HIST_ENV, &hist);
    std::env::set_var(clip::PINS_ENV, &pins);

    let started = Instant::now();
    // Cold ingest: file read + byte cleaning + hash + preview + widths.
    let entries = clip::load_entries();
    assert_eq!(entries.len(), 3201, "3200 history + 1 pin, deduped");
    assert!(
        entries.iter().any(|entry| entry.full.contains('�')),
        "invalid bytes surface as U+FFFD, lossy but safe"
    );
    assert!(
        entries[0].pinned && entries[0].full == "pinned perf entry",
        "pins-first ordering on the cold path"
    );
    // Precomputed widths agree with the renderer math.
    let total_cells: usize = entries
        .iter()
        .map(|entry| {
            assert_eq!(
                entry.measured.width,
                width::str_width(&entry.row.label),
                "precomputed width matches"
            );
            entry.measured.width
        })
        .sum();
    assert!(total_cells > 3200, "labels have nonzero width");
    // First frame @80x24 through the real renderer.
    let mut menu = flex_rice::menu(clip::PROVIDER, vec![clip::tab_from_entries(&entries)]);
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("TestBackend terminal");
    terminal
        .draw(|frame| render::render(frame, &mut menu))
        .expect("render frame");
    let elapsed = started.elapsed();

    match previous_hist {
        Some(value) => std::env::set_var(clip::HIST_ENV, value),
        None => std::env::remove_var(clip::HIST_ENV),
    }
    match previous_pins {
        Some(value) => std::env::set_var(clip::PINS_ENV, value),
        None => std::env::remove_var(clip::PINS_ENV),
    }
    let _ = std::fs::remove_dir_all(&dir);

    eprintln!("clip gate: 3200-row cold ingest→first-frame took {elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(500),
        "M4 budget is 0.5 s, took {elapsed:?}"
    );
}
