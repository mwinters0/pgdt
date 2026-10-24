//! The index: a gzip file's checkpoints, and the units between them.
//!
//! **One layout for every shape a gzip file takes.** A checkpoint is a place a
//! decode can begin: a plaintext offset, the compressed bit the deflate data
//! resumes at, and the window of history a back-reference there may reach —
//! none at a member's start, which begins with no history. A single member's
//! checkpoints are block boundaries a forward pass chose; a BGZF file's are
//! member starts, chosen alike by its walk and by a forward pass; a file of
//! several members has both. The span
//! from one checkpoint to the next is a **unit**, the thing a caller decodes
//! independently, as an `.xz` block is.
//!
//! **No window is held here.** A checkpoint states its window's length; the
//! window itself is the forward pass's output, kept wherever the caller keeps
//! it and handed back to a decode that needs it, so the index stays the size
//! of its offsets.
//!
//! **An index is a shareable handle**: [`Index`] is a reference-counted value,
//! so a clone is a count and never a copy, and a [`UnitTask`] carries what one
//! decode needs by value, holding no index at all.

use std::num::NonZeroU64;
use std::ops::Range;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::zlib::WINDOW_SIZE;

/// Where a checkpoint's decode begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Start {
    /// At a member's header, with no history. Its bit is a byte's start.
    Member,
    /// Inside a member's deflate data, at the start of a block, with the
    /// member's last `window` bytes of plaintext as history — all of it, up
    /// to 32 KiB.
    Block {
        /// The window's length.
        window: u16,
    },
}

/// A place a decode can begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Checkpoint {
    /// Plaintext offset of the first byte a decode from here produces.
    pub plaintext: u64,
    /// Compressed offset, in bits, the decode begins at.
    pub bit: u64,
    /// Whether the decode begins at a header or inside a member.
    pub start: Start,
    /// The CRC-32 of the unit this checkpoint begins, where one was recorded:
    /// a forward pass records every unit's but one of whole BGZF members,
    /// which their own trailers check, and a walk none, its units being all
    /// such; the last unit of an unfinished pass has none yet.
    pub crc: Option<u32>,
}

impl Checkpoint {
    /// The window a decode from here needs, in bytes; zero at a member start.
    pub fn window_len(&self) -> usize {
        match self.start {
            Start::Member => 0,
            Start::Block { window } => usize::from(window),
        }
    }
}

/// Where a complete file ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct End {
    /// The whole plaintext's length.
    pub plaintext: u64,
    /// The compressed length the members occupy: the file's.
    pub compressed: u64,
}

/// The plaintext's length as far as it is known: exact once a pass has read
/// to the end, open until then.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Size {
    /// The whole plaintext's length.
    Exact(u64),
    /// At least this long; the rest is not yet decoded.
    Open {
        /// The plaintext decoded so far.
        at_least: u64,
    },
}

/// A gzip file's checkpoints, complete or as far as a pass has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    inner: Arc<Inner>,
}

#[derive(Debug, PartialEq, Eq)]
struct Inner {
    spacing: NonZeroU64,
    checkpoints: Vec<Checkpoint>,
    /// For each checkpoint, the checkpoint its member begins at.
    member_of: Vec<u32>,
    /// [`Index::member_count`].
    members: u64,
    end: Option<End>,
}

