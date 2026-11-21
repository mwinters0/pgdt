//! A block's gatherer: its pieces and their join, the cap and density
//! merges, and its close (`docs/design/decisions.md`, "D81", "D82", "D85").

use std::any::Any;
use std::mem;
use std::sync::{Arc, OnceLock};

use pgdt_core::internal::StatisticsScope;
use pgdt_model::{
    BlockGathered, BlockStatistics, GroupSizing,
    internal::{BlockObserved, BlockObserver, RowObserver},
};

use super::charge::reserve_charged;
use super::column::ColumnGatherer;
use super::gatherer::{Closing, Groups, Sizing};
use super::piece::PieceGatherer;
use super::read::{Absorbed, Reader};
use super::strict::FieldCheck;
use crate::statistics::account::{Charge, Term};
use crate::statistics::density::density_merges;

/// **The observer of a whole block that gathers statistics**: what
/// [`observer_tracking`](super::observer_tracking) builds, handed the rows
/// the pass reads itself and folding in, in file order, the pieces the
/// leader's workers read ([`PieceGatherer`]).
pub(super) struct BlockGatherer {
    reader: Reader,
    /// The most groups the block holds, `None` under a stated size or
    /// maximum and for a block re-read at the size it holds.
    cap: Option<usize>,
    /// The fewest rows the finished block's median group is merged *towards*,
    /// `None` for an exact size and for a block re-read at the size it holds.
    /// Not a floor: the merging stops at one group, or where the next size
    /// would break `max_rows`, whichever comes first ([`density_merges`]).
    min_rows: Option<u64>,
    /// The most rows a merge here may put in the finished block's
    /// 90th-percentile group; `None` for an exact size, where none is stated,
    /// and for a block re-read at the size it holds. Not a ceiling: a block
    /// already past it at the size it gathered from merges nothing
    /// ([`density_merges`]) and finishes over it, the re-read at the finer
    /// size [`crate::statistics::StatisticsRequest::backfill`] predicts being
    /// what aims it back within one, which a block whose rows cluster can
    /// still miss.
    max_rows: Option<u64>,
    /// What the finished block records it was sized under.
    record: GroupSizing,
    /// Shared with every piece this block has made, once it has made one:
    /// mid-scan a block merges only while no piece holds a clone, so each
    /// piece joins at the size it gathered at. At the block's end the check
    /// is dropped, a piece still alive being past joining
    /// ([`Self::fit_cap`]).
    pieces: OnceLock<Arc<()>>,
    /// **Last**, its charge released after everything above is freed.
    groups: Groups,
}

impl BlockGatherer {
    /// A block gathering `columns` sized by `sizing`, checking every field
    /// by `checks` first where it is given, and charged to `charge` as it is
    /// made.
    pub(super) fn new(
        sizing: Sizing,
        columns: Vec<Option<ColumnGatherer>>,
        checks: Option<Arc<[Option<FieldCheck>]>>,
        charge: Charge,
    ) -> Self {
        let reader = Reader::new(checks, &columns);
        Self {
            reader,
            cap: sizing.cap.map(|cap| cap.max(1)),
            min_rows: sizing.min_rows,
            max_rows: sizing.max_rows,
            record: sizing.record,
            pieces: OnceLock::new(),
            groups: Groups::new(sizing.group_size, columns, None, charge),
        }
    }

    /// Whether this block has stopped gathering rows, having declined or
    /// refused; one that declined still checks them where it checks any.
    fn stopped(&self) -> bool {
        self.reader.refused.is_some() || self.groups.declined.is_some()
    }

    /// Decline where the last charge update found the account over its
    /// allowance.
    pub(super) fn decline_if_over(&mut self) {
        if !self.stopped() {
            self.groups.decline_if_over();
        }
    }

    /// The cap this block merges past now: none under a stated size or
    /// maximum and for a block re-read at the size it holds, and none while
    /// a piece it made is alive.
    pub(super) fn merging_cap(&self) -> Option<usize> {
        let alive = self.pieces.get().is_some_and(|pieces| Arc::strong_count(pieces) > 1);
        self.cap.filter(|_| !alive)
    }

