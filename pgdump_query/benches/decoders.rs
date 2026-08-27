//! Decoder microbenchmarks, one `criterion_group` function per mapped type
//! family (`docs/design/architecture.md`, "Testing philosophy"). A regression tripwire for
//! `decode.rs`'s own per-byte CPU cost, not a throughput number for the
//! reader — see `benches/whole_file.rs` for that, and
//! `docs/design/roadmap-P7-scan-performance.md` for the wider
//! measurement campaign this narrowly feeds into.
//!
//! `text`/`varchar`/`char` decode zero-copy (`Utf8View`, no `decode.rs`
//! function at all) and enum's "decode" is an unparsed dictionary-key append
//! (also no `decode.rs` function) — neither has a decoder to benchmark here.
//! `int` has no `decode.rs` function either (`batch.rs` calls `str::parse`
//! directly), but it is still benchmarked, both as a real mapped family and
//! as the cheap baseline the other numeric families (`float`, `numeric`) are
//! worth comparing against.
//!
//! The `nested` group is the exception to one-group-per-family: array,
//! record and their `text_copy` control sit together because the figure they
//! back is a *ratio between them* (`docs/design/measurements.md`, "Nested
//! decode costs what it copies"), not three independent tripwires. Two array
//! lengths, because the deferral this feeds is what *per-element* copying
//! costs and one length cannot separate the per-element slope from the
//! per-value overhead.
//!
//! There are **two** controls, because the two questions have different
//! answers. `text_copy` is `String::from` over the same byte count: a nested
//! value has no borrowed arm at all (`crate::batch::append_nested`), so what
//! the parse can be asked to justify is its cost *on top of* the copy it
//! cannot avoid — which is the variable P7 is choosing over.
//! `text_view_x1024` is the other one: a top-level `Utf8View` field takes
//! `push_utf8view_field`'s `Cow::Borrowed` arm whenever it carried no escapes,
//! writing a 16-byte view into a block it does not own, and that is what a
//! user comparing a text column against an array column actually pays. One
//! row covers every length, because a view write does not depend on the
//! value's size — which is the whole point of it.
//!
//! **Two things about that bench are deliberate and both are traps
//! otherwise.** It reports 1024 appends, not one, and its figure must be
//! divided: the operation is a few nanoseconds, and timing it singly through
//! `iter_batched_ref` gave ~12.6 ns against a harness floor `bool/decode` puts
//! at ~1.1 ns — three quarters apparatus. And what it measures is a *floor* on
//! the borrowed arm, not the borrowed arm: `push_utf8view_field` also scans
//! the chunk deque with `find_map` and calls `block_for`, neither reproduced
//! here. So the ratios taken against it bound the real ones from above.

use std::hint::black_box;

use arrow::array::builder::StringViewBuilder;
use arrow::buffer::Buffer;
use criterion::{Criterion, criterion_group, criterion_main};
use pgdump_query::decode::{
    decimal_unscaled_digits, decode_bool, decode_bytea, decode_date32, decode_f64,
    decode_time64_micros, decode_timestamp_micros, decode_uuid, render_bool, render_bytea,
    render_date32, render_decimal, render_f64, render_time64_micros, render_timestamp_micros,
    render_uuid,
};
use pgdump_query::nested::{decode_array, decode_record, render_array, render_record};

fn bool_family(c: &mut Criterion) {
    let mut g = c.benchmark_group("bool");
    g.bench_function("decode", |b| b.iter(|| decode_bool(black_box("t"))));
    g.bench_function("render", |b| b.iter(|| render_bool(black_box(true))));
    g.finish();
}

fn int_family(c: &mut Criterion) {
    let mut g = c.benchmark_group("int");
    g.bench_function("decode", |b| b.iter(|| black_box("1234567890").parse::<i64>().unwrap()));
    g.bench_function("render", |b| b.iter(|| black_box(1_234_567_890_i64).to_string()));
    g.finish();
}

fn float_family(c: &mut Criterion) {
    let mut g = c.benchmark_group("float");
    g.bench_function("decode", |b| b.iter(|| decode_f64(black_box("1.234567890123456e+15"))));
    g.bench_function("render", |b| b.iter(|| render_f64(black_box(1_234_567_890_123_456.0))));
    g.finish();
}

fn numeric_family(c: &mut Criterion) {
    let text = "123456789012345678.123456";
    let unscaled = decimal_unscaled_digits(text, 6).unwrap();
    let mut g = c.benchmark_group("numeric");
    g.bench_function("decode", |b| {
        b.iter(|| decimal_unscaled_digits(black_box(text), black_box(6)))
    });
    g.bench_function("render", |b| b.iter(|| render_decimal(black_box(&unscaled), black_box(6))));
    g.finish();
}

fn date_family(c: &mut Criterion) {
    let mut g = c.benchmark_group("date");
    g.bench_function("decode", |b| b.iter(|| decode_date32(black_box("2024-06-15"))));
    g.bench_function("render", |b| b.iter(|| render_date32(black_box(19_889))));
    g.finish();
}

