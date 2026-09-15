//! Fuzzy corpus (`src/filter.rs`): tier/penalty regression over a fixed
//! 20-name launcher corpus, plus the old-matcher superset property.
//!
//! Corpus needle: `"fir"`. `"Firefox"` (prefix tier) must rank above the
//! `Calf`- and `Fridge`-class entries.

use flex_core::filter::{
    rank_all, score, GAP_PENALTY_PER_CHAR, LEADING_PENALTY_PER_CHAR, TIER_PREFIX, TIER_PREFIX_WORD,
    TIER_RUN, TIER_SCATTERED, TIER_WORD_BOUNDARY,
};
use flex_core::{Row, RowId};

/// Fixed 20-name launcher corpus (provider order = index order).
fn corpus() -> Vec<Row> {
    let names = [
        "Firefox",         // 0: prefix → top
        "Files",           // 1: f@0,i@1, no `r` → None
        "Terminal",        // 2: no `f` → None
        "Calculator",      // 3: `Calf`-class, no `f` → None
        "Calf",            // 4: `Calf`-class: no `i` → None
        "Fridge",          // 5: `Fridge`-class: f,r,i — `r` after `i`? none → None
        "Old Fridge",      // 6: `Fridge`-class: none → None
        "Confirm",         // 7: run `fir` @3 → tier 60
        "Call First",      // 8: prefix-word @5 → 80-25 = 55 (below prefix)
        "Free Irina",      // 9: boundary @0, gappy → tier 40
        "Affirm",          // 10: scattered f@1,i@3,r@4
        "First Aid",       // 11: prefix → tier 100
        "Airplane Mode",   // 12: no `f` → None
        "Screenshot",      // 13: none (no f)
        "Filter Editor",   // 14: f@0,i@1,r@5 → boundary, gappy
        "Fireplace",       // 15: prefix → tier 100
        "Wireless",        // 16: none? w,i,r…: f? none → None
        "Infrared",        // 17: scattered? I,n,f,r…: f@2,i? after: r,e,d — no i → None
        "Firmware Update", // 18: prefix → tier 100
        "Calendar",        // 19: `Calf`-class: f? none → None
    ];
    names
        .iter()
        .enumerate()
        .map(|(i, name)| Row::new(RowId::new(format!("row-{i:02}")), (*name).to_string()))
        .collect()
}

#[test]
fn fir_ranks_firefox_above_calf_and_fridge_class() {
    let rows = corpus();
    let hits = rank_all("fir", &rows);
    assert!(!hits.is_empty(), "fir must match something");
    // Firefox (prefix) is the top hit.
    assert_eq!(rows[hits[0].index].label, "Firefox");
    // Calf/Fridge-class entries never match `fir` as an ordered
    // subsequence, so they rank below every match (they are filtered out).
    let matched: Vec<&str> = hits
        .iter()
        .map(|hit| rows[hit.index].label.as_str())
        .collect();
    for excluded in ["Calf", "Calculator", "Calendar", "Fridge", "Old Fridge"] {
        assert!(
            !matched.contains(&excluded),
            "{excluded:?} must not match `fir`; matched: {matched:?}"
        );
    }
}

#[test]
fn corpus_has_twenty_names() {
    assert_eq!(corpus().len(), 20);
}

#[test]
fn tier_prefix() {
    let (got, positions) = score("fir", "Firefox").expect("prefix matches");
    assert_eq!(got, TIER_PREFIX);
    assert_eq!(positions, vec![0, 1, 2]);
    // Case-insensitive.
    let (upper_score, _) = score("FIR", "firefox").expect("upper needle");
    assert_eq!(upper_score, TIER_PREFIX);
}

#[test]
fn tier_prefix_word() {
    // Whole needle prefixes a non-first word.
    let (got, positions) = score("fir", "My Firm").expect("prefix-word matches");
    assert_eq!(got, TIER_PREFIX_WORD - 3 * LEADING_PENALTY_PER_CHAR);
    assert_eq!(positions, vec![3, 4, 5]);
}

#[test]
fn string_prefix_beats_word_prefix() {
    // B-003: `"term"` must rank `"Terminal"` (string prefix) above
    // `"Gnome Terminal"` (word prefix at offset 6).
    let (string_score, _) = score("term", "Terminal").expect("prefix matches");
    let (word_score, _) = score("term", "Gnome Terminal").expect("word-prefix matches");
    assert_eq!(string_score, TIER_PREFIX);
    assert_eq!(word_score, TIER_PREFIX_WORD - 6 * LEADING_PENALTY_PER_CHAR);
    assert!(string_score > word_score);
    // Same for the corpus needle: Firefox (prefix) above My Firm (word).
    let (firefox, _) = score("fir", "Firefox").expect("prefix");
    let (firm, _) = score("fir", "My Firm").expect("word-prefix");
    assert!(firefox > firm);
}

