//! The cache file — its format, its header, what it encodes, and the reading
//! and the writing of it ([`read_cache_file`], [`write_cache_file`]) that
//! every load and save goes through, a gzip cache's window section after it
//! ([`windows`]) — and what a read of it finds
//! ([`CacheStatus`], [`LoadedCache`]), with the shape of the container under
//! its offsets ([`CompressionShape`]) and the rest it persists
//! ([`CacheEnvelope`]). Those are the cache's API; the format is not, reached
//! through `internal` for the tests that write a cache by hand.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, ErrorKind, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bincode::error::{DecodeError, EncodeError};
use serde::{Deserialize, Serialize};

use super::identity::{OriginMatch, SourceIdentity, WeakIdentity};
use pgdt_core::{Error, Result, Unusable};
use pgdt_io::{CompressionIndex, Extent};
use pgdt_model::{CACHE_FORMAT_VERSION, DumpIndex, calendar_end};

mod windows;

/// What [`Error::File`] says was being done to a cache that failed to open.
const OPEN_CACHE: &str = "open the cache at";
/// What [`Error::File`] says was being done to a cache a read of failed.
const READ_CACHE: &str = "read the cache at";
/// What [`Error::File`] says was being done to the file a save writes before
/// renaming it over the cache ([`beside`]).
const WRITE_BESIDE: &str = "write the cache's new copy at";
/// What [`Error::File`] says was being done where that rename failed.
const REPLACE_CACHE: &str = "rename the cache's new copy over the cache at";

/// The bytes every cache file opens with, ahead of [`CACHE_FORMAT_VERSION`] as
/// a little-endian `u32`, then the encoded [`CacheFile`] and, for a gzip
/// source, its window section ([`windows`]). **Both are read
/// before the rest is decoded, and neither is ever reshaped**, so any build
/// tells a file that is not a pgdt cache ([`Unusable::NotACache`]) from another
/// build's ([`Unusable::UnsupportedVersion`]) — the difference between a file
/// no scan may overwrite and one `--overwrite-unusable-cache` may
/// ([`CacheMode::with_overwrite_unusable`](super::CacheMode::with_overwrite_unusable)).
pub const CACHE_MAGIC: [u8; 8] = *b"pgdtcach";

/// What produced the indexed blocks' byte offsets. Plain-format offsets are
/// raw file positions; a future archive format's would be entry-relative, so
/// the two must never be silently conflated.
///
/// A compressed source's indexed offsets are *also* plain-format offsets —
/// byte for byte what a plain scan of the same decompressed content
/// produces — so a compressed source never sets this to anything but
/// `Plain`; what changes is the [`CompressionIndex`] beside it
/// (`docs/design/decisions.md`, "D21").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerKind {
    Plain,
}

/// What a cache file holds after [`CACHE_MAGIC`] and its version.
/// [`CacheFileRef`] is what a save encodes, the same fields in this order, so
/// the two change together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheFile {
    pub container_kind: ContainerKind,
    /// What compression sits between this cache's plain-format offsets and
    /// the bytes on disk — `None` for a source that needs no such index
    /// (`docs/design/decisions.md`, "D21"). A sibling of [`ContainerKind`],
    /// not a value of it: see that type's docs.
    ///
    /// Saved from
    /// [`ByteRangeSource::compression_index`](pgdt_io::ByteRangeSource::compression_index),
    /// which a compressed source answers from the index it already built
    /// while opening, so persisting a cache never rebuilds it; read back
    /// before any source exists ([`CacheMode::open`](super::CacheMode::open)),
    /// which spares every later command that build.
    pub compression: Option<CompressionIndex>,
    pub identity: SourceIdentity,
    /// The addressable (decompressed, for a compressed source) length —
    /// [`ByteRangeSource::extent`](pgdt_io::ByteRangeSource::extent) as
    /// observed at save time, open where the source's end had not been read
    /// yet. Its own field, not an alias of `identity`'s stored size
    /// (`docs/design/decisions.md`, "D21"), and what
    /// [`LoadedCache::extent`] hands out for the coverage arithmetic.
    pub extent: Extent,
    /// The last day of the calendar the index's unrepresentable counts were
    /// taken under ([`pgdt_model::calendar_end`]): a build whose calendar
    /// ends elsewhere refuses a cache holding one
    /// ([`Unusable::CalendarChanged`]; `docs/design/decisions.md`, "D96").
    pub calendar_end: i32,
    pub index: DumpIndex,
}

