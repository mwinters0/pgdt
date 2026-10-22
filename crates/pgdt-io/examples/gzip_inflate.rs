//! The `inflate-throughput` figure's instrument: inflate one single-member
//! gzip file whole, on one thread, through one of two deflate backends, and
//! print what was inflated.
//!
//! What the figure is for is P15's evidence slice
//! (`docs/design/roadmap-P15-gzip.md`, "Slice order"): a single member's first
//! pass decodes serially, so serial inflate set against the plain parse on
//! each measured device is what decides whether a parallel first pass is
//! worth building, and the backends' rates are what `gz-seek`'s throughput
//! floor is read off.
//!
//! ```sh
//! cargo build --release -p pgdt-io --example gzip_inflate
//! ./target/release/examples/gzip_inflate --source control_gz.gz --backend zlib-rs
//! ```
//!
//! # The backends, each driven directly
//!
//! - **`zlib-rs`**: the `zlib-rs` crate's stable `Inflate`.
//! - **`miniz_oxide`**: its streaming `inflate` over raw deflate, the gzip
//!   header, CRC-32 and `ISIZE` checked here, the CRC by `crc32fast` — what
//!   `flate2`'s Rust backend does around the same decoder.
//!
//! Not through `flate2`, which links one backend a build: two legs through it
//! would be two builds of one example under two feature sets, where one
//! binary taking a flag differs between legs by the decoder alone.
//!
//! **No zlib-ng leg**, though the spec names it as the floor's reference:
//! neither binding to it builds where every figure's binary is built, the
//! release image. `libz-ng-sys` builds only under CMake, which the image does
//! not install, and `libz-sys`'s CMake-free build of zlib-ng fails — at
//! 1.1.30 its function table is left incomplete (`WITH_OPTIM` undefined), so
//! `inflateInit2` returns `Z_VERSION_ERROR`, and at 1.1.23 it does not link,
//! defining `X86_AVX512` without compiling `chunkset_avx512.c`.
//!
//! # What a run checks, and why
//!
//! **The whole member, framing included**: `zlib-rs` is opened for gzip
//! (`windowBits` 31) and checks the header, the CRC-32 and `ISIZE` itself;
//! the `miniz_oxide` leg checks the same three here. A first pass
//! has to check its member's CRC-32 (the P15 spec's "Each unit carries its
//! own CRC-32"), so a rate without one would be a rate of something no first
//! pass does.
//!
//! **One member and nothing after it**, which is what `pg_dump -Fp -Z`
//! writes (the spec's "Facts the grilling found"). Trailing bytes after the
//! member are refused rather than read as a second member: this figure's
//! input is the single-member shape, and a run that silently read another
//! would be a reading of some other file.
//!
//! # What it does not time
//!
//! Nothing: `scripts/measure.py` times the whole process with the
//! container's own `bash`, as it times every figure, so a reading includes
//! startup and the file's reads. Both are small against a few GiB of
//! plaintext; the cold regimes' reads are what those regimes measure.
//!
//! # What consumes the bytes
//!
//! [`std::hint::black_box`] over one byte of each filled buffer, as in
//! `xz_decode.rs`: `O(1)` a call rather than a byte, so the figure's content
//! stays the volume inflated divided by a time.

use std::fs::File;
use std::hint::black_box;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

/// Compressed bytes read from the file at a time.
const READ_BUFFER: usize = 1 << 20;

/// Plaintext bytes asked of the decoder at a time.
const OUT_BUFFER: usize = 1 << 20;

/// `windowBits` for a gzip-wrapped stream with a 32 KiB window: 15, plus 16
/// for the gzip framing.
const GZIP_WINDOW_BITS: u8 = 15 + 16;

#[derive(Clone, Copy)]
enum Backend {
    ZlibRs,
    MinizOxide,
}

impl Backend {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "zlib-rs" => Ok(Self::ZlibRs),
            "miniz-oxide" => Ok(Self::MinizOxide),
            other => Err(format!("--backend {other}: one of zlib-rs, miniz-oxide")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::ZlibRs => "zlib-rs",
            Self::MinizOxide => "miniz-oxide",
        }
    }
}

