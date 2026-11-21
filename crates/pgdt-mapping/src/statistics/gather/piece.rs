//! A piece's gatherer: the rows of one piece of a block the leader split,
//! gathered to be joined into the block's in file order
//! (`docs/design/decisions.md`, "D85").

use std::any::Any;
use std::sync::Arc;

use pgdt_core::internal::StatisticsScope;
use pgdt_model::internal::{BlockObserved, BlockObserver};

use super::gatherer::{Closing, Groups, OpenGroup};
use super::read::Reader;

/// **The observer of one piece of a block**, made by
/// [`BlockGatherer`](super::block::BlockGatherer)'s `piece` and handed back
/// to its `absorb`, which recovers this type: it lists no group ahead of
/// its first row's, holds that group open once a row moves past it, merges
/// nothing, and is joined rather than closed.
pub(super) struct PieceGatherer {
    pub(super) reader: Reader,
    /// The piece's first group once a later row has moved past it, with where
    /// its last row ends. **Held open, not closed**: the rows before the piece
    /// may have begun it, and only the block's join knows. Each column's
    /// state over it is its own held head.
    pub(super) head: Option<(OpenGroup, u64)>,
    /// A clone of the block's, so the block merges nothing mid-scan while
    /// this piece is alive; `None` only as it is dropped. Read by nothing:
    /// held for the count.
    #[cfg_attr(not(feature = "introspect"), allow(dead_code))]
    alive: Option<Arc<()>>,
    /// **Last**, its charge released after everything above is freed.
    pub(super) groups: Groups,
}

impl PieceGatherer {
    pub(super) fn new(reader: Reader, groups: Groups, alive: Arc<()>) -> Self {
        Self { reader, head: None, alive: Some(alive), groups }
    }

    /// Whether this piece has stopped gathering rows, having declined or
    /// refused.
    fn stopped(&self) -> bool {
        self.reader.refused.is_some() || self.groups.declined.is_some()
    }
}

/// Under the instrument, a piece's fields are dropped inside a statistics
/// scope, as a block's are ([`BlockGatherer`](super::block::BlockGatherer)).
#[cfg(feature = "introspect")]
impl Drop for PieceGatherer {
    fn drop(&mut self) {
        let _attributed = StatisticsScope::enter();
        self.groups.free_attributed();
        drop(self.alive.take());
        self.reader.free_attributed();
        self.head = None;
    }
}

/// **A piece answers its rows alone**: it is absorbed into the block that
/// made it, never closed, and makes no piece of its own.
impl BlockObserver for PieceGatherer {
    fn holds(&self, column: usize) -> bool {
        self.reader.holds(column)
    }

    fn begin_row(&mut self, offset: u64) {
        self.reader.begin_row(offset);
        if self.stopped() {
            return;
        }
        let _attributed = StatisticsScope::enter();
        self.groups.open_row(offset, Closing::Piece(&mut self.head));
    }

    fn fold(&mut self, column: usize, bytes: &[u8], ends: bool) {
        self.reader.fold(column, bytes, ends, &mut self.groups);
    }

    fn end_row(&mut self, _fields: usize) {
        self.reader.end_row();
        if !self.stopped() {
            self.groups.end_row();
        }
    }

    fn close(self: Box<Self>, _end: u64) -> BlockObserved {
        unreachable!("a piece is absorbed into its block, never closed")
    }

    fn piece(&self) -> Box<dyn BlockObserver> {
        unreachable!("a piece makes no piece")
    }

    fn absorb(&mut self, _later: Box<dyn BlockObserver>) {
        unreachable!("a piece absorbs no piece")
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}