/// The shape of the container a cache's offsets sit under, read off the
/// persisted [`CompressionIndex`], each codec's terms mapped onto one shape:
/// an `.xz` file's streams and blocks, a gzip file's members and the units
/// between its checkpoints.
///
/// Three numbers, each answering a question the user is otherwise sent to
/// `xz --list` for. `max_block_uncompressed` is the largest term of what a
/// memory budget is compared against — four times over, one reader's block
/// beside the further blocks the pool keeps, plus the chunk buffer and the
/// decoder's own working memory — so it is most of the read-buffer budget
/// `--memory` has to leave when a query says the block path was declined, and
/// that query's own note states the whole of it
/// (`pgdt_query::PlanNoteKind::CompressedBlockPathDeclined`); `blocks` is how much
/// seeking the file offers at all; `streams` is what explains a slow first
/// command, a concatenated file costing one seek per stream to walk
/// (`docs/design/decisions.md`, "D18").
///
/// **Derived, not stored** — a projection of the seek table the envelope
/// already carries, computed on load like the diagnostics beside it. It is a
/// property of the *file*, which is why a cache answers it with no dump
/// present, unlike a budget decline, which depends on the run and reaches a
/// caller as `pgdt_query::PlanNoteKind::CompressedBlockPathDeclined`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CompressionShape {
    /// The container: `"xz"` or `"gzip"`. A word rather than an enum, being a
    /// name to print.
    pub container: &'static str,
    pub streams: usize,
    pub blocks: usize,
    pub max_block_uncompressed: u64,
}

impl CompressionShape {
    pub(super) fn of(index: &CompressionIndex) -> Self {
        let streams = match index {
            CompressionIndex::Xz(table) => table.stream_count(),
            CompressionIndex::Gzip(checkpoints) => checkpoints.member_count(),
        };
        Self {
            container: index.codec(),
            streams,
            blocks: index.unit_count(),
            max_block_uncompressed: index.largest_unit(),
        }
    }
}

/// Everything a cache file persists beside its index and `extent`, as it
/// was read — handed out on [`LoadedCache`] so a reporting caller can export the whole file, which `pgdt info --json`
/// does (`docs/design/decisions.md`, "D67").
///
/// **Opaque to Rust, whole to serde**: the fields stay private, so no caller
/// matches on [`SourceIdentity`] or reads the index through this, and its
/// `Serialize` is the persisted types' own, variant tags included. The index
/// serializes as `compression_index`, the name `compression` being
/// [`CompressionShape`]'s wherever the two sit side by side. Moved out of the
/// decoded file rather than copied, so carrying it costs nothing the load did
/// not already hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheEnvelope {
    pub(super) format_version: u32,
    pub(super) container_kind: ContainerKind,
    pub(super) compression_index: Option<CompressionIndex>,
    pub(super) identity: SourceIdentity,
    pub(super) calendar_end: i32,
}

/// What a read of a cache path found: nothing, something that cannot be
/// used and why, or a cache this build can read, checked against the live
/// source's identity where there is one — see the parent module's docs.
///
/// Each unusable outcome is named and stays named all the way to the caller:
/// an open refuses with it, or carries it across as
/// [`Held::Replacing`](super::Held::Replacing), rather than answering `None`
/// (`docs/design/decisions.md`, "D22").
// One value a run, moved once or twice and never stored in bulk, so boxing
// the loaded cache would buy an allocation and save nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    /// Nothing at this path.
    Missing,
    /// Something is at this path that cannot be used, and why.
    Unusable(Unusable),
    /// A cache this build can read, mapping the whole file or a prefix of it
    /// ([`LoadedCache::is_complete`]).
    Loaded(LoadedCache),
}

