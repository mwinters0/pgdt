//! What a compressed file's codec answers the two arms: the `Codec` trait over
//! its units, the forward-only `UnitRead` handle, the `Positioned` read a
//! decode pulls its compressed bytes through and the local dump it reads, and
//! the short read every arm and codec reports a read past the plaintext as.

use std::ops::Range;
use std::path::{Path, PathBuf};

use pgdt_core::{Error, Result};

use super::CompressionIndex;
use crate::source::local::{OPEN_DUMP, READ_DUMP, dump_failure};

/// What a unit decode reads its compressed bytes through: a file pulled from,
/// or a window of bytes already fetched, in the file's own coordinates.
///
/// **Fill-or-end**: a [`Positioned::read_at`] shorter than its buffer means
/// the file ended there and nothing else, and [`Positioned::slice_at`] lends a
/// range only where it holds every byte of it, declining being always
/// correct. A codec's adapter turns this into what its crate reads through —
/// `xz_seek`'s source trait, or the bytes a `pgdt_gzip_seek` decode is fed — so the
/// arms name no codec's crate.
pub(crate) trait Positioned: Send + Sync {
    /// Fill `buf` with the bytes at `offset`, answering how many were written.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize>;
    /// The whole compressed file's length.
    fn size(&self) -> std::io::Result<u64>;
    /// The `len` bytes at `offset`, borrowed, where every one is held.
    fn slice_at(&self, _offset: u64, _len: usize) -> Option<&[u8]> {
        None
    }
}

/// A local compressed dump: the file, read positionally and never seeked, and
/// the path it was opened at, which every failure of it names.
///
/// **A read failing names the path inside its `io::Error`**, an
/// [`Error::File`] as the payload ([`Error::into_io`]), so [`Positioned`]
/// stays `io::Result` and no decode path changes: through `xz_seek`'s or
/// `pgdt_gzip_seek`'s error the payload's message names the file, and where no
/// codec wrapped it, `From<io::Error>` hands back the [`Error::File`] itself.
/// *Rejected:* a path-carrying error type in the trait — `xz_seek`'s source
/// trait answers `io::Result`, so its reads would carry ours as a payload all
/// the same.
pub(crate) struct DumpFile {
    file: std::fs::File,
    path: PathBuf,
}

impl DumpFile {
    /// The dump at `path`, opened for reading.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(dump_failure(OPEN_DUMP, path))?;
        Ok(Self { file, path: path.to_path_buf() })
    }

    /// The file at `path` opened for writing alone, so every read of it fails
    /// and a `stat` answers its length: a failing read on any filesystem.
    #[cfg(test)]
    pub(crate) fn unreadable(path: &Path) -> Self {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        Self { file, path: path.to_path_buf() }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// A `stat` of the file.
    pub(crate) fn metadata(&self) -> Result<std::fs::Metadata> {
        self.file.metadata().map_err(dump_failure(READ_DUMP, &self.path))
    }

    /// The file's modification time.
    pub(crate) fn modified(&self) -> Result<std::time::SystemTime> {
        self.metadata()?.modified().map_err(dump_failure(READ_DUMP, &self.path))
    }

    /// `cause`, met reading the file, as the `io::Error` naming it.
    fn failed(&self, cause: std::io::Error) -> std::io::Error {
        Error::file(READ_DUMP, &self.path, cause).into_io()
    }
}

/// Positioned reads on the file, looping past a short `pread` that is not the
/// file's end — what makes the fill-or-end contract true of a file. It holds
/// nothing, so it lends nothing.
#[cfg(unix)]
impl Positioned for DumpFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            match FileExt::read_at(&self.file, &mut buf[done..], offset + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(self.failed(e)),
            }
        }
        Ok(done)
    }

    fn size(&self) -> std::io::Result<u64> {
        self.file.metadata().map(|m| m.len()).map_err(|e| self.failed(e))
    }
}

