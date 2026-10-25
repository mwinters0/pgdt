//! Where a first pass places its checkpoints: the placement rule, applied to
//! the events a decode reports in file order — whichever decode reported
//! them, so that the serial pass and the parallel one build one index.
//!
//! # Where a checkpoint goes is a function of the file alone
//!
//! A checkpoint is placed at **every member start** but a BGZF member's
//! joining a unit (below), and at **the first place
//! a decode can resume at or after each multiple of the spacing** in
//! plaintext: a deflate block boundary, a member start counting as one, or
//! inside a static block, before any symbol but its end-of-block code — so the same file and spacing give the
//! same index however the caller cuts its buffers, wherever an earlier pass was
//! interrupted, and whichever decode, on whichever thread, read each piece
//! (`docs/design/roadmap-P15-gzip.md`, "Settled in the grilling"). A decode
//! reports every block's end, and the rule checks each against the next
//! multiple; inside a static block the decode walks to the mark
//! ([`Placement::mark`]) and stops at the first symbol there, so a writer
//! that ends no block across a spacing — `deflate_quick` writes one static
//! block — is checkpointed as any other. At a plaintext offset holding both, a
//! block boundary comes first, an end-of-block code being no symbol a
//! checkpoint precedes, so a symbol's checkpoint never leaves a unit empty
//! (`docs/design/decisions.md`, "D118"). What the spacing moves is how many
//! windows are copied, never how often a decode stops at a boundary.
//!
//! **BGZF members are coalesced as a [`Walk`](crate::Walk) coalesces them.** A
//! member start whose own header is a BGZF member's joins the unit before it
//! where that unit began at a BGZF member start and is short of the spacing; no
//! block boundary inside a BGZF member is a checkpoint; and a unit of whole
//! BGZF members records no CRC-32, its trailers checking it. So a pass over
//! a BGZF file builds the index its walk reads off the headers, and any run of
//! BGZF members in another gzip file is coalesced alike, while a member whose
//! header is not BGZF's begins a unit and takes the rule above. Each member is
//! decided by its own header and the unit it would join, so a pass resumed at
//! a member start reads the rule off that header alone
//! (`docs/design/decisions.md`, "D116").
//!
//! What a unit's CRC-32 is, and each checkpoint's window, are the decode's to
//! supply: this module says only where the checkpoints go and which units
//! record a CRC-32.

use std::num::NonZeroU64;

use crate::engine::Member;
use crate::error::{Error, Result};
use crate::index::{Checkpoint, End, Index, Start};

/// The placement rule's state over one pass.
pub(crate) struct Placement {
    spacing: NonZeroU64,
    /// The next multiple of the spacing a checkpoint is owed at.
    next_mark: u64,
    checkpoints: Vec<Checkpoint>,
    /// Whether the member being read has a BGZF member's header: no block
    /// boundary inside one is a checkpoint, and its unit records no CRC-32.
    member_bgzf: bool,
    /// Whether the unit since the last checkpoint began at a BGZF member's
    /// start, so that a later BGZF member may join it.
    unit_bgzf: bool,
    /// A member start the spacing does not owe, placed only if its header
    /// proves not to be a BGZF member's.
    deferred: Option<Checkpoint>,
    /// How many members have begun.
    members: u64,
    /// How many members had begun when the last checkpoint was placed.
    members_at_last: u64,
    /// The member start the pass begins at and has yet to place, with the
    /// member count of the index it stands for: what [`Placement::index`]
    /// answers until it is placed.
    replacing: Option<(Checkpoint, u64)>,
}

/// Where a resumed pass's decode begins.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Resumed {
    /// At a member's header, its start to be reported again.
    Member { offset: u64, plaintext: u64 },
    /// Inside `member`'s deflate data, at a block's start.
    Block { bit: u64, plaintext: u64, member: Member },
    /// Inside `member`'s static block, before a symbol; `last` whether the
    /// block is the member's final one.
    Symbol { bit: u64, last: bool, plaintext: u64, member: Member },
}

impl Placement {
    /// The rule over a pass from the file's first byte.
    pub(crate) fn new(spacing: NonZeroU64) -> Placement {
        Placement {
            spacing,
            next_mark: 0,
            checkpoints: Vec::new(),
            member_bgzf: false,
            unit_bgzf: false,
            deferred: None,
            members: 0,
            members_at_last: 0,
            replacing: Some((Checkpoint::member(0, 0), 1)),
        }
    }