/// A cache this build can read, and what its file records beside the index.
///
/// **Complete or not is the index's own coverage**, asked of
/// [`LoadedCache::is_complete`]: a short one is a real, not-yet-finished scan
/// (a preamble-only scan, or a query that stopped once its target settled),
/// not a defect (`docs/design/decisions.md`, "D22"). What "not enough" means is
/// the caller's: `pgdt info` states the coverage
/// (`docs/design/decisions.md`, "The CLI"), a scan resuming builds on the
/// partial index, and the DataFusion provider refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCache {
    pub index: DumpIndex,
    /// What the source's current modification signal says against the one
    /// recorded at save time, which itself never makes the cache unusable
    /// (see the module docs); always agreeing where no source was compared.
    pub weak: WeakIdentity,
    /// What the source's origin says against the recorded one, as advisory
    /// as `weak`.
    pub origin: OriginMatch,
    /// [`CacheFile::extent`]: a reporting caller states coverage against
    /// it, and a cache-only caller has no live source to stat.
    pub extent: Extent,
    /// The container's shape where one sits under these offsets and `None` for
    /// a plain file — read off the persisted index, so a cache-only
    /// caller answers it with no dump present ([`CompressionShape`]).
    pub compression: Option<CompressionShape>,
    /// The rest of what the file persists, for a caller exporting it
    /// ([`CacheEnvelope`]).
    pub envelope: CacheEnvelope,
}

impl LoadedCache {
    /// Whether the index's `scanned_through` reaches the file's recorded
    /// extent — the whole file is mapped, which a map of a source whose end
    /// was not known never is.
    pub fn is_complete(&self) -> bool {
        self.extent.reached_by(self.index.scanned_through)
    }
}

/// Read and header-check the cache at `path`, shared by every read of one.
/// `Err(status)` is an outcome that needs no live source to reach; the two that
/// compare against one are the store's (`super::store`).
///
/// **The header is read before anything is decoded** ([`CACHE_MAGIC`]), so
/// another build's cache is told from foreign bytes whatever its shape; the
/// rest is decoded through a buffered reader, never read whole first
/// (`docs/design/decisions.md`, "D78"). A file ending before its magic
/// ends is not a pgdt cache; one ending after it is [`Unusable::Unreadable`],
/// as any other undecodable content is; an I/O failure reading it is still an
/// error.
pub fn read_cache_file(path: &Path) -> Result<std::result::Result<CacheFile, CacheStatus>> {
    let unusable = |why| Ok(Err(CacheStatus::Unusable(why)));
    let read = |cause| Error::file(READ_CACHE, path, cause);
    let mut reader = match File::open(path) {
        Ok(file) => BufReader::new(file),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(Err(CacheStatus::Missing));
        }
        Err(e) => return Err(Error::file(OPEN_CACHE, path, e)),
    };
    let mut magic = [0u8; CACHE_MAGIC.len()];
    match reader.read_exact(&mut magic) {
        Ok(()) if magic == CACHE_MAGIC => {}
        Ok(()) => return unusable(Unusable::NotACache),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return unusable(Unusable::NotACache),
        Err(e) => return Err(read(e)),
    }
    let mut version = [0u8; 4];
    match reader.read_exact(&mut version) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return unusable(Unusable::Unreadable),
        Err(e) => return Err(read(e)),
    }
    let found = u32::from_le_bytes(version);
    if found != CACHE_FORMAT_VERSION {
        return unusable(Unusable::UnsupportedVersion { found });
    }
    let decoded = bincode::serde::decode_from_reader::<CacheFile, _, _>(
        &mut reader,
        bincode::config::standard(),
    );
    match decoded {
        // Counts taken under another calendar are not this build's, and a
        // cache holding none has nothing to be wrong about.
        Ok(file)
            if file.calendar_end != calendar_end()
                && file.index.blocks().any(|block| block.census.is_some()) =>
        {
            unusable(Unusable::CalendarChanged {
                counted_under: file.calendar_end,
                build: calendar_end(),
            })
        }
        Ok(file) => with_windows(file, reader, path),
        Err(DecodeError::Io { inner, .. }) if inner.kind() != ErrorKind::UnexpectedEof => {
            Err(read(inner))
        }
        Err(_) => unusable(Unusable::Unreadable),
    }
}

