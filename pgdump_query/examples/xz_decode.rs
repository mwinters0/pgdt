//! The `xz-decode-scaling` figure's instrument: decode one `.xz` file's whole
//! plaintext at a declared worker count, and print what was decoded.
//!
//! It exists because the phase's thesis rests on a **probe** — "one core
//! decodes ~446 MB/s of plaintext, four concurrent decodes reach ~1.48 GB/s" —
//! taken by hand, with no harness, no `drop_caches` discipline and no
//! repetition (`docs/design/architecture.md`, "The compressed source"). A
//! number that decides how many workers a scan is worth has to be a figure,
//! and a figure needs a command a later session can run again.
//!
//! ```sh
//! cargo build --release -p pgdump_query --example xz_decode
//! ./target/release/examples/xz_decode --source control_xz.xz --workers 8
//! ```
//!
//! # Why an example and not a `pgdq` flag
//!
//! Nothing in the library decodes concurrently yet — `io::XzSource` is one
//! `xz_seek::Reader` behind a mutex, and replacing that is a later slice. This
//! binary reaches past the library to the decoder's own bulk entry point, so
//! the figure is a property of the decoder rather than of whatever the library
//! currently does with it, and taking it costs no library change at all. It is
//! a separate target, so it is in neither the shipped CLI nor the library's
//! own compilation.
//!
//! # What it does *not* time, and why the timer is still outside it
//!
//! `scripts/measure.py` times this the way it times every other figure: with
//! the container's own `bash` builtin around the whole process. So the reading
//! includes process startup, the seek-table walk and the teardown, and this
//! binary does not gate a counter around a region of its own.
//!
//! That is affordable only because every one of those is bounded and small
//! **for the inputs this figure declares**: both are single-file `.xz` inputs
//! whose walk is one read per stream, against a decode of ~3 GiB of plaintext.
//! It would not be affordable against the koji download's 31,150-stream walk,
//! which is 85 s on its own — which is one of the reasons that file is not an
//! input here and a stream-boundary prefix of it is.
//!
//! # The two things it refuses
//!
//! **A clamped worker count.** [`xz_seek::RangePlan`] answers how many workers
//! a range actually admits, which is the count asked for clamped by the byte
//! budget and by how many blocks there are to decode. A run whose plan admits
//! fewer workers than were asked for is not a reading of that worker count, and
//! publishing one would put the same number in two rows of a scaling table and
//! call the curve flat. So it exits non-zero and says both numbers.
//!
//! **A short decode.** The range is the whole plaintext the seek table
//! reports, and the byte count delivered is compared against it. A decoder
//! that stopped early would otherwise read as a fast one.
//!
//! # What consumes the bytes
//!
//! [`std::hint::black_box`] over one byte of each filled buffer, which is
//! `O(1)` per `read` call rather than per byte. A fold over the bytes — a sum,
//! a checksum — would add a second measurement of the decoded volume to the
//! first, and this figure's whole content is that volume divided by a time.

use std::fs::File;
use std::hint::black_box;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use xz_seek::{Bulk, Reader};

/// What one `read` call is asked for. Large enough that the per-call overhead
/// is nothing against a 24 MiB block, small enough that the buffer itself is
/// not part of what the figure measures resident.
const READ_BUFFER: usize = 1 << 20;

struct Args {
    source: PathBuf,
    workers: usize,
    /// The byte budget handed to [`Bulk`]. Defaults to "bind on nothing":
    /// this figure's bound is the worker count, and a budget that silently
    /// clamped it would turn a scaling table into a memory table.
    budget: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut source: Option<PathBuf> = None;
    let mut workers: Option<usize> = None;
    let mut budget = u64::MAX;
    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let mut value = || argv.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--source" => source = Some(PathBuf::from(value()?)),
            "--workers" => {
                let raw = value()?;
                workers = Some(raw.parse().map_err(|_| format!("--workers {raw}: not a count"))?);
            }
            "--budget" => {
                let raw = value()?;
                budget = raw.parse().map_err(|_| format!("--budget {raw}: not a byte count"))?;
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(Args {
        source: source.ok_or("--source is required")?,
        workers: workers.ok_or("--workers is required")?,
        budget,
    })
}

fn run(args: &Args) -> Result<(), String> {
    let file = File::open(&args.source).map_err(|e| format!("{}: {e}", args.source.display()))?;
    let reader = Reader::new(Arc::new(file)).map_err(|e| format!("opening the seek table: {e}"))?;
    let table = reader.index();
    let total = table.uncompressed_size();
    let plan = reader.plan_range(0..total, Bulk::new(args.workers, args.budget));

    // Printed before the refusal below, so a clamped run still says what the
    // file admitted rather than only that it was refused.
    println!("blocks={}", plan.blocks());
    println!("streams={}", table.stream_count());
    println!("workers={}", plan.workers());
    println!("requested={}", plan.requested_workers());
    println!("footprint={}", plan.footprint());
    println!("plaintext={total}");

    if plan.workers() != args.workers {
        return Err(format!(
            "asked for {} workers and this range admits {}: {} blocks, {} bytes of footprint \
             against a {} byte budget — not a reading of {} workers",
            args.workers,
            plan.workers(),
            plan.blocks(),
            plan.footprint(),
            plan.budget(),
            args.workers,
        ));
    }

    let mut read = reader
        .read_range(0..total, Bulk::new(args.workers, args.budget))
        .map_err(|e| format!("opening the range: {e}"))?;
    let mut buf = vec![0u8; READ_BUFFER];
    let mut done = 0u64;
    loop {
        let n = read.read(&mut buf).map_err(|e| format!("at {done}: {e}"))?;
        if n == 0 {
            break;
        }
        black_box(buf[0]);
        done += n as u64;
    }
    if done != total {
        return Err(format!("delivered {done} bytes of {total}: a short decode is not a reading"));
    }
    println!("delivered={done}");
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(why) => {
            eprintln!("xz_decode: {why}");
            eprintln!("usage: xz_decode --source <file.xz> --workers <n> [--budget <bytes>]");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("xz_decode: {why}");
            ExitCode::from(1)
        }
    }
}
