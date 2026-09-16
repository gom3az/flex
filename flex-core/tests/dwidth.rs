//! Display-width units (`src/width.rs`): ASCII / wide / emoji /
//! combining / control / tab / ZWJ, plus `truncate_exact`.
//!
//! Many fixtures intentionally use decomposed sequences (`e` + COMBINING
//! ACUTE); the lint is waived file-wide because NFC would destroy them.
//! Never "fix" these literals to precomposed `é`.
#![allow(clippy::unicode_not_nfc)]

use flex_core::width::{
    char_width, measure, sanitize, str_width, truncate_exact, truncate_measured, ELLIPSIS,
    ELLIPSIS_WIDTH, REPLACEMENT, TAB_WIDTH, ZWJ,
};

/// `e` + COMBINING ACUTE ACCENT (decomposed; never precomposed `é`).
const E_ACUTE: &str = "é";

#[test]
fn ascii_is_single_width() {
    assert_eq!(str_width("hello"), 5);
    assert_eq!(str_width(""), 0);
    assert_eq!(str_width("flex tui 123"), 12);
    assert_eq!(char_width('a'), 1);
    assert_eq!(char_width(' '), 1);
}

#[test]
fn wide_emoji_is_two_cells() {
    // Single emoji presentation char: width 2.
    assert_eq!(char_width('😀'), 2);
    assert_eq!(str_width("😀"), 2);
    assert_eq!(str_width("a😀b"), 4);
}

#[test]
fn combining_marks_are_zero_width() {
    assert_eq!(char_width('́'), 0);
    assert_eq!(str_width(E_ACUTE), 1);
    assert_eq!(str_width("éé"), 2);
}

#[test]
fn control_chars_become_replacement_width_one() {
    for c in ['\n', '\x1b', '\x07', '\u{7f}', '\u{85}'] {
        assert_eq!(char_width(c), 1, "control {c:?}");
    }
    assert_eq!(
        sanitize("a\nb\x1bc"),
        format!("a{REPLACEMENT}b{REPLACEMENT}c")
    );
    assert_eq!(str_width(&sanitize("a\nb")), 3);
}

#[test]
fn tab_expands_to_two_spaces() {
    assert_eq!(char_width('\t'), TAB_WIDTH);
    assert_eq!(TAB_WIDTH, 2);
    assert_eq!(sanitize("a\tb"), "a  b");
    assert_eq!(str_width("a\tb"), 4);
}

#[test]
fn zwj_sequences_measure_two() {
    assert_eq!(char_width(ZWJ), 0);
    // Family emoji: man ZWJ woman ZWJ girl → one glyph, width 2.
    let family = "👨‍👩‍👧";
    assert_eq!(str_width(family), 2);
    assert_eq!(str_width("a👨‍👩‍👧b"), 4);
    // Lone trailing ZWJ (no right partner) stays zero-width, not width 2.
    assert_eq!(str_width("a‍"), 1);
}

#[test]
fn ambiguous_codepoints_are_narrow() {
    // `unicode-width` built without the `cjk` feature: ambiguous → 1.
    assert_eq!(char_width('±'), 1);
    assert_eq!(char_width(ELLIPSIS), ELLIPSIS_WIDTH);
    assert_eq!(ELLIPSIS_WIDTH, 1);
    assert_eq!(str_width("a…b"), 3);
}

#[test]
fn truncate_exact_units() {
    // Fits: unchanged (verbatim).
    assert_eq!(truncate_exact("hello", 5), "hello");
    assert_eq!(truncate_exact("hi", 5), "hi");
    assert_eq!(truncate_exact("", 0), "");
    // Overflow: longest prefix within avail-1 plus ellipsis.
    assert_eq!(truncate_exact("hello world", 5), "hell…");
    assert_eq!(truncate_exact("hello", 1), "…");
    assert_eq!(truncate_exact("hello", 0), "");
    // Exact-boundary: width == avail returns whole label, no ellipsis.
    assert_eq!(truncate_exact("hello", 6), "hello");
    // Wide chars: never split a cell; ellipsis keeps total ≤ avail.
    assert_eq!(truncate_exact("a😀bc", 3), "a…");
    assert_eq!(truncate_exact("a😀bc", 4), "a😀…");
    assert_eq!(truncate_exact("a😀bc", 5), "a😀bc");
    // Combining marks attach for free, never orphaned from base.
    assert_eq!(truncate_exact("ééé", 2), "é…");
    assert_eq!(truncate_exact("éxyz", 2), "é…");
    // Tabs measure 2 (raw slice keeps the tab; render sanitizes).
    assert_eq!(truncate_exact("a\tbc", 4), "a\t…");
    assert_eq!(sanitize(&truncate_exact("a\tbc", 4)), "a  …");
    // Controls measure 1 (rendered as U+FFFD downstream).
    assert_eq!(truncate_exact("ab\ncde", 4), "ab\n…");
}

#[test]
fn truncate_is_width_exact() {
    let labels = [
        "Shutdown the machine now please",
        "a😀b👨‍👩‍👧c",
        "ééééé",
        "tab\there",
        "ctrl\x1bchars",
        "±…±…±…±…±…",
    ];
    for label in labels {
        for avail in 0..20 {
            let out = truncate_exact(label, avail);
            let measured = str_width(&sanitize(&out));
            assert!(
                measured <= avail,
                "truncate_exact({label:?}, {avail}) = {out:?} (width {measured})"
            );
            if str_width(label) > avail && avail > 0 {
                assert!(
                    out.ends_with(ELLIPSIS),
                    "overflow must end in ellipsis: {out:?}"
                );
            }
        }
    }
}

#[test]
fn precomputed_measure_agrees_with_direct_truncate() {
    let label = "a😀b👨‍👩‍👧é\t\nz";
    let measured = measure(label);
    assert_eq!(measured.width, str_width(label));
    // Clusters tile the source bytes exactly.
    let mut cursor = 0;
    for cluster in &measured.clusters {
        assert_eq!(cluster.start, cursor);
        cursor = cluster.end;
    }
    assert_eq!(cursor, label.len());
    for avail in 0..18 {
        assert_eq!(
            truncate_measured(label, &measured, avail),
            truncate_exact(label, avail),
            "avail {avail}"
        );
    }
}
