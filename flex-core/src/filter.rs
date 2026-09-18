//! Hand-rolled scored fuzzy filter.
//!
//! The predicate is an ordered subsequence (case-insensitive): anything the
//! old subsequence matcher accepted stays accepted — tiers only reorder.
//! Tiers (frozen per `Docs/Implementation.md`):
//!
//! - [`TIER_PREFIX`] (`100`): the haystack starts with the needle. A
//!   string-prefix outranks a word-prefix: `"term"` must rank `"Terminal"`
//!   above `"Gnome Terminal"` (B-003 decision).
//! - [`TIER_PREFIX_WORD`] (`80`): the whole needle is a contiguous prefix
//!   of a non-first word (`"My Firm"` vs `"fir"`).
//! - [`TIER_RUN`] (`60`): a consecutive run of at least [`MIN_RUN`] needle
//!   chars matches contiguously elsewhere.
//! - [`TIER_WORD_BOUNDARY`] (`40`): greedy match whose first char sits on a
//!   word start (start of string or after a non-alphanumeric).
//! - [`TIER_SCATTERED`] (`10`): any other ordered-subsequence match.
//!
//! Penalties: `-2` per gap char (matched span minus needle length) and `-5`
//! per leading-offset char (char index of the first match), saturating at 0.
//!
//! NOTE (B-003, resolved): `Docs/Implementation.md` froze
//! `prefix 100 > prefix-word 80`; this module implements exactly that.
//!
//! The rank signature [`score`] is `nucleo`-swappable: [`Ranker`] is the
//! seam (swap [`FuzzyRanker`] for a `nucleo`-backed ranker without touching
//! [`rank_all`] or the key/render layers).

use crate::Row;

/// Haystack starts with the needle (`"fir"` in `"Firefox"`).
pub const TIER_PREFIX: u16 = 100;
/// Whole needle prefixes a non-first word (`"fir"` in `"My Firm"`).
pub const TIER_PREFIX_WORD: u16 = 80;
/// A consecutive run of [`MIN_RUN`]+ needle chars matches contiguously.
pub const TIER_RUN: u16 = 60;
/// First match sits on a word start, but no higher tier applies.
pub const TIER_WORD_BOUNDARY: u16 = 40;
/// Any other ordered-subsequence match.
pub const TIER_SCATTERED: u16 = 10;
/// Minimum consecutive-run length for [`TIER_RUN`].
pub const MIN_RUN: usize = 3;
/// Score penalty per gap char inside the matched span.
pub const GAP_PENALTY_PER_CHAR: u16 = 2;
/// Score penalty per leading-offset char (index of the first match).
pub const LEADING_PENALTY_PER_CHAR: u16 = 5;
/// Passthrough score used for empty-needle hits (order = provider order).
pub const EMPTY_NEEDLE_SCORE: u16 = u16::MAX;

/// Rank seam: implementors map `(needle, haystack)` to a score plus the
/// matched char positions (char offsets into `haystack`, ascending), or
/// `None` when the predicate rejects the pair.
///
/// `nucleo`-swap path: implement this trait for the nucleo matcher and pass
/// it to [`rank_all_with`]; [`score`] stays as the default engine.
pub trait Ranker {
    /// Rank one pair; see [`score`] for the default semantics.
    fn rank(&self, needle: &str, haystack: &str) -> Option<(u16, Vec<u32>)>;
}

/// Which ranking engine backs [`rank_all`] (see [`App::visible_rows`]).
///
/// `Spec` is the default tiered fuzzy engine ([`score`]); `Legacy` is the R1
/// escape hatch (`--filter-mode=legacy`): the ordered-subsequence predicate
/// with provider order preserved (no score reordering).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterMode {
    /// Tiered fuzzy: `prefix 100 > prefix-word 80 > run 60 > boundary 40 >
    /// scattered 10`, minus gap/offset penalties.
    #[default]
    Spec,
    /// Subsequence predicate only; every hit scores equally so the stable
    /// index tie-break keeps provider order.
    Legacy,
}

/// Default ordered-subsequence ranker behind [`score`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FuzzyRanker;

impl Ranker for FuzzyRanker {
    fn rank(&self, needle: &str, haystack: &str) -> Option<(u16, Vec<u32>)> {
        score(needle, haystack)
    }
}

/// Legacy ordered-subsequence ranker (R1 escape hatch).
///
/// Acceptance matches [`score`] (case-insensitive ordered subsequence) but
/// every hit reports score `0` with no positions, so [`rank_all_with`]'s
/// index tie-break preserves provider order instead of reordering by tier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LegacyRanker;

