//! The instrument `gz-seek`'s figures time: one gzip file read through this
//! crate on one thread, as a first pass or as every unit resumed alone, and
//! what was read printed.
//!
//! The `gzseek-backend` figure times it on each engine the crate builds over
//! (`Cargo.toml`'s features), each a build of its own, which is what chose the
//! one that ships (`docs/design/decisions.md`, "D112"); `gzseek-spacing` times
//! the shipped engine's build at each candidate spacing:
//!
//! ```sh
//! cargo build --release -p gz-seek --example gz_read --no-default-features --features zlib-ng
//! ./target/release/examples/gz_read --source control_gz.gz --spacing 33554432 --shape units
//! ```
//!
//! # The two shapes
//!
//! - **`forward`**: the first pass — [`gz_seek::Forward`] over the whole file,
//!   a checkpoint at each multiple of `--spacing`, every member's CRC-32 and
//!   `ISIZE` compared and each unit's CRC-32 recorded, and each checkpoint's
//!   window copied out, as a caller writing them to its cache does.
//! - **`units`**: that pass, then every unit of the index it built decoded
//!   alone from its checkpoint and window, its compressed range read
//!   positionally, each a fresh [`gz_seek::UnitTask::decode`] comparing the
//!   unit's CRC-32. What a resumed unit decode costs is this shape's time
//!   less `forward`'s, both taken in one sitting.
//! - **`starts`**: that pass, then every unit begun alone from its checkpoint
//!   and window and fed until it produces its first plaintext byte, its first
//!   compressed bytes read positionally. What a resume costs before it
//!   decodes — a fresh inflate state, the window set, the bits primed, a
//!   read — is this shape's time less `forward`'s, over the unit count; the
//!   first block's header, which a whole unit decode reads anyway, is inside
//!   it, so it bounds that cost from above.
//!
//! # What it prints
//!
//! `engine=` — the engine the build links, as its `zlibVersion` names it,
//! which the harness holds to the leg it built; `delivered=` — the plaintext
//! the shape's last stage produced, the whole file's in both, which the
//! harness holds to the file it is a compression of, so a short read cannot
//! read as a fast one — the first pass's in `starts`, whose last stage
//! produces a byte a unit; `units=`, the index's unit count; `windows=`, the
//! bytes of every checkpoint's window, which is the window section a cache
//! holds for the index; and in `starts`, `started=`, the units begun and
//! brought to their first plaintext, which the harness holds to `units=`.
//! `--engine`
//! alone prints the first and reads nothing, which is how the harness asks a
//! build what it is before timing it.
//!
//! # What it does not time
//!
//! Nothing: `scripts/measure.py` times the whole process with the
//! container's own `bash`, as it times every figure, startup and the file's
//! reads included. What consumes the plaintext is
//! [`std::hint::black_box`] over one byte of each filled buffer, `O(1)` a
//! call, as in `pgdt-io`'s `xz_decode` example.

use std::fs::File;
use std::hint::black_box;
use std::io::Read;
use std::num::NonZeroU64;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::process::ExitCode;

use gz_seek::{Forward, Index};

/// Compressed bytes read from the file at a time by the first pass.
const READ_BUFFER: usize = 1 << 20;

/// Plaintext bytes asked of the first pass at a time.
const OUT_BUFFER: usize = 1 << 20;

/// Compressed bytes `starts` reads from a unit's start at a time: more than a
/// deflate block's header and its first symbol, so one read almost always
/// suffices.
const START_READ: usize = 4 << 10;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Forward,
    Units,
    Starts,
}

struct Args {
    source: PathBuf,
    spacing: NonZeroU64,
    shape: Shape,
}

