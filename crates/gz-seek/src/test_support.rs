//! What the tests share: payloads, files written by encoders not ours, and a
//! driver feeding a forward pass the way a caller does.

use std::io::Write;
use std::num::NonZeroU64;

use crate::{Forward, Index, Result};

pub(crate) fn spacing(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n).expect("a nonzero spacing")
}

/// A deterministic payload with the shape each deflate block kind codes:
/// `COPY`-like lines, long repeats, phrases recurring just inside the 32 KiB
/// window, and incompressible bursts — which kinds a stream holds being its
/// encoder's choice, level 1 under zlib-rs writing static blocks alone.
pub(crate) fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut out = Vec::with_capacity(len + 64 * 1024);
    let mut row = 0u64;
    while out.len() < len {
        match next() % 8 {
            0 => {
                let n = 1024 + (next() % 8192) as usize;
                out.extend((0..n).map(|_| next() as u8));
            }
            1 => {
                let byte = next() as u8;
                out.extend(std::iter::repeat_n(byte, 100 + (next() % 4000) as usize));
            }
            2 if out.len() > 40_000 => {
                let back = 30_000 + (next() % 2_700) as usize;
                let from = out.len() - back;
                let n = 200 + (next() % 600) as usize;
                let phrase = out[from..from + n].to_vec();
                out.extend(phrase);
            }
            _ => {
                for _ in 0..(1 + next() % 200) {
                    row += 1;
                    let line = format!("{row}\tvalue {}\t{}\n", next() % 10_007, next() % 97);
                    out.extend(line.as_bytes());
                }
            }
        }
    }
    out.truncate(len);
    out
}

/// `bytes` as one gzip member, written by `flate2` at `level` over zlib-rs
/// whatever a build selects (the workspace's `Cargo.toml`). Still no test assumes where
/// its blocks end, which is the encoder's to choose.
pub(crate) fn gzip(bytes: &[u8], level: u32) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(level));
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// Each of `parts` as a member of its own, concatenated.
pub(crate) fn gzip_members(parts: &[&[u8]], level: u32) -> Vec<u8> {
    parts.iter().flat_map(|part| gzip(part, level)).collect()
}

/// `bytes` as BGZF, written by `noodles-bgzf`: a member to each block of at
/// most 64 KiB, and the empty member that ends the file.
pub(crate) fn bgzf(bytes: &[u8]) -> Vec<u8> {
    let mut writer = noodles_bgzf::io::Writer::new(Vec::new());
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap()
}

/// What a forward pass handed out.
pub(crate) struct Pass {
    pub(crate) index: Index,
    pub(crate) plaintext: Vec<u8>,
    /// Each checkpoint's window, by checkpoint.
    pub(crate) windows: Vec<Vec<u8>>,
}

/// A forward pass over all of `file`, fed at most `input` bytes and given at
/// most `out` bytes of room a step.
pub(crate) fn forward(file: &[u8], spacing: u64, input: usize, out: usize) -> Result<Pass> {
    drive(Forward::new(self::spacing(spacing))?, file, input, out, Vec::new(), None)
}

/// Drive `pass` to the end of `file`, `windows` holding those of the
/// checkpoints it already has, stopping early — with the pass's partial
/// index — once `stop_after` more checkpoints have been placed.
pub(crate) fn drive(
    mut pass: Forward,
    file: &[u8],
    input: usize,
    out: usize,
    mut windows: Vec<Vec<u8>>,
    stop_after: Option<usize>,
) -> Result<Pass> {
    let mut buf = vec![0u8; out];
    let mut plaintext = Vec::new();
    let mut placed = 0;
    loop {
        let at = pass.compressed_position() as usize;
        let supply = &file[at.min(file.len())..(at + input).min(file.len())];
        let step = pass.read(supply, &mut buf)?;
        plaintext.extend_from_slice(&buf[..step.progress.produced]);
        if step.checkpoint.is_some() {
            windows.push(pass.window().to_vec());
            placed += 1;
            if stop_after == Some(placed) {
                return Ok(Pass { index: pass.index(), plaintext, windows });
            }
        }
        if step.progress == Default::default() && step.checkpoint.is_none() {
            break;
        }
    }
    Ok(Pass { index: pass.finish()?, plaintext, windows })
}

/// The bytes of `file` in `range`.
pub(crate) fn slice(file: &[u8], range: std::ops::Range<u64>) -> &[u8] {
    &file[range.start as usize..range.end as usize]
}
