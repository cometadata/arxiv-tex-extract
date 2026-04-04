use std::collections::HashMap;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use latex_extract::cleanup::cleanup;
use latex_extract::comments::remove_comments;
use latex_extract::diacritics::convert_diacritics;
use latex_extract::environments::convert_environments;
use latex_extract::formatting::convert_formatting;
use latex_extract::input_resolve::TexFile;
use latex_extract::pipeline;
use latex_extract::preamble::extract_body;
use latex_extract::references::convert_references;
use latex_extract::structure::convert_structure;
use latex_extract::symbols::convert_symbols;

fn load_fixture(name: &str) -> String {
    std::fs::read_to_string(format!("benches/fixtures/{}", name))
        .unwrap_or_else(|_| panic!("fixture file benches/fixtures/{} not found", name))
}

/// Full extraction pipeline on different document sizes.
fn bench_extract_single(c: &mut Criterion) {
    let mut group = c.benchmark_group("extract_single");

    for (label, fixture) in &[
        ("small_31kb", "small_paper.tex"),
        ("medium_66kb", "medium_paper.tex"),
        ("large_134kb", "large_paper.tex"),
    ] {
        let content = load_fixture(fixture);
        let files = vec![TexFile {
            name: "main.tex".into(),
            content,
        }];

        group.bench_with_input(BenchmarkId::from_parameter(label), &files, |b, files| {
            b.iter(|| pipeline::extract_text(files))
        });
    }

    group.finish();
}

/// Individual pipeline stages on the medium fixture to identify bottlenecks.
fn bench_pipeline_stages(c: &mut Criterion) {
    let content = load_fixture("medium_paper.tex");
    let (_extras, body) = extract_body(&content);
    let after_comments = remove_comments(&body);

    let mut group = c.benchmark_group("pipeline_stages");

    group.bench_function("extract_body", |b| b.iter(|| extract_body(&content)));

    group.bench_function("remove_comments", |b| {
        b.iter(|| remove_comments(&body))
    });

    group.bench_function("convert_structure", |b| {
        b.iter(|| {
            let mut labels = HashMap::new();
            convert_structure(&after_comments, &mut labels)
        })
    });

    group.bench_function("convert_formatting", |b| {
        b.iter(|| convert_formatting(&after_comments))
    });

    group.bench_function("convert_references", |b| {
        b.iter(|| {
            let labels = HashMap::new();
            convert_references(&after_comments, &labels)
        })
    });

    group.bench_function("convert_environments", |b| {
        b.iter(|| {
            let theorems = HashMap::new();
            convert_environments(&after_comments, &theorems)
        })
    });

    group.bench_function("convert_diacritics", |b| {
        b.iter(|| convert_diacritics(&after_comments))
    });

    group.bench_function("convert_symbols", |b| {
        b.iter(|| convert_symbols(&after_comments))
    });

    group.bench_function("cleanup", |b| b.iter(|| cleanup(&after_comments)));

    group.finish();
}

criterion_group!(benches, bench_extract_single, bench_pipeline_stages);
criterion_main!(benches);