    /// **Merge this block's groups pairwise until they are within its cap.**
    /// Mid-block, a last closed group left without a pair is taken back into
    /// the open group that follows it, which is its pair
    /// ([`Groups::reopen_last`]); `finishing`, no row follows, so it stands
    /// alone, and a piece still alive — past the block's end — joins nothing
    /// more. A block folding a window of pieces can pass its cap by the
    /// window's groups, which those pieces held already, until the last of
    /// them is folded.
    pub(super) fn fit_cap(&mut self, finishing: bool) {
        let cap = if finishing { self.cap } else { self.merging_cap() };
        let Some(cap) = cap else { return };
        let groups = &mut self.groups;
        if groups.groups.len() <= cap {
            return;
        }
        groups.charge_growth();
        while groups.groups.len() > cap {
            if !groups.groups.len().is_multiple_of(2) && !finishing {
                groups.reopen_last();
            }
            groups.merge_pairs();
        }
    }

    /// **Merge a finished block's groups pairwise until its median group holds
    /// its minimum**, or it is one group, or the next merge would put more
    /// than its maximum in its 90th-percentile group — the smallest size, no
    /// finer than the one it holds, at which the upper middle group reaches
    /// the minimum, and never past the coarsest at which the maximum holds
    /// ([`density_merges`]). Run after [`Self::fit_cap`], from the size the
    /// cap left: the finer sizes' rows are summed as the cap merges, and the
    /// predicate is monotone in size, so the size the cap left is never
    /// coarsened past one the minimum would have chosen for itself.
    fn fit_density(&mut self) {
        let Some(min_rows) = self.min_rows else { return };
        let rows: Vec<u64> = self.groups.groups.iter().map(|group| group.rows).collect();
        for _ in 0..density_merges(rows, min_rows, self.max_rows) {
            self.groups.merge_pairs();
        }
    }

    /// Fold `later`, a piece whose rows all follow this block's in file
    /// order, into it — leaving it as it would be had it observed those rows
    /// itself. Its first group continues this one's open group where both hold
    /// the same index, and each ordered column steps from this block's last
    /// value to the piece's first.
    ///
    /// **The piece's charge moves onto this block's in one update**, and from
    /// there this block charges what the piece still holds beside what it
    /// holds itself, as each part moves across or is freed: the account never
    /// reads the moved groups as held by neither or by both. What is left of
    /// the piece goes back to its own charge in one update, released as the
    /// piece is freed. Pieces are folded with none of them gathering
    /// (`crate::interior`), so no other observer updates the account while
    /// one is half moved.
    fn join(&mut self, mut later: PieceGatherer) {
        self.groups.take_over(&mut later.groups);
        self.join_rows(&mut later);
        self.groups.hand_back(&mut later.groups);
        drop(later);
        self.groups.charge_held();
    }

    /// [`Self::join`] but for moving the charge, draining `later` in place so
    /// that what it has left is what [`Groups::held`] reads of it.
    fn join_rows(&mut self, later: &mut PieceGatherer) {
        let (first, first_end) = match (later.head.take(), later.groups.open.take()) {
            (Some((group, end)), open) => {
                later.groups.open = open;
                (group, Some(end))
            }
            (None, Some(group)) => (group, None),
            // The piece held no row.
            (None, None) => return,
        };
        let continues = self.groups.open.as_ref().is_some_and(|open| open.index == first.index);
        match &mut self.groups.open {
            Some(open) if continues => open.rows += first.rows,
            _ => {
                let cap = self.merging_cap();
                self.groups.close_through(first.first_start, first.index, Closing::Block(cap));
                self.groups.open = Some(first);
            }
        }
        let (mine, theirs) = (&mut self.groups, &mut later.groups);
        for (mine, theirs) in mine.columns.iter_mut().zip(&mut theirs.columns) {
            let (Some(mine), Some(theirs)) = (mine, theirs) else { continue };
            mine.join_first(theirs, first_end.is_some(), continues);
        }
        mine.carry(theirs.held());
        // The piece moved past its first group, so the group closes where the
        // piece's next row starts and the piece's own closed groups follow.
        let Some(end) = first_end else { return };
        let open = mine.open.take().expect("the joined group is open");
        mine.push_closed(open, end);
        reserve_charged(&mut mine.groups, theirs.groups.len(), &mut mine.charge);
        mine.groups.extend_from_slice(&theirs.groups);
        theirs.groups = Vec::new();
        mine.open = theirs.open.take();
        mine.carry(theirs.held());
        for at in 0..mine.columns.len() {
            let later = theirs.columns.get_mut(at).and_then(Option::as_mut);
            if let (Some(column), Some(later)) = (mine.columns[at].as_mut(), later) {
                column.append(later, &mut mine.charge);
            }
        }
        mine.carry(theirs.held());
    }