/// One compressed file's units, as both arms read them: an `.xz` file's
/// blocks or the spans between a gzip file's checkpoints, numbered in file
/// order.
///
/// **What the arms need and nothing else** — each unit's plaintext and
/// compressed ranges, the largest unit, what a decoder holds, a unit decoded
/// whole out of a positioned source, and a forward-only handle over one — so
/// the local arm and the fetched arm are each written once, and a codec is an
/// implementation of this rather than a third and fourth source
/// (`docs/design/decisions.md`, "D15").
///
/// **Object-safe, held as `Arc<dyn Codec>`**: the codec is chosen at run time,
/// by the file's first bytes or the index a cache hands back
/// (`docs/design/decisions.md`, "D15").
pub(crate) trait Codec: Send + Sync {
    /// The format's name, as a message and the codec error say it.
    fn name(&self) -> &'static str;
    /// What a cache persists to open this file again without rebuilding it
    /// (`docs/design/decisions.md`, "D18").
    fn index(&self) -> CompressionIndex;
    /// The file's whole plaintext length.
    fn plaintext_size(&self) -> u64;
    /// The compressed file's length, as the index describes it.
    fn compressed_size(&self) -> u64;
    /// How many units the file holds.
    fn unit_count(&self) -> usize;
    /// The units whose plaintext overlaps `range`, as an index range.
    fn units_in(&self, range: Range<u64>) -> Range<usize>;
    /// Unit `unit`'s plaintext range, `None` past the last.
    fn plaintext(&self, unit: usize) -> Option<Range<u64>>;
    /// Unit `unit`'s whole compressed extent — what a fetch must hold for a
    /// decode of it to reach its check — `None` past the last.
    fn compressed(&self, unit: usize) -> Option<Range<u64>>;
    /// The largest unit's plaintext length: what a reader must hold to decode
    /// any unit whole, zero for a file with none.
    fn largest_unit(&self) -> u64;
    /// The largest unit's compressed extent: the widest window a fetch of one
    /// makes.
    fn widest_compressed(&self) -> u64;
    /// What one decode retains besides its output over a source that lends
    /// nothing, so that the decoder builds its own input buffer.
    fn pulling_footprint(&self) -> u64;
    /// What one decode retains besides its output over a source that lends,
    /// so that no input buffer is built.
    fn lending_footprint(&self) -> u64;
    /// Decode unit `unit` whole into `out`, exactly its plaintext long,
    /// comparing its check before returning.
    fn decode(&self, unit: usize, source: &dyn Positioned, out: &mut [u8]) -> Result<()>;
    /// A forward-only handle at the start of unit `unit`.
    fn begin(&self, unit: usize, source: &dyn Positioned) -> Result<Box<dyn UnitRead>>;
}

/// A forward-only decode of one unit, handed the source on every call rather
/// than holding it, which is what lets the fetched arm keep its window beside
/// the handle and the local arm its file in the source.
pub(crate) trait UnitRead: Send {
    /// The plaintext offset the next byte read comes from.
    fn position(&self) -> u64;
    /// Whether the unit has been read to its end.
    fn is_finished(&self) -> bool;
    /// Decode forward to plaintext offset `to`, discarding what lies before it.
    fn advance_to(&mut self, source: &dyn Positioned, to: u64) -> Result<()>;
    /// What the decoder produced into `out`, as soon as it produced any, zero
    /// at the unit's end.
    fn read(&mut self, source: &dyn Positioned, out: &mut [u8]) -> Result<usize>;
    /// Leave the unit, comparing its check — decoding its remainder where it
    /// carries one.
    fn complete(self: Box<Self>, source: &dyn Positioned) -> Result<()>;
}

/// The error a read past the end of the plaintext is — of the kind a short
/// `read_exact_at` raises on the plain source, `UnexpectedEof`, in words of
/// its own.
pub(crate) fn short_read() -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "compressed stream ended before the requested range",
    ))
}