fn parse_args() -> Result<Args, String> {
    let (mut source, mut spacing, mut shape) = (None, None, None);
    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let value = argv.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--source" => source = Some(PathBuf::from(value)),
            "--spacing" => {
                let n = value.parse::<u64>().map_err(|e| format!("--spacing {value}: {e}"))?;
                spacing = Some(NonZeroU64::new(n).ok_or("--spacing must be positive")?);
            }
            "--shape" => {
                shape = Some(match value.as_str() {
                    "forward" => Shape::Forward,
                    "units" => Shape::Units,
                    "starts" => Shape::Starts,
                    other => {
                        return Err(format!("--shape {other}: one of forward, units, starts"));
                    }
                })
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(Args {
        source: source.ok_or("--source is required")?,
        spacing: spacing.ok_or("--spacing is required")?,
        shape: shape.ok_or("--shape is required")?,
    })
}

/// The first pass over `file`: its index, each unit's window, and the
/// plaintext it produced.
fn first_pass(file: &mut File, spacing: NonZeroU64) -> Result<(Index, Vec<Vec<u8>>, u64), String> {
    let mut pass = Forward::new(spacing).map_err(|e| e.to_string())?;
    let mut input = vec![0u8; READ_BUFFER];
    let mut out = vec![0u8; OUT_BUFFER];
    let (mut held, mut taken, mut eof) = (0usize, 0usize, false);
    let mut windows = Vec::new();
    let mut delivered = 0u64;
    loop {
        if taken == held && !eof {
            held = file.read(&mut input).map_err(|e| e.to_string())?;
            taken = 0;
            eof = held == 0;
        }
        let step = pass.read(&input[taken..held], &mut out).map_err(|e| e.to_string())?;
        taken += step.progress.consumed;
        if step.progress.produced > 0 {
            black_box(out[step.progress.produced - 1]);
            delivered += step.progress.produced as u64;
        }
        if step.checkpoint.is_some() {
            windows.push(pass.window().to_vec());
        }
        if step.progress == Default::default() && step.checkpoint.is_none() {
            if eof {
                break;
            }
            if taken < held {
                // The pass took nothing of what it holds and wants more: move
                // the unread tail down and read on behind it.
                input.copy_within(taken..held, 0);
                held -= taken;
                taken = 0;
                let more = file.read(&mut input[held..]).map_err(|e| e.to_string())?;
                held += more;
                eof = more == 0;
            }
        }
    }
    let index = pass.finish().map_err(|e| e.to_string())?;
    Ok((index, windows, delivered))
}

/// Every unit of `index` decoded alone, its compressed range read from
/// `file`: the plaintext they produced.
fn every_unit(file: &File, index: &Index, windows: &[Vec<u8>]) -> Result<u64, String> {
    let tasks: Vec<_> = (0..index.unit_count()).map(|i| index.unit(i).expect("a unit")).collect();
    let len = |r: std::ops::Range<u64>| (r.end - r.start) as usize;
    let widest = tasks.iter().map(|t| len(t.compressed_range())).max().unwrap_or(0);
    let longest = tasks.iter().map(|t| len(t.plaintext_range())).max().unwrap_or(0);
    let (mut compressed, mut out) = (vec![0u8; widest], vec![0u8; longest]);
    let mut delivered = 0u64;
    for task in tasks {
        let range = task.compressed_range();
        let compressed = &mut compressed[..len(range.clone())];
        file.read_exact_at(compressed, range.start).map_err(|e| e.to_string())?;
        let out = &mut out[..len(task.plaintext_range())];
        task.decode(&windows[task.unit()], compressed, out).map_err(|e| e.to_string())?;
        if let Some(last) = out.last() {
            black_box(*last);
        }
        delivered += out.len() as u64;
    }
    Ok(delivered)
}

/// Every unit of `index` begun alone and fed from its start in `file` until
/// it produces its first plaintext byte: how many were.
fn every_start(file: &File, index: &Index, windows: &[Vec<u8>]) -> Result<u64, String> {
    let mut compressed = vec![0u8; START_READ];
    let mut first = [0u8; 1];
    let mut started = 0u64;
    for (unit, window) in windows.iter().enumerate().take(index.unit_count()) {
        let task = index.unit(unit).expect("a unit");
        let range = task.compressed_range();
        let mut read = task.begin(window).map_err(|e| e.to_string())?;
        let mut at = range.start;
        let mut produced = 0;
        while produced == 0 && !read.is_finished() {
            let len = (range.end - at).min(START_READ as u64) as usize;
            let chunk = &mut compressed[..len];
            file.read_exact_at(chunk, at).map_err(|e| e.to_string())?;
            let mut taken = 0;
            while taken < len && produced == 0 && !read.is_finished() {
                let step = read.read(&chunk[taken..], &mut first).map_err(|e| e.to_string())?;
                if step == Default::default() {
                    break;
                }
                taken += step.consumed;
                produced += step.produced;
            }
            if taken == 0 && produced == 0 && !read.is_finished() {
                return Err(format!("unit {unit} took nothing of its bytes at {at}"));
            }
            at += taken as u64;
        }
        black_box(first[0]);
        started += 1;
    }
    Ok(started)
}

fn run(args: &Args) -> Result<(), String> {
    let mut file =
        File::open(&args.source).map_err(|e| format!("{}: {e}", args.source.display()))?;
    let (index, windows, mut delivered) = first_pass(&mut file, args.spacing)?;
    let mut started = None;
    match args.shape {
        Shape::Forward => {}
        Shape::Units => delivered = every_unit(&file, &index, &windows)?,
        Shape::Starts => started = Some(every_start(&file, &index, &windows)?),
    }
    println!("engine={}", gz_seek::engine());
    println!("delivered={delivered}");
    println!("units={}", index.unit_count());
    println!("windows={}", windows.iter().map(Vec::len).sum::<usize>());
    if let Some(started) = started {
        println!("started={started}");
    }
    Ok(())
}

fn main() -> ExitCode {
    if std::env::args().skip(1).eq(["--engine"]) {
        println!("engine={}", gz_seek::engine());
        return ExitCode::SUCCESS;
    }
    let result = parse_args().and_then(|args| run(&args));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("gz_read: {message}");
            ExitCode::FAILURE
        }
    }
}