impl Index {
    /// Rebuild an index from what [`Index::spacing`], [`Index::checkpoints`],
    /// [`Index::member_count`] and [`Index::end`] answered, as a caller that
    /// persisted it hands it back.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidIndex`] where the parts break an invariant every index
    /// this crate builds holds: no checkpoint at the file's start, offsets running
    /// backward, a window other than its member's last 32 KiB, a unit that no
    /// check would cover, an end before the last checkpoint, or a count of
    /// fewer members than the checkpoints begin.
    pub fn from_parts(
        spacing: NonZeroU64,
        checkpoints: Vec<Checkpoint>,
        members: u64,
        end: Option<End>,
    ) -> Result<Index> {
        let invalid = |reason| Error::InvalidIndex { reason };
        let first = checkpoints.first().ok_or(invalid("no checkpoint"))?;
        if *first != (Checkpoint { crc: first.crc, ..Checkpoint::member(0, 0) }) {
            return Err(invalid("the first checkpoint is not a member start at the file's start"));
        }
        let mut member_of = Vec::with_capacity(checkpoints.len());
        let mut member = 0usize;
        for (i, c) in checkpoints.iter().enumerate() {
            if i > 0 {
                let before = &checkpoints[i - 1];
                if c.bit <= before.bit || c.plaintext < before.plaintext {
                    return Err(invalid("checkpoints are not in file order"));
                }
            }
            match c.start {
                Start::Member => {
                    if c.bit % 8 != 0 {
                        return Err(invalid("a member start inside a byte"));
                    }
                    member = i;
                }
                Start::Block { window } => {
                    let history = c.plaintext - checkpoints[member].plaintext;
                    if u64::from(window) != history.min(WINDOW_SIZE as u64) {
                        return Err(invalid("a window that is not its member's last 32 KiB"));
                    }
                }
            }
            member_of.push(u32::try_from(member).map_err(|_| invalid("too many checkpoints"))?);
        }
        let last = checkpoints.last().expect("not empty");
        if let Some(end) = end
            && (end.plaintext < last.plaintext || end.compressed * 8 <= last.bit)
        {
            return Err(invalid("the end is before the last checkpoint"));
        }
        let starts = checkpoints.iter().filter(|c| c.start == Start::Member).count();
        if members < starts as u64 {
            return Err(invalid("the index counts fewer members than its checkpoints begin"));
        }
        let index =
            Index { inner: Arc::new(Inner { spacing, checkpoints, member_of, members, end }) };
        for unit in 0..index.unit_count() {
            let task = index.unit(unit).expect("a counted unit");
            if task.start.crc.is_none()
                && !(task.start.start == Start::Member && task.ends_at_member)
            {
                return Err(invalid("a unit that is not whole members carries no CRC-32"));
            }
        }
        Ok(index)
    }

