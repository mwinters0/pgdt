//! The `.xz` adapter: the codec over `xz_seek`'s layout, its seek table as
//! this crate hands it out, the walk over a fetched file that builds one, and
//! the bridge from a [`Positioned`] read to `xz_seek`'s own source — with its
//! table, the only production modules of the compressed layer naming
//! `xz_seek`.

use std::ops::Range;
use std::sync::Arc;

use pgdt_core::{Error, Result};

use super::{Codec, CompressionIndex, DumpFile, Positioned, UnitRead, short_read};

mod table;

pub use self::table::SeekTable;
pub(crate) use self::table::XZ_MAGIC;

/// The codec's name, which the codec error carries.
const XZ: &str = "xz";

/// An `xz_seek` failure as this crate's codec error, naming its codec.
fn xz_error(cause: xz_seek::Error) -> Error {
    Error::codec(XZ, cause)
}

/// Whether `error` is `xz_seek` refusing a table handed in as not describing
/// the file ([`super::index_refused`]).
pub(super) fn refuses_table(error: &Error) -> bool {
    matches!(error.codec_cause(), Some(xz_seek::Error::InvalidTable { .. }))
}

/// An `.xz` file's blocks as the compressed arms read them, each block a unit.
///
/// **The table, the memory limit, the verification state and the backend as
/// one sourceless `xz_seek::Layout`** — everything a decode needs that is not
/// bytes, and the only thing that hands out an `xz_seek::BlockTask`, which is
/// `Copy` and owns its block, so naming one takes no lock
/// (`docs/design/decisions.md`, "D14").
pub(super) struct XzCodec {
    layout: xz_seek::Layout,
    /// The layout's own table, aliased rather than copied, so every table
    /// read takes no lock and costs no second copy:
    /// `xz_seek::Layout::index_shared` hands out the `Arc` the layout holds.
    table: Arc<xz_seek::SeekTable>,
}

impl XzCodec {
    /// The codec over a table, checked against the compressed file's own
    /// length and for internal consistency, **reading no bytes**; a table that
    /// does not describe the file is `xz_seek::Error::InvalidTable`
    /// ([`refuses_table`]).
    ///
    /// **`Builder::new()` rather than a configured one**, and every
    /// constructor comes through here, so a walked file and a cached one, a
    /// local one and a fetched one, decode identically — `Verify::Full`
    /// included, which is `xz_seek`'s own default.
    pub(super) fn over(table: SeekTable, stored_size: u64) -> Result<Self> {
        let layout = xz_seek::Builder::new().layout(table.0, stored_size).map_err(xz_error)?;
        let table = layout.index_shared();
        Ok(Self { layout, table })
    }

    /// Walk the stream footers of the local `.xz` file for its table, saying
    /// so before and after: the walk this names is the one a cached index
    /// exists to skip, so only this constructor emits it
    /// (`docs/design/decisions.md`, "D64").
    pub(super) fn walk(file: &DumpFile, stored_size: u64) -> Result<Self> {
        let path = file.path();
        tracing::info!(path = %path.display(), "seek table build started");
        let table = xz_seek::SeekTable::from_source(&Bridge(file)).map_err(xz_error)?;
        tracing::info!(
            path = %path.display(),
            streams = table.stream_count(),
            blocks = table.block_count(),
            "seek table build complete",
        );
        Self::over(SeekTable(table), stored_size)
    }

    fn task(&self, unit: usize) -> Result<xz_seek::BlockTask> {
        self.layout.block_task(unit).ok_or_else(short_read)
    }
}

impl Codec for XzCodec {
    fn name(&self) -> &'static str {
        XZ
    }

    fn index(&self) -> CompressionIndex {
        CompressionIndex::Xz(SeekTable((*self.table).clone()))
    }

    fn plaintext_size(&self) -> u64 {
        self.table.uncompressed_size()
    }

    fn compressed_size(&self) -> u64 {
        self.table.compressed_file_size
    }

    fn unit_count(&self) -> usize {
        self.table.block_count()
    }

    fn units_in(&self, range: Range<u64>) -> Range<usize> {
        self.table.blocks_in(range)
    }

    fn plaintext(&self, unit: usize) -> Option<Range<u64>> {
        self.table.blocks.get(unit).map(xz_seek::BlockEntry::uncompressed_range)
    }

    /// `total_size()` rather than `unpadded_size`, which is what
    /// `BlockTask::compressed_range` asks for: a window cut shorter would stop
    /// before the block's check.
    fn compressed(&self, unit: usize) -> Option<Range<u64>> {
        self.layout.block_task(unit).map(|task| task.compressed_range())
    }

    fn largest_unit(&self) -> u64 {
        self.table.max_block_uncompressed()
    }

    fn widest_compressed(&self) -> u64 {
        self.table.blocks.iter().map(xz_seek::BlockEntry::total_size).max().unwrap_or(0)
    }

    /// `xz_seek::Layout::decode_footprint`: the decoder and the compressed
    /// input chunk it builds over a source that lends nothing.
    fn pulling_footprint(&self) -> u64 {
        self.layout.decode_footprint()
    }

    /// `xz_seek::Layout::decoder_bytes`: the decoder alone, a lending source
    /// being read through `slice_at`.
    fn lending_footprint(&self) -> u64 {
        self.layout.decoder_bytes()
    }

    fn decode(&self, unit: usize, source: &dyn Positioned, out: &mut [u8]) -> Result<()> {
        self.task(unit)?.decode_into(&Bridge(source), out).map_err(xz_error)
    }

    fn begin(&self, unit: usize, source: &dyn Positioned) -> Result<Box<dyn UnitRead>> {
        let read = self.task(unit)?.begin(&Bridge(source)).map_err(xz_error)?;
        Ok(Box::new(XzRead(read)))
    }
}