fn time_family(c: &mut Criterion) {
    let mut g = c.benchmark_group("time");
    g.bench_function("decode", |b| b.iter(|| decode_time64_micros(black_box("13:45:30.123456"))));
    g.bench_function("render", |b| b.iter(|| render_time64_micros(black_box(49_530_123_456_i64))));
    g.finish();
}

fn timestamp_family(c: &mut Criterion) {
    let mut g = c.benchmark_group("timestamp");
    g.bench_function("decode", |b| {
        b.iter(|| decode_timestamp_micros(black_box("2024-06-15 13:45:30.123456"), false))
    });
    g.bench_function("render", |b| {
        b.iter(|| render_timestamp_micros(black_box(1_718_459_130_123_456_i64), false))
    });
    g.finish();
}

fn timestamptz_family(c: &mut Criterion) {
    let mut g = c.benchmark_group("timestamptz");
    g.bench_function("decode", |b| {
        b.iter(|| decode_timestamp_micros(black_box("2024-06-15 13:45:30.123456+00"), true))
    });
    g.bench_function("render", |b| {
        b.iter(|| render_timestamp_micros(black_box(1_718_459_130_123_456_i64), true))
    });
    g.finish();
}

fn uuid_family(c: &mut Criterion) {
    let text = "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11";
    let bytes = decode_uuid(text).unwrap();
    let mut g = c.benchmark_group("uuid");
    g.bench_function("decode", |b| b.iter(|| decode_uuid(black_box(text))));
    g.bench_function("render", |b| b.iter(|| render_uuid(black_box(&bytes))));
    g.finish();
}

fn bytea_family(c: &mut Criterion) {
    let text = format!("\\x{}", "deadbeef".repeat(32));
    let bytes = decode_bytea(&text).unwrap();
    let mut g = c.benchmark_group("bytea");
    g.bench_function("decode", |b| b.iter(|| decode_bytea(black_box(&text))));
    g.bench_function("render", |b| b.iter(|| render_bytea(black_box(&bytes))));
    g.finish();
}

/// One `integer[]` literal of `n` elements, shaped like what
/// `scripts/generate_perf_data.py --arrays` writes.
fn int_array_literal(n: usize) -> String {
    let mut out = String::from("{");
    for i in 0..n {
        if i > 0 {
            out.push(',');
        }
        // Wide enough to be a realistic int4, and constant-width so the two
        // lengths differ only in element count.
        out.push_str("-1234567890");
    }
    out.push('}');
    out
}

fn nested_family(c: &mut Criterion) {
    let short = int_array_literal(4);
    let long = int_array_literal(50);
    let record = "(-1234567890,\"lorem ipsum dolor sit amet\")";
    let short_text = "x".repeat(short.len());
    let long_text = "x".repeat(long.len());

    let short_lit = decode_array(&short).unwrap();
    let long_lit = decode_array(&long).unwrap();
    let record_lit = decode_record(record).unwrap();

    let mut g = c.benchmark_group("nested");
    g.bench_function("array_4/decode", |b| b.iter(|| decode_array(black_box(&short))));
    g.bench_function("array_4/render", |b| b.iter(|| render_array(black_box(&short_lit))));
    g.bench_function("array_50/decode", |b| b.iter(|| decode_array(black_box(&long))));
    g.bench_function("array_50/render", |b| b.iter(|| render_array(black_box(&long_lit))));
    g.bench_function("record_2/decode", |b| b.iter(|| decode_record(black_box(record))));
    g.bench_function("record_2/render", |b| b.iter(|| render_record(black_box(&record_lit))));
    // Control 1: the same byte counts, copied and nothing more.
    g.bench_function("text_copy/array_4_len", |b| b.iter(|| String::from(black_box(&short_text))));
    g.bench_function("text_copy/array_50_len", |b| b.iter(|| String::from(black_box(&long_text))));
    g.bench_function("text_copy/record_2_len", |b| {
        let t = "x".repeat(record.len());
        b.iter(|| String::from(black_box(&t)))
    });
    // Control 2: the borrowed arm — one view write into a block the builder
    // does not own, which is what an unescaped text field of *any* length
    // costs. A fresh builder per iteration (its construction is not timed)
    // keeps the view vector from growing without bound across the run.
    // Reports 1024 appends; divide. See the module header for why it is
    // batched and why the result is a floor rather than the borrowed arm.
    const VIEW_BATCH: usize = 1024;
    let buf = Buffer::from(long_text.as_bytes());
    let view_len = u32::try_from(short_text.len()).unwrap();
    g.bench_function("text_view_x1024", |b| {
        b.iter(|| {
            let mut builder = StringViewBuilder::with_capacity(VIEW_BATCH);
            let block = builder.append_block(buf.clone());
            for _ in 0..VIEW_BATCH {
                // SAFETY: `long_text` is ASCII, so 0..view_len is a valid
                // UTF-8 range inside the block appended above.
                unsafe { builder.append_view_unchecked(black_box(block), 0, view_len) }
            }
            builder
        })
    });
    g.finish();
}

criterion_group!(
    decoders,
    bool_family,
    int_family,
    float_family,
    numeric_family,
    date_family,
    time_family,
    timestamp_family,
    timestamptz_family,
    uuid_family,
    bytea_family,
    nested_family,
);
criterion_main!(decoders);
