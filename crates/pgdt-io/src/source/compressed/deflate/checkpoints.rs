//! What the compression index names of gzip: its checkpoints as this crate
//! hands them out, the provider a unit decode reads its window through, and
//! the magic recognition compares — a leaf, so the index can name the layout
//! without reaching the codec that decodes it.

use std::ops::Range;
use std::sync::Arc;

pub(crate) use gz_seek::GZIP_MAGIC;

/// Where a gzip file's units sit — its checkpoints, each a place a decode can
/// begin, the span from one to the next a unit — and where each checkpoint's
/// window is read from: [`CompressionIndex::Gzip`]'s layout.
///
/// **Opaque**: `gz_seek`, whose index this wraps, is a crate to be split off
/// this workspace (`docs/design/roadmap-P15-gzip.md`, "Settled in the
/// grilling"), so its types are not part of this crate's API. What an
/// embedder reads is the file's shape, through the methods below; what it
/// persists is the checkpoints whole, under the `serde` feature.
///
/// **Only offsets, bit positions and CRCs are held.** A unit decode inside a
/// member needs the plaintext before its checkpoint, which is read through the
/// [`WindowProvider`] riding beside the checkpoints as that decode begins, so
/// the index stays the size of its offsets. The provider is neither persisted
/// nor compared: two indexes are equal where they describe the same
/// checkpoints, whoever keeps the windows.
///
/// [`CompressionIndex::Gzip`]: super::super::CompressionIndex::Gzip
#[derive(Clone)]
pub struct GzipIndex {
    pub(crate) index: gz_seek::Index,
    pub(crate) windows: Option<Arc<dyn WindowProvider>>,
}

/// Where a unit decode reads its checkpoint's window from: the plaintext of
/// the checkpoint's member before it, the last 32 KiB of it at most, which the
/// forward pass that placed the checkpoint handed out as it did.
///
/// **Implemented by whoever keeps the windows** — a cache, which holds them in
/// a section of its own read positionally, or a test's memory — so this crate
/// names nothing that holds them, and the index keeps none resident.
pub trait WindowProvider: Send + Sync {
    /// Fill `out` with the window of checkpoint `checkpoint` — the one unit
    /// `checkpoint` begins at, or an incomplete index's last, which begins
    /// none yet: exactly [`GzipIndex::window_len`] bytes, which `out` is long.
    /// Never asked for an empty one.
    fn read_window(&self, checkpoint: usize, out: &mut [u8]) -> std::io::Result<()>;
}

impl GzipIndex {
    /// The same checkpoints, their windows read through `windows`: what a
    /// caller holding them attaches before handing a persisted index back.
    pub fn with_windows(self, windows: Arc<dyn WindowProvider>) -> Self {
        Self { windows: Some(windows), ..self }
    }

    /// Whether a window provider rides beside the checkpoints.
    pub fn has_windows(&self) -> bool {
        self.windows.is_some()
    }

    /// Whether the checkpoints reach the file's end: an index a pass stopped
    /// short of it describes a prefix.
    pub fn is_complete(&self) -> bool {
        self.index.is_complete()
    }

    /// Whether seeking buys anything: more than one unit. A one-unit file is
    /// read correctly, necessarily by decoding from its start
    /// (`docs/design/decisions.md`, "D19").
    pub fn is_seekable(&self) -> bool {
        self.unit_count() > 1
    }

    /// How many units can be decoded: every checkpoint's, but the last's while
    /// the index is incomplete.
    pub fn unit_count(&self) -> usize {
        self.index.unit_count()
    }

    /// How many members the file holds, as far as the index reaches.
    pub fn member_count(&self) -> usize {
        self.index.checkpoints().iter().filter(|c| c.start == gz_seek::Start::Member).count()
    }

    /// The plaintext spacing the checkpoints were placed at.
    pub fn spacing(&self) -> u64 {
        self.index.spacing().get()
    }