/// A [`Positioned`] read as `xz_seek`'s own source, every call forwarded —
/// the loan included, or a fetched window's decode would copy what it holds.
struct Bridge<'a>(&'a dyn Positioned);

impl xz_seek::CompressedSource for Bridge<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read_at(offset, buf)
    }

    fn size(&self) -> std::io::Result<u64> {
        self.0.size()
    }

    fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
        self.0.slice_at(offset, len)
    }
}

/// How many plaintext bytes a piecewise read throws away per decoder call
/// while skipping to its first wanted byte.
///
/// On the stack, so a read that holds no whole block allocates nothing beyond
/// the buffer it is filling — which is the budget that arm exists to respect —
/// and large enough that skipping a 24 MiB block is thousands of decoder calls
/// rather than millions. It is `xz_seek`'s own drain chunk, which sizes the
/// same skip on the other side of `complete`.
const PIECEWISE_SKIP_CHUNK: usize = 4096;

/// One `xz_seek::BlockRead`, the handle the live piecewise decode holds.
struct XzRead(xz_seek::BlockRead);

impl UnitRead for XzRead {
    fn position(&self) -> u64 {
        self.0.position()
    }

    fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    fn advance_to(&mut self, source: &dyn Positioned, to: u64) -> Result<()> {
        let mut skip = [0u8; PIECEWISE_SKIP_CHUNK];
        self.0.advance_to(&Bridge(source), to, &mut skip).map_err(xz_error)
    }

    fn read(&mut self, source: &dyn Positioned, out: &mut [u8]) -> Result<usize> {
        self.0.read(&Bridge(source), out).map_err(xz_error)
    }

    /// `xz_seek::Verify::Full`'s seek-away escape, a call we make. A
    /// `--check=none` stream has nothing to compare, so draining it would
    /// decode a block's remainder to verify nothing — the one case that escape
    /// also skips.
    fn complete(self: Box<Self>, source: &dyn Positioned) -> Result<()> {
        if self.0.task().check() == xz_seek::Check::None {
            return Ok(());
        }
        self.0.complete(&Bridge(source)).map_err(xz_error)
    }
}

/// Build an `.xz` file's seek table by driving `xz_seek`'s footer walk over an
/// **asynchronous** source, and say how many fetches it took.
///
/// `xz_seek::Walk` is the footer parse with the reads taken out: it hands out
/// a range and takes the bytes back, holding no borrow and no lifetime, so the
/// fetch is ours and may `await`. That is the whole reason a dump reached over
/// the network can be `.xz` at all —
/// `xz_seek::Reader` pulls through a *synchronous* positional trait, and
/// nothing can `await` inside it
/// (`docs/design/decisions.md`, "D14").
///
/// `leading` is what the caller already holds of the file's first bytes — an
/// [`Origin`](crate::Origin) probe's, normally. The walk's first request is the six magic
/// bytes at offset zero, which is exactly what that probe fetched, so a caller
/// passing them spends no round trip on them. Passing an empty slice is
/// correct and costs one.
///
/// **The count it returns is the file's, not this driver's.** The walk asks for
/// a footer, an index, a header and at least one padding probe per stream, in a
/// strictly backward chain where each request's position comes out of the bytes
/// of the one before it — so neither coalescing nor concurrency buys anything
/// on its own (`docs/design/decisions.md`, "D18").
///
/// Deficiency register: `deficiency: KD36` — this driver fetches exactly what
/// it is asked for, so a cold walk over a dump compressed as many small streams
/// is four round trips a stream holding a block before a row is read. The
/// remedy is a **straddling window**: one read per stream positioned backward
/// from the pending request and sized to cover what the walk asks for next,
/// which answers three of every stream's four requests because a stream's
/// header is adjacent to its predecessor's padding, footer and index. **One fetch a stream is then a
/// floor, not a cost still looking for a remedy** — the distance to the next
/// boundary is knowable only from the index just read, and this file family has
/// no stream stride to predict it from. What would beat the floor is not a
/// better driver but a forward table build, which stops paying separately for
/// the walk at all. **(c) unowned**; promoted by a phase that tunes the network,
/// which is where the fetch policy this source defers belongs, and which is also
/// what would take the figure. Two copies say "several round trips per
/// stream" because of this and are rewritten by its closure: the warning
/// `announce_remote_walk` in `remote.rs` gives, and
/// `docs/manual/dump-inspection.md`, "Reading a dump over HTTP".
#[cfg(any(feature = "http", test))]
pub(crate) async fn walk_seek_table(
    source: &dyn crate::ByteRangeSource,
    stored_size: u64,
    leading: &[u8],
) -> Result<(SeekTable, usize)> {
    let (mut walk, mut request) = xz_seek::Walk::begin(stored_size);
    let mut fetches = 0usize;
    loop {
        let asked = usize::try_from(request.len()).map_err(|_| short_read())?;
        let step = if request.start() == 0 && asked <= leading.len() {
            walk.supply(request, &leading[..asked]).map_err(xz_error)?
        } else {
            fetches += 1;
            let bytes = source.read_range(request.start(), asked).await?;
            walk.supply(request, &bytes).map_err(xz_error)?
        };
        match step {
            xz_seek::Step::Need(next) => request = next,
            xz_seek::Step::Done(table) => return Ok((SeekTable(table), fetches)),
        }
    }
}

#[cfg(test)]
mod tests;
