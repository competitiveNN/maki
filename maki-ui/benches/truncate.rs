//! Costs of the display-width truncation primitives the UI calls on every
//! frame: `head_fitting`/`tail_fitting` walk graphemes and measure each one,
//! so the shape of the text (ASCII vs wide vs ZWJ clusters) matters.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use maki_ui::truncate::{head_fitting, tail_fitting};
use unicode_width::UnicodeWidthStr;

/// A long ASCII run, the common case for labels and paths.
const ASCII: &str = "the-quick-brown-fox-jumps-over-the-lazy-dog-again-and-again-forever";
/// Three-byte CJK, two cells each, so bytes, chars, and cells all disagree.
const CJK: &str = "日本語のテキストは表示幅で数える必要がありますそして切り詰めます";
/// ZWJ family emoji repeated: one grapheme of many bytes, two cells wide.
const ZWJ: &str = "👨‍👩‍👧👨‍👩‍👧👨‍👩‍👧👨‍👩‍👧👨‍👩‍👧👨‍👩‍👧👨‍👩‍👧👨‍👩‍👧";
/// Combining marks: width comes from the base char only.
const COMBINING: &str = "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}";

fn bench_head(c: &mut Criterion) {
    let mut group = c.benchmark_group("truncate");
    for (name, text) in [
        ("ascii", ASCII),
        ("cjk", CJK),
        ("zwj", ZWJ),
        ("combining", COMBINING),
    ] {
        let budget = text.width() / 2 + 1;
        group.bench_function(format!("head_fitting/{name}"), |b| {
            b.iter(|| head_fitting(black_box(text), black_box(budget)))
        });
    }
    group.finish();
}

fn bench_tail(c: &mut Criterion) {
    let mut group = c.benchmark_group("truncate");
    for (name, text) in [
        ("ascii", ASCII),
        ("cjk", CJK),
        ("zwj", ZWJ),
        ("combining", COMBINING),
    ] {
        let budget = text.width() / 2 + 1;
        group.bench_function(format!("tail_fitting/{name}"), |b| {
            b.iter(|| tail_fitting(black_box(text), black_box(budget)))
        });
    }
    group.finish();
}

/// Truncating a row that already fits is the hot path: nothing is cut, so the
/// early `width() <= max` return has to stay cheap.
fn bench_fits(c: &mut Criterion) {
    let mut group = c.benchmark_group("truncate");
    for (name, text) in [
        ("ascii", ASCII),
        ("cjk", CJK),
        ("zwj", ZWJ),
        ("combining", COMBINING),
    ] {
        let budget = text.width() + 8;
        group.bench_function(format!("head_fitting_fits/{name}"), |b| {
            b.iter(|| head_fitting(black_box(text), black_box(budget)))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_head, bench_tail, bench_fits);
criterion_main!(benches);