struct Args {
    source: PathBuf,
    backend: Backend,
}

fn parse_args() -> Result<Args, String> {
    let mut source: Option<PathBuf> = None;
    let mut backend: Option<Backend> = None;
    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let mut value = || argv.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--source" => source = Some(PathBuf::from(value()?)),
            "--backend" => backend = Some(Backend::parse(&value()?)?),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(Args {
        source: source.ok_or("--source is required")?,
        backend: backend.ok_or("--backend is required")?,
    })
}

/// The compressed side: the file, read a buffer at a time, with what the
/// decoder has not consumed of the current buffer.
struct Input {
    file: File,
    buf: Vec<u8>,
    start: usize,
    end: usize,
}

impl Input {
    fn new(file: File) -> Self {
        Self { file, buf: vec![0u8; READ_BUFFER], start: 0, end: 0 }
    }

    /// The unconsumed bytes, refilled from the file once they run out; empty
    /// only at the file's end.
    fn pending(&mut self) -> Result<&[u8], String> {
        if self.start == self.end {
            self.start = 0;
            self.end = self.file.read(&mut self.buf).map_err(|e| format!("reading: {e}"))?;
        }
        Ok(&self.buf[self.start..self.end])
    }

    fn consume(&mut self, n: usize) {
        self.start += n;
    }

    /// Refuses anything after the member, which is the shape this figure is
    /// taken on.
    fn expect_end(&mut self) -> Result<(), String> {
        let left = self.pending()?.len();
        if left == 0 {
            Ok(())
        } else {
            Err(format!(
                "{left} or more bytes follow the first member: this instrument reads the \
                 single-member shape `pg_dump -Fp -Z` writes, and nothing else"
            ))
        }
    }
}

fn run(args: &Args) -> Result<(), String> {
    let file = File::open(&args.source).map_err(|e| format!("{}: {e}", args.source.display()))?;
    let compressed = file.metadata().map_err(|e| format!("{e}"))?.len();
    let mut input = Input::new(file);
    let mut out = vec![0u8; OUT_BUFFER];
    let delivered = match args.backend {
        Backend::ZlibRs => zlib_rs(&mut input, &mut out)?,
        Backend::MinizOxide => miniz_oxide(&mut input, &mut out)?,
    };
    input.expect_end()?;
    println!("backend={}", args.backend.name());
    println!("compressed={compressed}");
    println!("delivered={delivered}");
    Ok(())
}

/// Why a decoder that made no progress stopped: the file ended inside the
/// member, or the decoder refused what it was offered.
fn stalled(offered: usize, total_out: u64) -> String {
    if offered == 0 {
        format!("the file ends inside the member, after {total_out} plaintext bytes")
    } else {
        format!("inflate made no progress over {offered} bytes, after {total_out} plaintext bytes")
    }
}

// -- zlib-rs ----------------------------------------------------------------

fn zlib_rs(input: &mut Input, out: &mut [u8]) -> Result<u64, String> {
    use zlib_rs::{Inflate, InflateFlush, Status};

    let mut inflater = Inflate::new(true, GZIP_WINDOW_BITS);
    loop {
        let pending = input.pending()?;
        let offered = pending.len();
        let before_in = inflater.total_in();
        let before_out = inflater.total_out();
        let status = inflater
            .decompress(pending, out, InflateFlush::NoFlush)
            .map_err(|e| format!("inflate: {}, after {before_out} plaintext bytes", e.as_str()))?;
        let read = (inflater.total_in() - before_in) as usize;
        let wrote = (inflater.total_out() - before_out) as usize;
        input.consume(read);
        if wrote > 0 {
            black_box(out[0]);
        }
        if status == Status::StreamEnd {
            return Ok(inflater.total_out());
        }
        if read == 0 && wrote == 0 {
            return Err(stalled(offered, inflater.total_out()));
        }
    }
}

// -- miniz_oxide ------------------------------------------------------------

