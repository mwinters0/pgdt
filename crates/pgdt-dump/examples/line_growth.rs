//! A probe, never a figure (`docs/design/measurements.md`, "What an instrument
//! can see"): what a held line's growth by remapping costs against the same
//! line written into a mapping sized for it, the choice a read knowing its
//! longest line could make (`docs/design/decisions.md`, "D139").
//!
//! ```sh
//! line_growth <line bytes>… [--reps N] [--chunk BYTES]
//! ```
//!
//! For each length, `N` repetitions (default 5) of two legs, alternated so
//! neither always runs first: **`remap`**, a [`LineMapping`] begun at one
//! chunk and extended a chunk at a time, as a carry grows a line past its
//! chunk; and **`sized`**, the same chunks written into a mapping of the
//! line's length. Each prints the time to write the line and the time to drop
//! it, how many times the line moved while it was written and whether it
//! ended on a step's boundary ([`LineMapping::step`]), the bytes mapped, and
//! how much of the process's anonymous memory transparent huge pages back
//! before the drop (`/proc/self/smaps_rollup`'s `AnonHugePages`); the chunk is
//! `pgdt_io`'s default read unless stated.
//!
//! `runs/p33.12-line-steps/probe.sh` runs it; `runs/p33.10-line-mapping/growth-thp.out`
//! is the reading of exact growth that D139's step answers. **Compact memory before a leg**
//! (`vm.compact_memory`) and record `/proc/buddyinfo` beside it: uncompacted,
//! the host's free 2 MiB blocks decide both legs' backing, so the reading
//! prices the host (`growth.out` against `growth-compacted.out` there).

use std::time::Instant;

use pgdt_dump::internal::LineMapping;
use pgdt_io::SCAN_CHUNK_DEFAULT_SIZE_BYTES;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut lengths, mut reps, mut chunk) = (Vec::new(), 5usize, SCAN_CHUNK_DEFAULT_SIZE_BYTES);
    while let Some(arg) = args.next() {
        let mut value = || args.next().and_then(|v| v.parse().ok()).expect("a number after a flag");
        match arg.as_str() {
            "--reps" => reps = value(),
            "--chunk" => chunk = value(),
            length => lengths.push(length.parse::<usize>().expect("a line length in bytes")),
        }
    }
    assert!(!lengths.is_empty(), "usage: line_growth <line bytes>… [--reps N] [--chunk BYTES]");
    let bytes: Vec<u8> = (0..chunk).map(|i| b'a' + (i % 26) as u8).collect();
    for &len in &lengths {
        for rep in 0..reps {
            let legs = if rep % 2 == 0 { ["remap", "sized"] } else { ["sized", "remap"] };
            for leg in legs {
                let capacity = if leg == "sized" { len } else { chunk.min(len) };
                let started = Instant::now();
                let mut line = LineMapping::reserving(capacity, &bytes[..chunk.min(len)]);
                let mut moves = 0usize;
                while line.as_ref().len() < len {
                    let more = chunk.min(len - line.as_ref().len());
                    let at = line.as_ref().as_ptr();
                    line.extend(&bytes[..more]);
                    moves += usize::from(line.as_ref().as_ptr() != at);
                }
                let written = started.elapsed();
                let step = LineMapping::step();
                assert_eq!(line.mapped(), len.next_multiple_of(step));
                let aligned = (line.as_ref().as_ptr() as usize).is_multiple_of(step);
                let mapped = line.mapped();
                let huge = anon_huge_kib();
                let started = Instant::now();
                drop(line);
                let dropped = started.elapsed();
                println!(
                    "leg={leg} len={len} chunk={chunk} rep={rep} write_ms={:.3} drop_ms={:.3} \
                     moves={moves} aligned={aligned} mapped={mapped} anon_huge_kib={huge}",
                    written.as_secs_f64() * 1e3,
                    dropped.as_secs_f64() * 1e3,
                );
            }
        }
    }
}

/// The process's anonymous memory transparent huge pages back, in KiB.
fn anon_huge_kib() -> u64 {
    let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    rollup
        .lines()
        .find_map(|line| line.strip_prefix("AnonHugePages:"))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}