    /// Each unit's plaintext range, in file order: adjacent, and together the
    /// whole of [`GzipIndex::uncompressed_size`] once the index is complete.
    pub fn unit_ranges(&self) -> impl ExactSizeIterator<Item = Range<u64>> + '_ {
        (0..self.unit_count()).map(|unit| self.task(unit).plaintext_range())
    }

    /// The largest unit's plaintext length, which is what a reader must hold
    /// to decode any unit whole; zero for a file with none.
    pub fn largest_unit(&self) -> u64 {
        self.unit_ranges().map(|unit| unit.end - unit.start).max().unwrap_or(0)
    }

    /// The plaintext's length: the whole file's once complete, otherwise as
    /// far as the last checkpoint.
    pub fn uncompressed_size(&self) -> u64 {
        match self.index.size() {
            gz_seek::Size::Exact(len) | gz_seek::Size::Open { at_least: len } => len,
        }
    }

    /// The compressed file's length, once the index is complete: the length
    /// a source's stored size must match for this index to describe it.
    pub fn compressed_size(&self) -> Option<u64> {
        self.index.end().map(|end| end.compressed)
    }

    /// How many checkpoints the index holds: one a unit, and an incomplete
    /// index's last besides, which a pass resumes from.
    pub fn checkpoint_count(&self) -> usize {
        self.index.checkpoints().len()
    }

    /// The length of checkpoint `checkpoint`'s window, which a decode of the
    /// unit it begins reads: zero at a member start, its member's history
    /// before it, up to 32 KiB, otherwise.
    pub fn window_len(&self, checkpoint: usize) -> Option<usize> {
        self.index.checkpoints().get(checkpoint).map(gz_seek::Checkpoint::window_len)
    }

    /// Fill `out`, [`GzipIndex::window_len`] long, with checkpoint
    /// `checkpoint`'s window, read through the provider riding beside the
    /// checkpoints — what a cache saving this index writes out — or nothing
    /// read for an empty one.
    ///
    /// # Errors
    ///
    /// The provider's, and [`std::io::ErrorKind::InvalidInput`] for a
    /// checkpoint the index does not hold, an `out` of another length, or a
    /// window with no provider to read it through.
    pub fn read_window(&self, checkpoint: usize, out: &mut [u8]) -> std::io::Result<()> {
        let invalid = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, why);
        let len = self.window_len(checkpoint).ok_or_else(|| invalid("no such checkpoint"))?;
        if out.len() != len {
            return Err(invalid("a window is read whole, into a buffer of its length"));
        }
        if len == 0 {
            return Ok(());
        }
        let windows = self.windows.as_ref().ok_or_else(|| {
            invalid("a gzip checkpoint's window has no provider to be read through")
        })?;
        windows.read_window(checkpoint, out)
    }

    /// Unit `unit`, which the caller has counted.
    pub(crate) fn task(&self, unit: usize) -> gz_seek::UnitTask {
        self.index.unit(unit).expect("a counted unit")
    }
}

impl PartialEq for GzipIndex {
    /// The checkpoints alone: a provider is where the windows are read from,
    /// not what they are.
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl Eq for GzipIndex {}

impl std::fmt::Debug for GzipIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GzipIndex")
            .field("index", &self.index)
            .field("windows", &self.windows.as_ref().map(|_| "attached"))
            .finish()
    }
}

/// The persisted form: the parts `gz_seek::Index::from_parts` takes back,
/// which validates them, so a damaged cache is refused as undecodable rather
/// than handed out as an index.
#[cfg(feature = "serde")]
mod persisted {
    use std::num::NonZeroU64;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::GzipIndex;

    #[derive(Serialize, Deserialize)]
    struct Parts {
        spacing: NonZeroU64,
        checkpoints: Vec<Point>,
        end: Option<End>,
    }

    /// A checkpoint: `window` is its length inside a member, `None` at a
    /// member's start.
    #[derive(Serialize, Deserialize)]
    struct Point {
        plaintext: u64,
        bit: u64,
        window: Option<u16>,
        crc: Option<u32>,
    }

    #[derive(Serialize, Deserialize)]
    struct End {
        plaintext: u64,
        compressed: u64,
    }

    impl Serialize for GzipIndex {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let index = &self.index;
            let checkpoints = index
                .checkpoints()
                .iter()
                .map(|c| Point {
                    plaintext: c.plaintext,
                    bit: c.bit,
                    window: match c.start {
                        gz_seek::Start::Member => None,
                        gz_seek::Start::Block { window } => Some(window),
                    },
                    crc: c.crc,
                })
                .collect();
            let end = index.end().map(|e| End { plaintext: e.plaintext, compressed: e.compressed });
            Parts { spacing: index.spacing(), checkpoints, end }.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for GzipIndex {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let parts = Parts::deserialize(deserializer)?;
            let checkpoints = parts
                .checkpoints
                .into_iter()
                .map(|p| gz_seek::Checkpoint {
                    plaintext: p.plaintext,
                    bit: p.bit,
                    start: match p.window {
                        None => gz_seek::Start::Member,
                        Some(window) => gz_seek::Start::Block { window },
                    },
                    crc: p.crc,
                })
                .collect();
            let end = parts
                .end
                .map(|e| gz_seek::End { plaintext: e.plaintext, compressed: e.compressed });
            let index = gz_seek::Index::from_parts(parts.spacing, checkpoints, end)
                .map_err(serde::de::Error::custom)?;
            Ok(GzipIndex { index, windows: None })
        }
    }
}
