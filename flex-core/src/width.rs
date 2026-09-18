//! Display-width units via `unicode-width 0.2` (non-CJK) plus `flex`
//! rendering rules.
//!
//! Rules (normative for `crate::render` and `tests/dwidth.rs`):
//!
//! - ASCII printable text is width 1 per char.
//! - Control chars (`char::is_control`, e.g. `\n`, `\x1b`, DEL) render as
//!   U+FFFD (`�`) with width 1.
//! - `\t` expands to two spaces (width 2).
//! - Combining marks contribute width 0 (they attach to the previous cell).
//! - Emoji ZWJ sequences (`A\u{200D}B…`, e.g. `👨‍👩‍👧`) measure width 2
//!   total, matching `unicode-width`'s presentation width.
//! - East-Asian Ambiguous codepoints are **narrow** (width 1): the crate is
//!   built with `unicode-width/default-features = false` (no `cjk` feature).
//! - CJK wide text is out of scope for v1 (renders, but truncation only
//!   guarantees the width math, not glyph coverage).
//!
//! The [`measure`] + [`truncate_measured`] pair is the precompute-friendly
//! API: measure a label once (e.g. when rows are built), then truncate per
//! frame without re-scanning.

use std::borrow::Cow;

use unicode_width::UnicodeWidthChar;

/// Ellipsis appended by [`truncate_exact`] (`U+2026`, width 1).
pub const ELLIPSIS: char = '…';
/// Display width of [`ELLIPSIS`] in cells.
pub const ELLIPSIS_WIDTH: usize = 1;
/// Replacement char rendered for control codes (width 1).
pub const REPLACEMENT: char = '\u{FFFD}';
/// Expanded width of `\t` (two spaces).
pub const TAB_WIDTH: usize = 2;
/// Zero-width joiner: links neighbouring chars into one width-2 cluster.
pub const ZWJ: char = '\u{200D}';

/// Display width of a single char under the rules above.
///
/// Control chars report 1 (they render as [`REPLACEMENT`]); `\t` reports
/// [`TAB_WIDTH`]; [`ZWJ`] reports 0 (it joins a cluster); everything else
/// defers to `unicode-width` (non-CJK: ambiguous narrow, combining 0,
///
/// emoji presentation 2).
#[must_use]
pub fn char_width(c: char) -> usize {
    if c == '\t' {
        return TAB_WIDTH;
    }
    if c == ZWJ {
        return 0;
    }
    if c.is_control() {
        return 1;
    }
    c.width().unwrap_or(1)
}

/// Display width of `text` in cells (see [`measure`]).
#[must_use]
pub fn str_width(text: &str) -> usize {
    measure(text).width
}

/// One display cluster: a byte range plus its cell width.
///
/// A cluster is either a single char, or a maximal `A\u{200D}B…` run joined
/// by [`ZWJ`] on both sides (a lone leading/trailing/doubled ZWJ does not
/// join and forms its own zero-width cluster). Joined clusters have fixed
/// width 2; otherwise the width is the sum of [`char_width`] (so combining
/// marks contribute 0 and stay attached to their base char).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    /// Byte offset of the cluster start in the source string.
    pub start: usize,
    /// Byte offset one past the cluster end in the source string.
    pub end: usize,
    /// Display width in cells.
    pub width: usize,
    /// Whether the cluster contains a joining ZWJ (fixed width 2).
    pub joined: bool,
}

/// Precomputed measurement of a string (see [`measure`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Measured {
    /// Total display width in cells.
    pub width: usize,
    /// Display clusters in source order; byte ranges tile `[0, len)`.
    pub clusters: Vec<Cluster>,
}

/// Measure `text` once; reuse the result with [`truncate_measured`].
///
/// Byte ranges in [`Measured::clusters`] always tile the source string, so
/// truncating at a cluster boundary never splits a UTF-8 sequence.
///
/// The scan is allocation-free (streaming `char_indices` with a cloned
/// lookahead for `ZWJ` pairs); only [`Measured::clusters`] grows.
///
/// # Panics
///
/// Never panics on valid `&str` input: the two internal `expect`s only fire
/// when a peeked/probed `char` vanishes from the same iterator, which is
/// impossible without concurrent mutation (and `&str` is immutable).
#[must_use]
pub fn measure(text: &str) -> Measured {
    let mut out = Measured::default();
    let mut chars = text.char_indices().peekable();
    while let Some((start, head)) = chars.next() {
        let mut end = start + head.len_utf8();
        let mut joined = false;
        // Extend over `ZWJ non-ZWJ` pairs: the ZWJ must have a partner on
        // both sides to join (lone ZWJ stays a zero-width cluster). The
        // probe clones the iterator state (`CharIndices` is `Clone`, no
        // allocation) so a failed pair costs nothing.
        while chars.peek().is_some_and(|&(_, c)| c == ZWJ) {
            let mut probe = chars.clone();
            probe.next();
            let pair = probe.next().is_some_and(|(_, c)| c != ZWJ);
            if !pair {
                break;
            }
            let (_, zwj) = chars.next().expect("peeked ZWJ");
            debug_assert!(zwj == ZWJ);
            let (next_start, next) = chars.next().expect("probed pair tail");
            end = next_start + next.len_utf8();
            joined = true;
        }
        // A non-joined cluster is exactly one char (extension only happens
        // via pairs above), so its width is a single [`char_width`].
        let width = if joined { 2 } else { char_width(head) };
        out.width += width;
        out.clusters.push(Cluster {
            start,
            end,
            width,
            joined,
        });
    }
    out
}