    /// The rule over a pass resumed at the last checkpoint of an incomplete
    /// `index` whose window is `window`, and where its decode begins.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidIndex`] for a complete index, or one whose member
    /// holding the last checkpoint has a unit with no CRC-32 — one no forward
    /// pass built; [`Error::WindowLength`] for a window of another length.
    pub(crate) fn resume(index: &Index, window: &[u8]) -> Result<(Placement, Resumed)> {
        if index.is_complete() {
            return Err(Error::InvalidIndex { reason: "a complete index has nothing to resume" });
        }
        let checkpoints = index.checkpoints().to_vec();
        let at = checkpoints.len() - 1;
        let last = checkpoints[at];
        if window.len() != last.window_len() {
            return Err(Error::WindowLength { expected: last.window_len(), given: window.len() });
        }
        let resumed = match last.start {
            Start::Member => Resumed::Member { offset: last.bit / 8, plaintext: last.plaintext },
            Start::Block { .. } | Start::Symbol { .. } => {
                let prefix = index.member_prefix(at).ok_or(Error::InvalidIndex {
                    reason: "a unit before the resume point carries no CRC-32",
                })?;
                let member_at = checkpoints[..=at]
                    .iter()
                    .rposition(|c| c.start == Start::Member)
                    .expect("the first checkpoint is a member start");
                let member = checkpoints[member_at];
                let member =
                    Member::resumed(member.bit / 8, member.plaintext, Some(prefix), last.plaintext);
                let (bit, plaintext) = (last.bit, last.plaintext);
                match last.start {
                    Start::Symbol { last, .. } => Resumed::Symbol { bit, last, plaintext, member },
                    _ => Resumed::Block { bit, plaintext, member },
                }
            }
        };
        let members = index.member_count();
        let mut placement = Placement {
            spacing: index.spacing(),
            next_mark: 0,
            checkpoints,
            // A checkpoint inside a member is one only a plain member has; a
            // member start is placed again, its header read again.
            member_bgzf: false,
            unit_bgzf: false,
            deferred: None,
            members,
            members_at_last: members,
            replacing: None,
        };
        if last.start == Start::Member {
            // The member's start is placed again, and its member counted
            // again, as the pass reads its header.
            placement.checkpoints.pop();
            placement.members -= 1;
            placement.replacing = Some((last, members));
        } else {
            placement.checkpoints[at].crc = None;
            placement.marked(last.plaintext);
        }
        Ok((placement, resumed))
    }

    /// A member begins at `plaintext`, its header at compressed `offset`: the
    /// checkpoint placed there, unless the unit before may still take it in.
    pub(crate) fn member_start(&mut self, plaintext: u64, offset: u64) -> Option<Checkpoint> {
        self.members += 1;
        self.member_bgzf = false;
        let start = Checkpoint::member(plaintext, offset);
        if self.owes_member(plaintext) {
            Some(start)
        } else {
            self.deferred = Some(start);
            None
        }
    }

    /// The member's header has been read: a start that was deferred is placed
    /// unless the header is a BGZF member's.
    ///
    /// A start is deferred only into a unit a BGZF member began, so the unit
    /// this member is in now began at one exactly where its own header is
    /// BGZF's: joined to it, or placed at its own start.
    pub(crate) fn header(&mut self, bgzf: bool) -> Option<Checkpoint> {
        self.member_bgzf = bgzf;
        self.unit_bgzf = bgzf;
        let deferred = self.deferred.take();
        if bgzf { None } else { deferred }
    }

    /// The plaintext spacing checkpoints are placed at.
    pub(crate) fn spacing(&self) -> NonZeroU64 {
        self.spacing
    }

    /// The plaintext offset a static block's walk stops at the first symbol
    /// at or past: the next multiple owed, `None` inside a BGZF member, where
    /// no place but a member start is a checkpoint.
    pub(crate) fn mark(&self) -> Option<u64> {
        (!self.member_bgzf).then_some(self.next_mark)
    }

    /// Whether a block boundary, or a static block's symbol, at `plaintext`
    /// is a checkpoint.
    pub(crate) fn owes_block(&self, plaintext: u64) -> bool {
        !self.member_bgzf && plaintext >= self.next_mark
    }

    /// Whether the unit open when the member ends records a CRC-32: one of
    /// whole BGZF members is checked by their trailers.
    pub(crate) fn member_end_records_crc(&self) -> bool {
        !self.member_bgzf
    }

    /// The unit since the last checkpoint ends here with CRC-32 `crc`.
    pub(crate) fn close(&mut self, crc: u32) {
        self.checkpoints.last_mut().expect("a member began").crc = Some(crc);
    }

    /// Place `checkpoint`, which an event above answered.
    pub(crate) fn place(&mut self, checkpoint: Checkpoint) {
        let plaintext = checkpoint.plaintext;
        self.checkpoints.push(checkpoint);
        self.members_at_last = self.members;
        self.replacing = None;
        self.marked(plaintext);
    }

    /// The index as far as the pass has got: its last checkpoint's unit open.
    pub(crate) fn index(&self) -> Index {
        let mut checkpoints = self.checkpoints.clone();
        let members = match self.replacing {
            Some((start, members)) => {
                checkpoints.push(start);
                members
            }
            None => self.members_at_last,
        };
        if let Some(last) = checkpoints.last_mut() {
            last.crc = None;
        }
        Index::built(self.spacing, checkpoints, members, None)
    }

    /// Whether a member start the pass began at is still to be placed: a pass
    /// ending there read nothing of it.
    pub(crate) fn is_replacing(&self) -> bool {
        self.replacing.is_some()
    }

    /// The complete index, the file ending at `end`.
    pub(crate) fn finish(self, end: End) -> Index {
        Index::built(self.spacing, self.checkpoints, self.members, Some(end))
    }

    /// Whether a member beginning at `plaintext` begins a unit before its
    /// header is read: always, unless the unit since the last checkpoint
    /// began at a BGZF member's start and is short of the spacing.
    fn owes_member(&self, plaintext: u64) -> bool {
        !self.unit_bgzf
            || self
                .checkpoints
                .last()
                .is_none_or(|last| plaintext - last.plaintext >= self.spacing.get())
    }

    /// A checkpoint was placed at `plaintext`: the next is owed at the next
    /// multiple of the spacing past it.
    fn marked(&mut self, plaintext: u64) {
        let spacing = self.spacing.get();
        self.next_mark = (plaintext / spacing + 1) * spacing;
    }
}
