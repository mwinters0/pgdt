//! One warm-cache whole-file run (`docs/design/decisions.md`, "D73"): an end-to-end regression tripwire for typed-decode CPU cost,
//! complementing `benches/decoders.rs`'s per-family microbenchmarks. It is
//! deliberately narrow — `docs/design/measurements.md` is where the dial
//! turns up to koji-sized (~100 GB) runs and CPU%/bytes-per-second tracking;
//! this only has to catch a regression on data that fits page cache.
//!
//! `SchemaMode::Typed` (`QueryOptions::default()`) is used deliberately: it's
//! the mode that walks every field through `decode.rs`, which
//! `SchemaMode::Strings` (the pure zero-copy path) does not, so
//! only `Typed` can see a decode regression at all.
//!
//! The dataset is generated on first run via `scripts/generate_perf_data.py`
//! (see its module docs) into the gitignored `runs/` directory and reused on
//! later runs — never committed, and shape-only, not pg_dump-authoritative,
//! per that script's own docs.
//!
//! **The input is regenerated when the generator changes**, not only when it
//! is missing. A stamp beside it holds a hash of the generator's source and
//! of `PERF_DATA_SIZE_MB`; a mismatch regenerates. Without that, a checkout
//! holding an input from before a generator change would benchmark the old
//! bytes indefinitely and silently — and the machines holding one are exactly
//! the ones that would compare the new number against an old one.

use std::hint::black_box;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Command;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use pgdump_query::cache::CacheMode;
use pgdump_query::{LocalFileSource, QueryOptions, ScanOptions, read_table};

/// Fits page cache on any development machine
/// (`scripts/generate_perf_data.py --help`).
const PERF_DATA_SIZE_MB: u32 = 256;

/// What the input's bytes depend on: the generator's own source and the size
/// asked of it. Hashed rather than compared, so the stamp stays one line.
/// `DefaultHasher` is not cryptographic and does not need to be — the only
/// failure it can produce is a needless regeneration after a Rust upgrade
/// changes its keys, which costs one run of a 25-second script.
fn input_stamp(generator: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let src = std::fs::read(generator).expect("the generator script is readable");
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    PERF_DATA_SIZE_MB.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn perf_data_path() -> PathBuf {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let scripts = repo_root.join("scripts");
    let path = repo_root.join("runs").join("perf-whole-file.sql");
    let stamp_path = path.with_extension("stamp");

    let want = input_stamp(&scripts.join("generate_perf_data.py"));
    let have = std::fs::read_to_string(&stamp_path).unwrap_or_default();
    if !path.exists() || have.trim() != want {
        // Remove the stamp first: a generator run that dies partway leaves a
        // truncated input, and a stamp written beside it would certify it.
        let _ = std::fs::remove_file(&stamp_path);
        let status = Command::new("uv")
            .args(["run", "generate_perf_data.py"])
            .arg(&path)
            .args(["--size-mb", &PERF_DATA_SIZE_MB.to_string()])
            .current_dir(&scripts)
            .status()
            .expect("failed to run scripts/generate_perf_data.py -- is `uv` installed?");
        assert!(status.success(), "generate_perf_data.py failed");
        std::fs::write(&stamp_path, &want).expect("the stamp is writable");
    }
    path
}

async fn scan_once(path: &Path) -> u64 {
    let source = LocalFileSource::open(path).unwrap();
    let mut rows = 0u64;
    read_table(
        &source,
        "public.perf",
        &ScanOptions::default(),
        &QueryOptions::default(),
        CacheMode::Disabled,
        |batch| {
            rows += batch.num_rows() as u64;
            ControlFlow::Continue(())
        },
    )
    .await
    .unwrap();
    rows
}

fn warm_cache_whole_file(c: &mut Criterion) {
    let path = perf_data_path();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Warm the page cache before measuring: a figure about parsing CPU is
    // taken warm, never against a device (`docs/design/measurements.md`,
    // "Scan throughput by input shape").
    let rows = rt.block_on(scan_once(&path));
    assert!(rows > 0, "perf dataset produced no rows");

    let mut g = c.benchmark_group("whole_file");
    g.sample_size(10);
    g.throughput(Throughput::Bytes(std::fs::metadata(&path).unwrap().len()));
    g.bench_function("warm_cache_typed_scan", |b| {
        b.to_async(&rt).iter(|| async { black_box(scan_once(black_box(&path)).await) });
    });
    g.finish();
}

criterion_group!(whole_file, warm_cache_whole_file);
criterion_main!(whole_file);