    /// Built by this crate, so every invariant [`Index::from_parts`] checks
    /// holds; checked anyway under `debug_assertions`.
    pub(crate) fn built(
        spacing: NonZeroU64,
        checkpoints: Vec<Checkpoint>,
        members: u64,
        end: Option<End>,
    ) -> Index {
        if cfg!(debug_assertions) {
            return Index::from_parts(spacing, checkpoints, members, end)
                .expect("an index this crate built");
        }
        let mut member = 0u32;
        let member_of = checkpoints
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if c.start == Start::Member {
                    member = i as u32;
                }
                member
            })
            .collect();
        Index { inner: Arc::new(Inner { spacing, checkpoints, member_of, members, end }) }
    }

    /// The plaintext spacing checkpoints were placed at.
    pub fn spacing(&self) -> NonZeroU64 {
        self.inner.spacing
    }

    /// Every checkpoint, in file order.
    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.inner.checkpoints
    }

    /// Where the file ends, once a pass has reached it.
    pub fn end(&self) -> Option<End> {
        self.inner.end
    }

    /// How many members the index reaches: every member of the file once
    /// complete, and otherwise each beginning at or before the last
    /// checkpoint — a unit of whole members holding several, so the count is
    /// stated rather than read off the checkpoints.
    pub fn member_count(&self) -> u64 {
        self.inner.members
    }

    /// Whether the index covers the whole file.
    pub fn is_complete(&self) -> bool {
        self.inner.end.is_some()
    }

    /// The plaintext's length: exact once complete, otherwise at least to the
    /// last checkpoint.
    pub fn size(&self) -> Size {
        match self.inner.end {
            Some(end) => Size::Exact(end.plaintext),
            None => {
                Size::Open { at_least: self.inner.checkpoints.last().map_or(0, |c| c.plaintext) }
            }
        }
    }

    /// How many units can be decoded: every checkpoint's, but the last's
    /// while the index is incomplete, whose end is not yet known.
    pub fn unit_count(&self) -> usize {
        let n = self.inner.checkpoints.len();
        if self.is_complete() { n } else { n - 1 }
    }

    /// The unit whose plaintext holds offset `plaintext`, `None` past the
    /// decodable units.
    pub fn unit_at(&self, plaintext: u64) -> Option<usize> {
        let checkpoints = &self.inner.checkpoints;
        let unit = checkpoints.partition_point(|c| c.plaintext <= plaintext).checked_sub(1)?;
        let task = self.unit(unit)?;
        (plaintext < task.plaintext_range().end).then_some(unit)
    }

    /// Unit `unit`, as a task one decode can take; `None` past the decodable
    /// units.
    pub fn unit(&self, unit: usize) -> Option<UnitTask> {
        if unit >= self.unit_count() {
            return None;
        }
        let checkpoints = &self.inner.checkpoints;
        let start = checkpoints[unit];
        let member = checkpoints[self.inner.member_of[unit] as usize];
        let (end_plaintext, end_bit, ends_at_member) = match checkpoints.get(unit + 1) {
            Some(next) => (next.plaintext, next.bit, next.start == Start::Member),
            None => {
                let end = self.inner.end.expect("the last unit of a complete index");
                (end.plaintext, end.compressed * 8, true)
            }
        };
        Some(UnitTask {
            unit,
            start,
            end_plaintext,
            end_bit,
            ends_at_member,
            member_plaintext: member.plaintext,
            member_offset: member.bit / 8,
        })
    }

    /// The CRC-32 of the plaintext from the member holding checkpoint
    /// `checkpoint` to that checkpoint, from the units between: what a pass
    /// resumed there compares the member's trailer against. `None` where a
    /// unit between carries none.
    pub(crate) fn member_prefix(&self, checkpoint: usize) -> Option<u32> {
        let checkpoints = &self.inner.checkpoints;
        let member = self.inner.member_of[checkpoint] as usize;
        let mut crc = 0;
        for unit in member..checkpoint {
            let len = checkpoints[unit + 1].plaintext - checkpoints[unit].plaintext;
            crc = crate::zlib::crc32_combine(crc, checkpoints[unit].crc?, len);
        }
        Some(crc)
    }
}

impl Checkpoint {
    pub(crate) fn member(plaintext: u64, offset: u64) -> Checkpoint {
        Checkpoint { plaintext, bit: offset * 8, start: Start::Member, crc: None }
    }
}

/// One unit, as one decode takes it: `Copy` and `Send`, holding no index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitTask {
    pub(crate) unit: usize,
    pub(crate) start: Checkpoint,
    pub(crate) end_plaintext: u64,
    pub(crate) end_bit: u64,
    /// The unit ends where a member does: at the next member's header, or at
    /// the file's end.
    pub(crate) ends_at_member: bool,
    pub(crate) member_plaintext: u64,
    pub(crate) member_offset: u64,
}

impl UnitTask {
    /// Which unit of its index this is.
    pub fn unit(&self) -> usize {
        self.unit
    }

    /// The checkpoint the unit begins at.
    pub fn checkpoint(&self) -> Checkpoint {
        self.start
    }

    /// The plaintext the unit decodes to.
    pub fn plaintext_range(&self) -> Range<u64> {
        self.start.plaintext..self.end_plaintext
    }

    /// The compressed bytes a decode of the whole unit reads: from the byte
    /// holding its first bit to the byte holding its last, the trailers of
    /// the members it ends included.
    pub fn compressed_range(&self) -> Range<u64> {
        let end = if self.ends_at_member { self.end_bit / 8 } else { self.end_bit.div_ceil(8) };
        self.start.bit / 8..end
    }

    /// The window a decode of it needs, in bytes.
    pub fn window_len(&self) -> usize {
        self.start.window_len()
    }
}

#[cfg(test)]
mod tests;