    /// What the block gathered, declined or refused, its groups closed and
    /// merged to its cap and its density.
    fn finish(&mut self, end: u64) -> BlockGathered {
        if let Some(refusal) = self.reader.refused.take() {
            return BlockGathered::Refused(refusal);
        }
        if let Some(allowance) = self.groups.declined {
            return BlockGathered::Declined { allowance };
        }
        if let Some(group) = &self.groups.open {
            let next = group.index + 1;
            let cap = self.merging_cap();
            self.groups.close_through(end, next, Closing::Block(cap));
        }
        self.fit_cap(true);
        self.fit_density();
        let groups = &mut self.groups;
        let columns = mem::take(&mut groups.columns);
        let statistics = BlockStatistics {
            group_size: groups.group_size,
            sizing: self.record,
            groups: mem::take(&mut groups.groups),
            columns: columns.into_iter().map(|c| c.map(ColumnGatherer::finish)).collect(),
        };
        groups.charge.retain(statistics.heap_bytes());
        BlockGathered::Gathered(statistics)
    }
}

/// Under the instrument, a block's fields are dropped inside a statistics
/// scope, so what it frees is attributed as what it allocated was
/// (`pgdt_core::instrument`); its charge, declared last, is released after.
#[cfg(feature = "introspect")]
impl Drop for BlockGatherer {
    fn drop(&mut self) {
        let _attributed = StatisticsScope::enter();
        self.groups.free_attributed();
        drop(self.pieces.take());
        self.reader.free_attributed();
    }
}

impl RowObserver for BlockGatherer {
    fn holds(&self, column: usize) -> bool {
        self.reader.holds(column)
    }

    fn begin_row(&mut self, offset: u64) {
        self.reader.begin_row(offset);
        // **Each field is checked before it is gathered, and whether or not
        // gathering has declined**: a strict parse reads every field of
        // every row.
        if self.stopped() {
            return;
        }
        let _attributed = StatisticsScope::enter();
        let cap = self.merging_cap();
        if self.groups.open_row(offset, Closing::Block(cap)) {
            self.fit_cap(false);
        }
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

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl BlockObserver for BlockGatherer {
    /// **The block's charge becomes its retained bytes** once the interning
    /// maps are freed, in one update of the account — and a block that
    /// declined answers the allowance it declined under, holding nothing to
    /// retain.
    fn close(mut self: Box<Self>, end: u64) -> BlockObserved {
        let _attributed = StatisticsScope::enter();
        let ignored_refusals = self.reader.take_ignored();
        let checked_in_full = self.reader.checks_every_field();
        let gathered = self.finish(end);
        BlockObserved { gathered, ignored_refusals, checked_in_full }
    }

    fn piece(&self) -> Box<dyn RowObserver> {
        let _attributed = StatisticsScope::enter();
        let columns = self.groups.columns.iter().map(|c| c.as_ref().map(ColumnGatherer::fresh));
        let charge = Charge::new(Arc::clone(self.groups.charge.account()), Term::Pieces);
        // A piece of a block that has already stopped gathers nothing: it
        // still reads its rows for the census and the block's extent, and
        // holds no statistic while it does.
        let groups =
            Groups::new(self.groups.group_size, columns.collect(), self.groups.declined, charge);
        let alive = Arc::clone(self.pieces.get_or_init(Arc::default));
        Box::new(PieceGatherer::new(self.reader.piece(), groups, alive))
    }

    /// **A declined piece declines its block**, the block having lost that
    /// piece's rows and being unable to state statistics over the rest
    /// (`docs/design/decisions.md`, "D85"); a block that declined while the
    /// window ran drops every piece it is handed instead of folding it.
    ///
    /// **A refusal outranks a decline, and the earlier row's outranks a later
    /// one's** ([`Reader::absorb`]). Every other piece's rows are counted,
    /// folded or dropped.
    fn absorb(&mut self, later: Box<dyn RowObserver>) {
        let _attributed = StatisticsScope::enter();
        let later = later.into_any().downcast::<PieceGatherer>().expect("a piece of this block");
        let mut later = *later;
        match self.reader.absorb(&mut later.reader) {
            Absorbed::Dropped => drop(later),
            Absorbed::Refused(refusal) => {
                drop(later);
                self.reader.refuse(refusal, &mut self.groups);
            }
            Absorbed::Rows => match (self.groups.declined, later.groups.declined) {
                (Some(_), _) => drop(later),
                (None, Some(allowance)) => {
                    drop(later);
                    self.groups.decline(allowance);
                }
                (None, None) => {
                    self.join(later);
                    self.fit_cap(false);
                    self.decline_if_over();
                }
            },
        }
    }
}