/// Replace control chars with [`REPLACEMENT`] and expand `\t` to two
/// spaces, preserving ZWJ/combining structure so [`measure`] agrees with
/// the rendered output.
///
/// Width is preserved exactly: every replacement has the same width as the
/// rule in [`char_width`] assigns the original.
#[must_use]
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\t' {
            out.push_str("  ");
        } else if c.is_control() {
            out.push(REPLACEMENT);
        } else {
            out.push(c);
        }
    }
    out
}

/// Truncate `label` to exactly fit `avail` display cells.
///
/// - If the label fits (`width <= avail`) it is returned unchanged.
/// - Otherwise the longest cluster-boundary prefix with
///   `width <= avail - 1` is returned with [`ELLIPSIS`] appended, so the
///   result is exactly `<= avail` cells wide.
/// - `avail == 0` yields `""`; `avail == 1` with overflow yields `"…"`.
/// - Zero-width clusters (combining marks) attach for free and are never
///   split from their base char.
///
/// The input is used verbatim (callers render via [`sanitize`], which
/// preserves widths, so measuring the raw label stays correct).
#[must_use]
pub fn truncate_exact(label: &str, avail: usize) -> String {
    let measured = measure(label);
    truncate_measured(label, &measured, avail)
}

/// [`truncate_exact`] over a precomputed [`measure`] result.
///
/// # Panics
///
/// Panics if `measured` was not produced by [`measure`] for `label` with
/// the same length (byte ranges are indexed into `label`).
#[must_use]
pub fn truncate_measured(label: &str, measured: &Measured, avail: usize) -> String {
    assert_eq!(
        measured.clusters.last().map_or(0, |c| c.end),
        label.len(),
        "Measured does not match label"
    );
    if measured.width <= avail {
        return label.to_string();
    }
    if avail == 0 {
        return String::new();
    }
    let budget = avail - ELLIPSIS_WIDTH;
    let mut end = 0_usize;
    let mut width = 0_usize;
    for cluster in &measured.clusters {
        if width + cluster.width > budget {
            break;
        }
        width += cluster.width;
        end = cluster.end;
    }
    let mut out = label[..end].to_string();
    out.push(ELLIPSIS);
    debug_assert!(str_width(&out) <= avail);
    out
}

/// [`truncate_measured`] without the fit-case allocation.
///
/// Returns a borrow when the label already fits (`width <= avail`), else an
/// owned truncated string. Draw paths render borrowed spans through this so
/// fitting labels cost nothing per frame.
///
/// # Panics
///
/// Panics when `measured` was not built from `label` (the `assert_eq!`
/// pins the OPT-3 cache contract: byte ranges must tile `label.len()`).
#[must_use]
pub fn truncate_measured_cow<'a>(
    label: &'a str,
    measured: &Measured,
    avail: usize,
) -> Cow<'a, str> {
    assert_eq!(
        measured.clusters.last().map_or(0, |c| c.end),
        label.len(),
        "Measured does not match label"
    );
    if measured.width <= avail {
        return Cow::Borrowed(label);
    }
    Cow::Owned(truncate_measured(label, measured, avail))
}

/// Sanitized text bundled with its own [`measure`] result.
///
/// [`sanitize`] preserves widths exactly, so measuring the sanitized text
/// agrees with measuring the raw label while the byte ranges index the
/// string the draw paths actually render. Build once per content change,
/// then truncate per frame with [`truncate_measured`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sanitized {
    /// [`sanitize`] output (what the draw paths render).
    pub text: String,
    /// [`measure`] over [`Sanitized::text`].
    pub measured: Measured,
}

/// [`sanitize`] + [`measure`] in one pass over the input.
#[must_use]
pub fn sanitize_measured(raw: &str) -> Sanitized {
    let text = sanitize(raw);
    let measured = measure(&text);
    Sanitized { text, measured }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanity_ascii_and_ellipsis() {
        assert_eq!(str_width("hello"), 5);
        assert_eq!(char_width(ELLIPSIS), ELLIPSIS_WIDTH);
        assert_eq!(truncate_exact("hello world", 5), "hell…");
        assert_eq!(truncate_exact("hi", 5), "hi");
        assert_eq!(truncate_exact("hello", 5), "hello");
    }
}
