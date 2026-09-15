//! Re-rank benchmark (M1 spike shape, M5 regression gate).
//!
//! Measures tiered fuzzy re-rank over the deterministic 3200-row clipboard
//! corpus (`synthetic_clip_corpus` at `FLEX_TEST_SEED`) across a mixed
//! query set, so criterion tracks the per-keystroke budget signal.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use flex_core::backend::FLEX_TEST_SEED;
use flex_core::filter::rank_all;
use flex_core::synthetic_clip_corpus;

fn rerank_3200(c: &mut Criterion) {
    let rows = synthetic_clip_corpus(3200, FLEX_TEST_SEED);
    let queries = ["fir", "clip", "a", "term", "zzz-no-match"];
    c.bench_function("rerank_3200_mixed", |b| {
        b.iter(|| {
            for query in queries {
                black_box(rank_all(black_box(query), black_box(&rows)));
            }
        });
    });
}

criterion_group!(benches, rerank_3200);
criterion_main!(benches);