/// `file` with its gzip index's provider attached, reading the window section
/// that begins where `reader`, over the cache at `path`, stopped decoding —
/// the encoded file's end, bincode reading no byte past what it decodes.
fn with_windows(
    mut file: CacheFile,
    mut reader: BufReader<File>,
    path: &Path,
) -> Result<std::result::Result<CacheFile, CacheStatus>> {
    let path = path.to_path_buf();
    let read = move |cause| Error::file(READ_CACHE, &path, cause);
    let base = reader.stream_position().map_err(&read)?;
    match windows::attach_section(file.compression.take(), reader.into_inner(), base, read)? {
        Ok(compression) => Ok(Ok(CacheFile { compression, ..file })),
        Err(why) => Ok(Err(CacheStatus::Unusable(why))),
    }
}

/// What [`write_cache_file`] encodes for a save: [`CacheFile`]'s
/// fields in [`CacheFile`]'s order, the index borrowed rather than cloned, so
/// the bytes are the ones an owned `CacheFile` encodes to and
/// [`read_cache_file`] decodes.
#[derive(Serialize)]
pub struct CacheFileRef<'a> {
    pub container_kind: ContainerKind,
    pub compression: Option<CompressionIndex>,
    pub identity: SourceIdentity,
    pub extent: Extent,
    pub calendar_end: i32,
    pub index: &'a DumpIndex,
}

/// Write `file` to `path` as [`read_cache_file`] reads it back — [`CACHE_MAGIC`],
/// [`CACHE_FORMAT_VERSION`] as a little-endian `u32`, the encoded file, then a
/// gzip index's window section — streamed into a file beside `path` and
/// renamed over it ([`write_beside`]).
pub fn write_cache_file(path: &Path, file: &CacheFileRef<'_>) -> Result<()> {
    write_beside(path, |writer, beside| {
        let write = |cause| Error::file(WRITE_BESIDE, beside, cause);
        writer.write_all(&CACHE_MAGIC).map_err(write)?;
        writer.write_all(&CACHE_FORMAT_VERSION.to_le_bytes()).map_err(write)?;
        bincode::serde::encode_into_std_write(file, writer, bincode::config::standard()).map_err(
            |e| match e {
                EncodeError::Io { inner, .. } => write(inner),
                other => Error::cache_encode(other),
            },
        )?;
        windows::write_section(writer, write, file.compression.as_ref())
    })
}

/// Run `encode` into a new file [`beside`] `path`, handed that file's path to
/// name in a failure, then rename it over `path`; on any failure, remove it
/// and leave `path` as it was.
fn write_beside(
    path: &Path,
    encode: impl FnOnce(&mut BufWriter<File>, &Path) -> Result<()>,
) -> Result<()> {
    let beside = beside(path);
    let write = |cause| Error::file(WRITE_BESIDE, &beside, cause);
    let file = OpenOptions::new().write(true).create_new(true).open(&beside).map_err(write)?;
    let written = (|| {
        let mut writer = BufWriter::new(file);
        encode(&mut writer, &beside)?;
        writer.flush().map_err(write)?;
        // Closed before the rename, which some platforms need.
        drop(writer);
        std::fs::rename(&beside, path).map_err(|e| Error::file(REPLACE_CACHE, path, e))?;
        Ok(())
    })();
    if written.is_err() {
        // Best-effort: the error being returned is the one worth reporting.
        let _ = std::fs::remove_file(&beside);
    }
    written
}

/// The file a save writes before renaming it over `path`: in `path`'s own
/// directory, so the rename never crosses a filesystem, and named
/// `<cache file name>.<pid>-<n>.tmp`, unique to this process and this save,
/// so two saves of one cache — two processes, or two tasks of one — never
/// write into one file.
fn beside(path: &Path) -> PathBuf {
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let mut name: OsString = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    name.push(format!(".{}-{}.tmp", std::process::id(), SAVES.fetch_add(1, Ordering::Relaxed)));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests;