impl Ranker for LegacyRanker {
    fn rank(&self, needle: &str, haystack: &str) -> Option<(u16, Vec<u32>)> {
        if needle.is_empty() {
            return Some((EMPTY_NEEDLE_SCORE, Vec::new()));
        }
        if is_subsequence(needle, haystack) {
            Some((0, Vec::new()))
        } else {
            None
        }
    }
}

/// Score `(needle, haystack)`; `None` when `needle` is not an
/// ordered subsequence of `haystack` (case-insensitive).
///
/// Positions are char offsets into `haystack` in ascending order. An empty
/// needle trivially matches with [`EMPTY_NEEDLE_SCORE`] and no positions
/// (see [`rank_all`] for the order guarantee).
///
/// # Panics
///
/// Never panics on any input: internal indexing only touches the non-empty
/// greedy match vector built above it.
#[must_use]
pub fn score(needle: &str, haystack: &str) -> Option<(u16, Vec<u32>)> {
    if needle.is_empty() {
        return Some((EMPTY_NEEDLE_SCORE, Vec::new()));
    }
    let folded_needle: Vec<char> = lowercase_chars(needle);
    let folded_hay: Vec<char> = lowercase_chars(haystack);
    if folded_needle.len() > folded_hay.len() {
        return None;
    }
    // Tier 1: full prefix.
    if folded_hay.starts_with(&folded_needle[..]) {
        return Some(finish(TIER_PREFIX, 0, folded_needle.len()));
    }
    // Tier 2: prefix of a non-first word (leftmost wins: lowest penalty).
    if let Some(word_start) = prefix_word_start(&folded_needle, &folded_hay) {
        return Some(finish(
            TIER_PREFIX_WORD,
            word_start,
            word_start + folded_needle.len(),
        ));
    }
    // Greedy leftmost ordered-subsequence alignment. This is the acceptance
    // predicate: every pair matched here is `Some`; tiers only reorder.
    // (Known limit: greedy can miss a higher-tier alignment, e.g. needle
    // `"fir"` vs `"ffir"` scores scattered instead of prefix. Acceptance is
    // unaffected — only the tier is conservative.)
    let mut positions: Vec<u32> = Vec::with_capacity(folded_needle.len());
    let mut cursor = 0_usize;
    for &wanted in &folded_needle {
        let mut found = None;
        for (index, &got) in folded_hay.iter().enumerate().skip(cursor) {
            if got == wanted {
                found = Some(index);
                cursor = index + 1;
                break;
            }
        }
        positions.push(u32::try_from(found?).unwrap_or(u32::MAX));
    }
    let tier = if longest_run(&positions) >= MIN_RUN {
        TIER_RUN
    } else if is_word_start(&folded_hay, positions[0] as usize) {
        TIER_WORD_BOUNDARY
    } else {
        TIER_SCATTERED
    };
    let first = positions[0] as usize;
    let last = positions[positions.len() - 1] as usize;
    let gaps = last - first + 1 - folded_needle.len();
    Some((apply_penalties(tier, first, gaps), positions))
}

/// One ranked row: provider index, score, and match positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedHit {
    /// Index into the provider's row slice.
    pub index: usize,
    /// Tier minus penalties (or [`EMPTY_NEEDLE_SCORE`] on empty needle).
    pub score: u16,
    /// Ascending char-offset match positions (empty on empty needle).
    pub positions: Vec<u32>,
}

/// Re-rank `rows` against `needle` with the default [`FuzzyRanker`].
///
/// - Empty needle: every row in provider order (no scoring).
/// - Otherwise: matches ordered by score descending, ties broken by the
///   original provider index ascending (stable, deterministic).
#[must_use]
pub fn rank_all(needle: &str, rows: &[Row]) -> Vec<RankedHit> {
    rank_all_with(&FuzzyRanker, needle, rows)
}

/// [`rank_all`] over an explicit [`Ranker`] (the `nucleo` seam).
#[must_use]
pub fn rank_all_with<R: Ranker>(ranker: &R, needle: &str, rows: &[Row]) -> Vec<RankedHit> {
    if needle.is_empty() {
        return rows
            .iter()
            .enumerate()
            .map(|(index, _)| RankedHit {
                index,
                score: EMPTY_NEEDLE_SCORE,
                positions: Vec::new(),
            })
            .collect();
    }
    let mut hits: Vec<RankedHit> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            ranker
                .rank(needle, &row.label)
                .map(|(score, positions)| RankedHit {
                    index,
                    score,
                    positions,
                })
        })
        .collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.index.cmp(&b.index)));
    hits
}

/// [`rank_all`] over the [`LegacyRanker`] (R1 escape hatch).
///
/// Same subsequence predicate as [`score`], but every hit scores `0`, so
/// the index tie-break preserves provider order (no score reordering).
#[must_use]
pub fn rank_all_legacy(needle: &str, rows: &[Row]) -> Vec<RankedHit> {
    rank_all_with(&LegacyRanker, needle, rows)
}