#[test]
fn tier_consecutive_run() {
    // `fir` runs contiguously at offset 3 inside "Confirm".
    // (MIN_RUN is 3; the run here is exactly 3, so this tier applies.)
    let (got, positions) = score("fir", "Confirm").expect("run matches");
    assert_eq!(positions, vec![3, 4, 5]);
    assert_eq!(got, TIER_RUN - 3 * LEADING_PENALTY_PER_CHAR);
}

#[test]
fn tier_word_boundary() {
    // `Free Irina`: f@0 (word start), then a gappy tail — no run ≥ 3.
    let (got, positions) = score("fir", "Free Irina").expect("boundary matches");
    assert_eq!(positions[0], 0);
    let span = positions[2]
        .saturating_sub(positions[0])
        .saturating_add(1)
        .saturating_sub(3);
    let gap = u16::try_from(span).unwrap_or(u16::MAX);
    assert_eq!(got, TIER_WORD_BOUNDARY - gap * GAP_PENALTY_PER_CHAR);
}

#[test]
fn tier_scattered_with_penalties() {
    // `Affirm`: a,f,f,i,r,m → f@1, i@3, r@4: scattered, 1 gap, leading 1.
    let (got, positions) = score("fir", "Affirm").expect("scattered matches");
    assert_eq!(positions, vec![1, 3, 4]);
    assert_eq!(
        got,
        TIER_SCATTERED - GAP_PENALTY_PER_CHAR - LEADING_PENALTY_PER_CHAR
    );
}

#[test]
fn penalties_floor_at_zero() {
    // Far-offset gappy match saturates instead of underflowing.
    let hay = "x".repeat(60) + "f" + &"x".repeat(30) + "ir";
    let (far_score, _) = score("fir", &hay).expect("far match still matches");
    assert_eq!(far_score, 0);
}

#[test]
fn empty_needle_preserves_provider_order() {
    let rows = corpus();
    let hits = rank_all("", &rows);
    let order: Vec<usize> = hits.iter().map(|hit| hit.index).collect();
    assert_eq!(order, (0..rows.len()).collect::<Vec<_>>());
}

#[test]
fn ties_break_by_original_index() {
    let rows = corpus();
    let hits = rank_all("fir", &rows);
    // "Firefox" (0), "First Aid" (11), "Fireplace" (15), "Firmware…" (18)
    // all score TIER_PREFIX with no penalties: index-ascending tie-break.
    let prefix_hits: Vec<usize> = hits
        .iter()
        .filter(|hit| hit.score == TIER_PREFIX)
        .map(|hit| hit.index)
        .collect();
    assert_eq!(prefix_hits, vec![0, 11, 15, 18]);
    let mut sorted = prefix_hits.clone();
    sorted.sort_unstable();
    assert_eq!(prefix_hits, sorted);
}

#[test]
fn score_returns_ascending_match_positions() {
    let (_, positions) = score("fir", "Free Irina").expect("matches");
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(positions, sorted);
    assert_eq!(positions.len(), 3);
}

/// The pre-M1 reference matcher: pure ordered subsequence, case-insensitive.
/// Property: every pair the old matcher accepted stays accepted (`Some`).
fn old_matcher_accepts(needle: &str, haystack: &str) -> bool {
    let folded_needle = needle.to_lowercase();
    let folded_hay = haystack.to_lowercase();
    let mut cursor = folded_hay.chars();
    'outer: for wanted in folded_needle.chars() {
        for got in cursor.by_ref() {
            if got == wanted {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

#[test]
fn old_superset_property_on_corpus_pairs() {
    let rows = corpus();
    let needles = [
        "fir", "fire", "a", "e", "zz", "fm", "FIR", "up", "re", "irm",
    ];
    for needle in needles {
        for row in &rows {
            if old_matcher_accepts(needle, &row.label) {
                assert!(
                    score(needle, &row.label).is_some(),
                    "old matcher accepted ({needle:?}, {:?}) but score() rejected it",
                    row.label
                );
            } else {
                assert!(
                    score(needle, &row.label).is_none(),
                    "score() accepted ({needle:?}, {:?}) which the old matcher rejects",
                    row.label
                );
            }
        }
    }
}

#[test]
// `"éfir"` below is decomposed (`e` + COMBINING ACUTE) on purpose: the
// property must hold for non-NFC input too. Never NFC-normalize it.
#[allow(clippy::unicode_not_nfc)]
fn old_superset_property_fuzz_shapes() {
    // Deterministic pseudo-fuzz over shapes: unicode, spaces, repeats.
    let hays = [
        "a",
        "AA",
        "a b c",
        "Éclair",
        "naïve fir",
        "ffir",
        "fifr",
        "irf",
        "F I R",
        "firfir",
        "xxxx",
        "",
        "f",
        "fi",
        "😀fir",
        "fir👨‍👩‍👧",
        "éfir",
    ];
    let needles = ["", "f", "fir", "FIR", "fri", "ff", "i", "😀", " ", "firf"];
    for needle in needles {
        for hay in hays {
            let old = old_matcher_accepts(needle, hay) || needle.is_empty();
            assert_eq!(
                score(needle, hay).is_some(),
                old,
                "predicate mismatch for ({needle:?}, {hay:?})"
            );
        }
    }
}