/// The gzip header's fixed part: magic, method, flags, `MTIME`, `XFL`, OS.
const HEADER: usize = 10;
const FHCRC: u8 = 1 << 1;
const FEXTRA: u8 = 1 << 2;
const FNAME: u8 = 1 << 3;
const FCOMMENT: u8 = 1 << 4;

/// Reads exactly `buf.len()` bytes off the input, across refills.
fn take(input: &mut Input, buf: &mut [u8]) -> Result<(), String> {
    let mut got = 0;
    while got < buf.len() {
        let pending = input.pending()?;
        if pending.is_empty() {
            return Err("the file ends inside the gzip framing".to_string());
        }
        let n = pending.len().min(buf.len() - got);
        buf[got..got + n].copy_from_slice(&pending[..n]);
        input.consume(n);
        got += n;
    }
    Ok(())
}

/// Skips a NUL-terminated field.
fn skip_cstr(input: &mut Input) -> Result<(), String> {
    let mut byte = [0u8];
    loop {
        take(input, &mut byte)?;
        if byte[0] == 0 {
            return Ok(());
        }
    }
}

/// RFC 1952's member header, every optional field skipped.
fn gzip_header(input: &mut Input) -> Result<(), String> {
    let mut fixed = [0u8; HEADER];
    take(input, &mut fixed)?;
    if fixed[..3] != [0x1f, 0x8b, 8] {
        return Err("not a deflate-compressed gzip member".to_string());
    }
    let flags = fixed[3];
    if flags & FEXTRA != 0 {
        let mut xlen = [0u8; 2];
        take(input, &mut xlen)?;
        let mut extra = vec![0u8; u16::from_le_bytes(xlen) as usize];
        take(input, &mut extra)?;
    }
    if flags & FNAME != 0 {
        skip_cstr(input)?;
    }
    if flags & FCOMMENT != 0 {
        skip_cstr(input)?;
    }
    if flags & FHCRC != 0 {
        let mut crc16 = [0u8; 2];
        take(input, &mut crc16)?;
    }
    Ok(())
}

fn miniz_oxide(input: &mut Input, out: &mut [u8]) -> Result<u64, String> {
    use miniz_oxide::inflate::stream::{InflateState, inflate};
    use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

    gzip_header(input)?;
    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut crc = crc32fast::Hasher::new();
    let mut total: u64 = 0;
    loop {
        let pending = input.pending()?;
        let offered = pending.len();
        let result = inflate(&mut state, pending, out, MZFlush::None);
        let (read, wrote) = (result.bytes_consumed, result.bytes_written);
        input.consume(read);
        if wrote > 0 {
            crc.update(&out[..wrote]);
            black_box(out[0]);
        }
        total += wrote as u64;
        match result.status {
            // Returned only once the decoder's window holds nothing unwritten.
            Ok(MZStatus::StreamEnd) => break,
            // Asked with nothing to give, at the file's end.
            Err(MZError::Buf) if offered == 0 => return Err(stalled(offered, total)),
            Err(e) => return Err(format!("inflate: {e:?}, after {total} plaintext bytes")),
            Ok(_) if read == 0 && wrote == 0 => return Err(stalled(offered, total)),
            Ok(_) => {}
        }
    }
    let mut trailer = [0u8; 8];
    take(input, &mut trailer)?;
    let stated_crc = u32::from_le_bytes(trailer[..4].try_into().expect("four bytes"));
    let stated_size = u32::from_le_bytes(trailer[4..].try_into().expect("four bytes"));
    let crc = crc.finalize();
    if crc != stated_crc {
        return Err(format!("CRC-32 {crc:08x} where the trailer states {stated_crc:08x}"));
    }
    // `ISIZE` is the plaintext's length mod 2^32 (RFC 1952, 2.3.1).
    if total as u32 != stated_size {
        return Err(format!("{total} plaintext bytes where `ISIZE` states {stated_size} mod 2^32"));
    }
    Ok(total)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(why) => {
            eprintln!("gzip_inflate: {why}");
            eprintln!("usage: gzip_inflate --source <file.gz> --backend zlib-rs|miniz-oxide");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("gzip_inflate: {why}");
            ExitCode::from(1)
        }
    }
}