fn lowercase_chars(text: &str) -> Vec<char> {
    text.to_lowercase().chars().collect()
}

/// Whether `needle` is a case-insensitive ordered subsequence of `haystack`.
///
/// This is the shared acceptance predicate: [`score`] and [`LegacyRanker`]
/// agree on *whether* a pair matches and differ only in scoring.
#[must_use]
pub fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let folded_needle = lowercase_chars(needle);
    let folded_hay = lowercase_chars(haystack);
    if folded_needle.len() > folded_hay.len() {
        return false;
    }
    let mut cursor = 0_usize;
    for wanted in folded_needle {
        let mut found = false;
        for (index, got) in folded_hay.iter().enumerate().skip(cursor) {
            if *got == wanted {
                cursor = index + 1;
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}

/// Leftmost non-first word start where `needle` matches contiguously.
fn prefix_word_start(needle: &[char], haystack: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    for start in 1..=haystack.len() - needle.len() {
        if is_word_start(haystack, start) && haystack[start..start + needle.len()] == *needle {
            return Some(start);
        }
    }
    None
}

/// Whether `index` starts a word (string start or non-alphanumeric before).
fn is_word_start(haystack: &[char], index: usize) -> bool {
    index == 0 || !haystack[index - 1].is_alphanumeric()
}

/// Length of the longest run of consecutive positions.
fn longest_run(positions: &[u32]) -> usize {
    let mut best = 1_usize;
    let mut run = 1_usize;
    for pair in positions.windows(2) {
        if pair[1] == pair[0] + 1 {
            run += 1;
            best = best.max(run);
        } else {
            run = 1;
        }
    }
    best
}

/// Build `(score, positions)` for a contiguous `[start, end)` match.
fn finish(tier: u16, start: usize, end: usize) -> (u16, Vec<u32>) {
    let positions: Vec<u32> = (start..end)
        .map(|i| u32::try_from(i).unwrap_or(u32::MAX))
        .collect();
    (apply_penalties(tier, start, 0), positions)
}

/// Subtract gap/leading penalties, saturating at zero.
fn apply_penalties(tier: u16, leading: usize, gaps: usize) -> u16 {
    let gap_hit = u16::try_from(gaps).unwrap_or(u16::MAX);
    let lead_hit = u16::try_from(leading).unwrap_or(u16::MAX);
    tier.saturating_sub(GAP_PENALTY_PER_CHAR.saturating_mul(gap_hit))
        .saturating_sub(LEADING_PENALTY_PER_CHAR.saturating_mul(lead_hit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RowId;

    fn row(label: &str) -> Row {
        Row::new(RowId::new(label), label)
    }

    #[test]
    fn prefix_beats_scattered_for_fir() {
        let firefox = score("fir", "Firefox").expect("Firefox matches fir");
        assert_eq!(firefox.0, TIER_PREFIX);
        assert_eq!(firefox.1, vec![0, 1, 2]);
    }

    #[test]
    fn legacy_accepts_exactly_the_spec_predicate() {
        let ranker = LegacyRanker;
        for (needle, haystack) in [
            ("fir", "Firefox"),
            ("FIR", "Firefox"),
            ("fx", "Firefox"),
            ("fir", "Confirm"),
            ("fir", "My Firm"),
            ("zzz", "Firefox"),
            ("firefoxes", "Firefox"),
        ] {
            assert_eq!(
                ranker.rank(needle, haystack).is_some(),
                score(needle, haystack).is_some(),
                "predicate must agree for {needle:?} vs {haystack:?}"
            );
        }
    }

    #[test]
    fn legacy_preserves_provider_order_while_spec_reorders() {
        // Provider order is deliberately anti-score: spec must rank
        // "Firefox" (prefix 100) > "My Firm" (prefix-word 80-15=65) >
        // "Confirm" (run 60-15=45), while legacy keeps insertion order.
        let rows = vec![row("Confirm"), row("Firefox"), row("My Firm")];
        let legacy: Vec<usize> = rank_all_legacy("fir", &rows)
            .into_iter()
            .map(|hit| hit.index)
            .collect();
        assert_eq!(legacy, vec![0, 1, 2], "legacy keeps provider order");
        let spec: Vec<usize> = rank_all("fir", &rows)
            .into_iter()
            .map(|hit| hit.index)
            .collect();
        assert_eq!(spec, vec![1, 2, 0], "spec reorders by tier");
    }

    #[test]
    fn legacy_rejects_non_subsequences() {
        let rows = vec![row("Firefox"), row("Htop")];
        let hits = rank_all_legacy("fir", &rows);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].index, 0);
    }
}
