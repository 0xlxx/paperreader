//! TOC extraction benchmarks.
//!
//! Run with: `cargo bench --bench toc_bench`
//!
//! Representative files (edit paths for your machine):
//!   - history: indexed CJK textbook, plain fast path
//!   - math7:   indexed CJK textbook, layout path (garbled plain text)
//!   - gea2:    indexed tech book, printed TOC without extractable page numbers
use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};
use paperreader::toc;

const CASES: &[(&str, &str)] = &[
    ("history_plain", "/Users/bjorn/Documents/ChinaTextbooks/历史/高中/中外历史纲要（上）.pdf"),
    ("math7_layout", "/Users/bjorn/Documents/ChinaTextbooks/数学/初中/数学七年级上册.pdf"),
    ("gea2_no_pagenums", "/Users/bjorn/learn/game-engine/papers/GEA_Vol2_Graphics.pdf"),
];

fn bench_toc(c: &mut Criterion) {
    for (name, path) in CASES {
        let p = Path::new(path);
        if !p.exists() {
            eprintln!("skip missing: {path}");
            continue;
        }
        c.bench_function(name, |b| {
            b.iter(|| {
                let report = toc::detect_toc(p, false);
                std::hint::black_box(report.entries.len());
            })
        });
    }
}

criterion_group!(benches, bench_toc);
criterion_main!(benches);
