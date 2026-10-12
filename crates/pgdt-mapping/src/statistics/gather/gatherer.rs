//! The block's observer: its groups, its pieces and their join, the cap and
//! density merges, and the decline (`docs/design/decisions.md`, "D81",
//! "D82", "D85").

use std::any::Any;
use std::borrow::Cow;
use std::mem::{self};
use std::sync::{Arc, OnceLock};

use pgdt_core::internal::StatisticsScope;
use pgdt_core::{decode_field, split_fields};
use pgdt_model::internal::IgnoredRefusalsExt;
use pgdt_model::{
    BlockGathered, BlockStatistics, FieldRefusal, GroupSizing, IgnoredRefusals, RowGroup,
    internal::{BlockObserver, vec_heap},
};

use super::bounds::{Refused, merge_adjacent};
use super::charge::{push_charged, reserve_charged};
use super::column::{ColumnGatherer, Observed};
use super::strict::{FieldCheck, check_row};
use crate::statistics::account::{Charge, STATISTICS_ACCOUNT_CHARGE_STEP, Term};
use crate::statistics::density::density_merges;
use crate::statistics::request::StatisticsBackfill;

/// The group a row is being added to.
struct OpenGroup {
    index: u64,
    first_start: u64,
    rows: u64,
}

/// How a block observer sizes its groups: a [`StatisticsBackfill`] less its
/// columns.
#[derive(Clone, Copy)]
pub(super) struct Sizing {
    pub(super) group_size: u64,
    pub(super) cap: Option<usize>,
    pub(super) min_rows: Option<u64>,
    pub(super) max_rows: Option<u64>,
    pub(super) record: GroupSizing,
}

impl Sizing {
    pub(super) fn of(plan: &StatisticsBackfill) -> Self {
        let (group_size, cap) = (plan.group_size, plan.group_cap);
        let (min_rows, max_rows) = (plan.min_rows, plan.max_rows);
        Self { group_size, cap, min_rows, max_rows, record: plan.sizing }
    }
}

pub(super) struct Gatherer {
    group_size: u64,
    /// The most groups a block holds, `None` under a stated size or maximum
    /// and for a piece, which never merges.
    cap: Option<usize>,
    /// The fewest rows the finished block's median group is merged *towards*,
    /// `None` for an exact size and for a piece, which is never finished. Not
    /// a floor: the merging stops at one group, or where the next size would
    /// break `max_rows`, whichever comes first ([`density_merges`]).
    min_rows: Option<u64>,
    /// The most rows a merge here may put in the finished block's
    /// 90th-percentile group; `None` for an exact size and for a piece. Not a
    /// ceiling: a block already past it at the size it gathered from merges
    /// nothing, where its groups pair evenly ([`density_merges`]), and
    /// finishes over it, the re-read in
    /// [`crate::statistics::StatisticsRequest::backfill`] being what brings it
    /// back within one.
    max_rows: Option<u64>,
    /// What the finished block records it was sized under.
    record: GroupSizing,
    /// Shared with every piece this block has made, once it has made one:
    /// mid-scan a block merges only while no piece holds a clone, so each
    /// piece joins at the size it gathered at. At the block's end the check
    /// is dropped, a piece still alive being past joining
    /// ([`Gatherer::fit_cap`]).
    pieces: OnceLock<Arc<()>>,
    /// Whether this observes a piece of its block rather than the whole of it:
    /// a piece lists no group ahead of its first row's, holds that group open
    /// once a row moves past it, and is joined rather than finished.
    piece: bool,
    /// Every closed group in order, from the block's first — or, in a piece,
    /// from the one after [`Self::head`].
    groups: Vec<RowGroup>,
    /// A piece's first group once a later row has moved past it, with where
    /// its last row ends. **Held open, not closed**: the rows before the piece
    /// may have begun it, and only [`Gatherer::join`] knows. Each column's
    /// state over it is [`ColumnGatherer::head`].
    head: Option<(OpenGroup, u64)>,
    open: Option<OpenGroup>,
    /// Whether any column is tracked, so a row is worth splitting.
    splits: bool,
    /// Whether this observer gathers statistics at all, which a [`checker`](super::strict::checker)
    /// does not: it opens no group, never declines, and answers
    /// [`BlockGathered::Checked`].
    pub(super) gathers: bool,
    columns: Vec<Option<ColumnGatherer>>,
    /// What a strict parse checks every field by, one per column, where this
    /// observer checks them ([`PostgresInvalidValues::Strict`](pgdt_model::PostgresInvalidValues::Strict)): each row is
    /// checked before it is gathered, and after a decline too.
    pub(super) checks: Option<Arc<[Option<FieldCheck>]>>,
    /// What the open groups have grown by since [`Self::charge_held`] last
    /// ran, which it runs again once this passes [`STATISTICS_ACCOUNT_CHARGE_STEP`].
    uncharged: i64,
    /// Whether the account stood over its allowance at the last charge update
    /// — read at a row's end and at a fold's, never mid-merge, so an observer
    /// declines only where its structures are whole.
    over: bool,
    /// What a piece [`Self::join`] is folding in still holds, which this
    /// observer's charge carries until the fold ends.
    carried: (u64, u64),
    /// The allowance this observer **declined** under, once the account passed
    /// it: everything gathered is freed, no further row is gathered, and a block
    /// answers [`BlockGathered::Declined`]
    /// (`docs/design/decisions.md`, "D85"). A piece carries it into the block
    /// it folds into, the block having lost that piece's rows.
    declined: Option<u64>,
    /// The first field this observer met that its type's `*_in` refuses: it
    /// frees what it holds and reads no further row, as a declined observer
    /// does, and a block answers [`BlockGathered::Refused`]. A piece carries
    /// it into the block it folds into.
    refused: Option<FieldRefusal>,
    /// The fields its type's `*_in` refuses that this observer keyed and went
    /// past, told to ignore them. A piece's is folded into its block's, its
    /// first numbered past the block's rows, as a refusal is.
    ignored: Option<IgnoredRefusals>,
    /// The rows this observer has been handed, and, in a block, those of
    /// every piece it has folded in, **counted whether or not it has
    /// stopped**: a piece made before a decline can still refuse, and its
    /// line is numbered past every row before it ([`FieldRefusal::line`]).
    rows: u64,
    /// What this observer holds, in the pass's account. **Last**, so it is
    /// released after everything above is freed ([`Charge`]).
    charge: Charge,
}

impl Gatherer {
    pub(super) fn block(
        sizing: Sizing,
        columns: Vec<Option<ColumnGatherer>>,
        charge: Charge,
    ) -> Self {
        Self {
            group_size: sizing.group_size,
            cap: sizing.cap.map(|cap| cap.max(1)),
            min_rows: sizing.min_rows,
            max_rows: sizing.max_rows,
            record: sizing.record,
            pieces: OnceLock::new(),
            piece: false,
            groups: Vec::new(),
            head: None,
            open: None,
            splits: columns.iter().any(Option::is_some),
            gathers: true,
            columns,
            checks: None,
            uncharged: 0,
            over: false,
            carried: (0, 0),
            declined: None,
            refused: None,
            ignored: None,
            rows: 0,
            charge,
        }
    }

    /// Whether this observer has stopped gathering rows, having declined or
    /// refused; one that declined still checks them where it checks any.
    fn stopped(&self) -> bool {
        self.declined.is_some() || self.refused.is_some()
    }

    /// Free everything this observer holds, its charge going to nothing in
    /// the same update: what [`Self::decline`] and [`Self::refuse`] share.
    fn free(&mut self) {
        debug_assert_eq!(self.carried, (0, 0), "a fold is not a place to stop");
        drop(mem::take(&mut self.columns));
        drop(mem::take(&mut self.groups));
        (self.head, self.open, self.splits) = (None, None, false);
        self.uncharged = 0;
        self.charge.set(0, 0);
    }

    /// **Stop at a field PostgreSQL refuses**: the pass fails on it, so what
    /// was gathered is freed rather than finished, and no later row is read
    /// for statistics (`roadmap.md`, "A literal is guaranteed in `*_out`'s
    /// form and never read past `*_in`'s").
    fn refuse(&mut self, refusal: FieldRefusal) {
        self.refused = Some(refusal);
        self.free();
    }

    /// **Drop everything this observer holds and gather no more**, the account
    /// having passed its allowance: the block's groups and columns are freed,
    /// its charge goes to nothing in the same update, and the allowance it
    /// declined under is what the map records
    /// (`pgdt_model::CopyBlock::statistics_declined`). Called with the
    /// caller's [`StatisticsScope`] open, so what it frees is attributed as
    /// what allocated it was.
    ///
    /// **Nothing else declines with it**: the scan goes on, and a piece still
    /// gathering is freed as it is folded in or dropped
    /// (`docs/design/decisions.md`, "D85").
    ///
    /// deficiency: KD33 — the account this tests is cumulative, and nothing
    /// releases `Term::Retained` while the pass gathers forward (the back-fill
    /// does, replacing a block's statistics, `crate::backfill::backfill_statistics`), so
    /// once a long dump's retained statistics reach the allowance every block
    /// from there on declines on its first charge update. Statistics are then a *prefix* of
    /// the file rather than a sample of it, and a query prunes nothing over
    /// the tail; how much is covered depends on the allowance the box
    /// resolved. Coarsening under that pressure is refused by "D85" — a cache
    /// would depend on its container — so the remedy owned by P23 is a
    /// granularity derived from the dump's length, which is known up front and
    /// is the same on every machine.
    fn decline(&mut self, allowance: u64) {
        self.declined = Some(allowance);
        self.free();
    }

    /// Decline where the last charge update found the account over its
    /// allowance, which [`Self::charge_held`] records.
    pub(super) fn decline_if_over(&mut self) {
        if self.over && !self.stopped() && self.gathers {
            let allowance = self.charge.allowance().expect("only an allowance can be passed");
            self.decline(allowance);
        }
    }

    /// The heap this observer holds, as its structure and its interning maps.
    fn held(&self) -> (u64, u64) {
        let own = vec_heap(&self.groups) + vec_heap(&self.columns);
        self.columns
            .iter()
            .flatten()
            .map(ColumnGatherer::held)
            .fold((own, 0), |(structure, interned), (s, i)| (structure + s, interned + i))
    }

    /// Charge what this observer holds now to the account, with what a piece
    /// being folded in still holds.
    pub(super) fn charge_held(&mut self) {
        let (structure, interned) = self.held();
        self.over = self.charge.set(structure + self.carried.0, interned + self.carried.1);
        self.uncharged = 0;
    }

    /// Close the open group, its last row's line ending at `end`, and list an
    /// empty group for every index short of `next`. **The rows' growth is
    /// charged first**, since each column's close moves the charge by its own
    /// difference alone.
    ///
    /// **A block past its cap merges as the groups close**, wherever their
    /// count is past the cap and even and so every pair is whole, `next`
    /// halving with them — so a long row lists at most one group past the
    /// cap, except while a piece this block made is alive, when nothing merges
    /// before [`Gatherer::fit_cap`].
    fn close_through(&mut self, end: u64, next: u64) {
        if self.uncharged != 0 {
            self.charge_held();
        }
        let mut next = next;
        let mut at = match self.open.take() {
            Some(group) => {
                let mut after = group.index + 1;
                if self.piece && self.head.is_none() {
                    for column in self.columns.iter_mut().flatten() {
                        column.hold_head();
                    }
                    self.head = Some((group, end));
                } else {
                    self.push_closed(group, end);
                    self.merge_whole_pairs(&mut after, &mut next);
                }
                after
            }
            // A piece before its first row lists nothing ahead of it.
            None if self.piece => next,
            None => 0,
        };
        while at < next {
            push_charged(&mut self.groups, RowGroup { rows: 0, bytes: 0 }, &mut self.charge);
            for column in self.columns.iter_mut().flatten() {
                column.close_group(&mut self.charge);
            }
            at += 1;
            self.merge_whole_pairs(&mut at, &mut next);
        }
        self.charge_held();
    }

    /// The cap this block merges past now: none for a piece or under a stated
    /// size or maximum,
    /// and none while a piece it made is alive.
    fn merging_cap(&self) -> Option<usize> {
        let alive = self.pieces.get().is_some_and(|pieces| Arc::strong_count(pieces) > 1);
        self.cap.filter(|_| !alive)
    }

    /// Merge the closed groups pairwise if they are past the cap and even in
    /// number, halving `at` and `next` — group indices — with them.
    fn merge_whole_pairs(&mut self, at: &mut u64, next: &mut u64) {
        let Some(cap) = self.merging_cap() else { return };
        let closed = self.groups.len();
        if closed > cap && closed.is_multiple_of(2) {
            self.merge_pairs();
            (*at, *next) = (*at / 2, *next / 2);
        }
    }

    /// **Merge this block's groups pairwise until they are within its cap.**
    /// Mid-block, a last closed group left without a pair is taken back into
    /// the open group that follows it, which is its pair ([`Self::reopen_last`]);
    /// `finishing`, no row follows, so it stands alone, and a piece still
    /// alive — past the block's end — joins nothing more. A block folding a
    /// window of pieces can pass its cap by the window's groups, which those
    /// pieces held already, until the last of them is folded.
    fn fit_cap(&mut self, finishing: bool) {
        let cap = if finishing { self.cap } else { self.merging_cap() };
        let Some(cap) = cap else { return };
        if self.groups.len() <= cap {
            return;
        }
        if self.uncharged != 0 {
            self.charge_held();
        }
        while self.groups.len() > cap {
            if !self.groups.len().is_multiple_of(2) && !finishing {
                self.reopen_last();
            }
            self.merge_pairs();
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
        let rows: Vec<u64> = self.groups.iter().map(|group| group.rows).collect();
        for _ in 0..density_merges(rows, min_rows, self.max_rows) {
            self.merge_pairs();
        }
    }

    /// Take the last closed group back into the open group following it, as
    /// though both groups' rows had been observed into one.
    fn reopen_last(&mut self) {
        let closed = self.groups.pop().expect("a block past its cap holds closed groups");
        let open = self.open.as_mut().expect("a block mid-scan holds an open group");
        debug_assert_eq!(open.index, self.groups.len() as u64 + 1, "the open group follows");
        open.index -= 1;
        // The closed group's bytes run from its first row to the open group's.
        open.first_start -= closed.bytes;
        open.rows += closed.rows;
        for column in self.columns.iter_mut().flatten() {
            column.reopen_last(&mut self.charge);
        }
    }

    /// Merge every closed group with the one after it, a last one alone, and
    /// double the group size: what gathering at twice the size would hold.
    fn merge_pairs(&mut self) {
        merge_adjacent(&mut self.groups, |a, b| RowGroup {
            rows: a.rows + b.rows,
            bytes: a.bytes + b.bytes,
        });
        for column in self.columns.iter_mut().flatten() {
            column.merge_pairs(&mut self.charge);
        }
        self.group_size *= 2;
        if let Some(open) = &mut self.open {
            debug_assert_eq!(open.index % 2, 0, "an open group's pair is still to come");
            open.index /= 2;
        }
        self.charge_held();
    }

    fn push_closed(&mut self, group: OpenGroup, end: u64) {
        let closed = RowGroup { rows: group.rows, bytes: end - group.first_start };
        push_charged(&mut self.groups, closed, &mut self.charge);
        for column in self.columns.iter_mut().flatten() {
            column.close_group(&mut self.charge);
        }
    }

    /// Fold `later`, a piece whose rows all follow this observer's in file
    /// order, into it — leaving it as it would be had it observed those rows
    /// itself. Its first group continues this one's open group where both hold
    /// the same index, and each ordered column steps from this observer's last
    /// value to the piece's first.
    ///
    /// **The piece's charge moves onto this observer's in one update**, and
    /// from there this observer charges what the piece still holds beside
    /// what it holds itself, as each part moves across or is freed: the
    /// account never reads the moved groups as held by neither or by both.
    /// What is left of the piece goes back to its own charge in one update,
    /// released as the piece is freed.
    /// Pieces are folded with none of them gathering (`crate::interior`), so no
    /// other observer updates the account while one is half moved.
    fn join(&mut self, mut later: Gatherer) {
        debug_assert!(!self.piece && later.piece, "a block observer joins its pieces");
        later.charge_held();
        self.charge.take_over(&mut later.charge);
        self.carried = later.held();
        self.charge_held();
        self.join_rows(&mut later);
        self.charge.hand_back(&mut later.charge, mem::take(&mut self.carried));
        drop(later);
        self.charge_held();
    }

    /// [`Self::join`] but for moving the charge, draining `later` in place so
    /// that what it has left is what [`Self::held`] reads of it.
    fn join_rows(&mut self, later: &mut Gatherer) {
        let (first, first_end) = match (later.head.take(), later.open.take()) {
            (Some((group, end)), open) => {
                later.open = open;
                (group, Some(end))
            }
            (None, Some(group)) => (group, None),
            // The piece held no row.
            (None, None) => return,
        };
        let continues = self.open.as_ref().is_some_and(|open| open.index == first.index);
        match &mut self.open {
            Some(open) if continues => open.rows += first.rows,
            _ => {
                self.close_through(first.first_start, first.index);
                self.open = Some(first);
            }
        }
        for (mine, theirs) in self.columns.iter_mut().zip(&mut later.columns) {
            let (Some(mine), Some(theirs)) = (mine, theirs) else { continue };
            mine.join_first(theirs, first_end.is_some(), continues);
        }
        self.carried = later.held();
        self.charge_held();
        // The piece moved past its first group, so the group closes where the
        // piece's next row starts and the piece's own closed groups follow.
        let Some(end) = first_end else { return };
        let open = self.open.take().expect("the joined group is open");
        self.push_closed(open, end);
        reserve_charged(&mut self.groups, later.groups.len(), &mut self.charge);
        self.groups.extend_from_slice(&later.groups);
        later.groups = Vec::new();
        self.open = later.open.take();
        self.carried = later.held();
        self.charge_held();
        for at in 0..self.columns.len() {
            let theirs = later.columns.get_mut(at).and_then(Option::as_mut);
            if let (Some(mine), Some(theirs)) = (self.columns[at].as_mut(), theirs) {
                mine.append(theirs, &mut self.charge);
            }
        }
        self.carried = later.held();
        self.charge_held();
    }
}

/// Under the instrument, an observer's fields are dropped inside a statistics
/// scope, so what it frees is attributed as what it allocated was
/// (`pgdt_core::instrument`); its charge, declared last, is released after.
#[cfg(feature = "introspect")]
impl Drop for Gatherer {
    fn drop(&mut self) {
        let _attributed = StatisticsScope::enter();
        drop(mem::take(&mut self.columns));
        drop(mem::take(&mut self.groups));
        drop(self.pieces.take());
        (self.head, self.open) = (None, None);
    }
}

impl BlockObserver for Gatherer {
    fn observe_row(&mut self, offset: u64, raw: &[u8]) {
        self.rows += 1;
        if self.refused.is_some() {
            return;
        }
        // **Checked before it is gathered, and whether or not gathering has
        // declined**: a strict parse reads every field of every row.
        let line = self.rows;
        if let Some(refusal) = self.checks.as_deref().and_then(|c| check_row(c, offset, line, raw))
        {
            let _attributed = StatisticsScope::enter();
            return self.refuse(refusal);
        }
        if self.stopped() || !self.gathers {
            return;
        }
        let _attributed = StatisticsScope::enter();
        let index = offset / self.group_size;
        match &mut self.open {
            Some(group) if group.index == index => group.rows += 1,
            _ => {
                self.close_through(offset, index);
                // Closing may have merged the groups, coarsening the index.
                let index = offset / self.group_size;
                self.open = Some(OpenGroup { index, first_start: offset, rows: 1 });
                self.fit_cap(false);
            }
        }
        if !self.splits {
            self.decline_if_over();
            return;
        }
        for (at, (field, column)) in split_fields(raw).zip(self.columns.iter_mut()).enumerate() {
            let Some(column) = column else { continue };
            let refusal = |column: &ColumnGatherer| FieldRefusal {
                offset,
                line: self.rows,
                column: at,
                declared_type: column.declared_type.clone().unwrap_or_default(),
                value: decode_field(field).ok().flatten().map(Cow::into_owned),
            };
            match column.observe(field) {
                Ok(Observed { grew, ignored }) => {
                    self.uncharged += grew;
                    if ignored {
                        IgnoredRefusals::add(&mut self.ignored, refusal(column));
                    }
                }
                Err(Refused) => {
                    let refusal = refusal(column);
                    return self.refuse(refusal);
                }
            }
        }
        if self.uncharged >= STATISTICS_ACCOUNT_CHARGE_STEP as i64 {
            self.charge_held();
        }
        // **The row is whole before the decline**, so nothing is freed
        // half-observed; every path above that charges leaves `over` set for
        // it (`docs/design/decisions.md`, "D85").
        self.decline_if_over();
    }

    /// **The observer's charge becomes the block's retained bytes** once the
    /// interning maps are freed, in one update of the account — and a block
    /// that declined answers the allowance it declined under, holding nothing
    /// to retain.
    fn finish(mut self: Box<Self>, end: u64) -> BlockGathered {
        let _attributed = StatisticsScope::enter();
        debug_assert!(!self.piece, "a piece is joined, never finished");
        if let Some(refusal) = self.refused.take() {
            return BlockGathered::Refused(refusal);
        }
        if !self.gathers {
            return BlockGathered::Checked;
        }
        if let Some(allowance) = self.declined {
            return BlockGathered::Declined { allowance };
        }
        if let Some(group) = &self.open {
            let next = group.index + 1;
            self.close_through(end, next);
        }
        self.fit_cap(true);
        self.fit_density();
        let columns = mem::take(&mut self.columns);
        let statistics = BlockStatistics {
            group_size: self.group_size,
            sizing: self.record,
            groups: mem::take(&mut self.groups),
            columns: columns.into_iter().map(|c| c.map(ColumnGatherer::finish)).collect(),
        };
        self.charge.retain(statistics.heap_bytes());
        BlockGathered::Gathered(statistics)
    }

    fn piece(&self) -> Box<dyn BlockObserver> {
        let _attributed = StatisticsScope::enter();
        let columns = self.columns.iter().map(|c| c.as_ref().map(ColumnGatherer::fresh)).collect();
        let charge = Charge::new(Arc::clone(self.charge.account()), Term::Pieces);
        let sizing = Sizing {
            group_size: self.group_size,
            cap: None,
            min_rows: None,
            max_rows: None,
            record: self.record,
        };
        let mut piece = Gatherer::block(sizing, columns, charge);
        piece.piece = true;
        piece.gathers = self.gathers;
        piece.checks = self.checks.clone();
        // A piece of a block that has already stopped gathers nothing: it
        // still reads its rows for the census and the block's extent, and
        // holds no statistic while it does.
        piece.declined = self.declined;
        piece.refused = self.refused.clone();
        let pieces = self.pieces.get_or_init(Arc::default);
        piece.pieces = OnceLock::from(Arc::clone(pieces));
        piece.charge_held();
        Box::new(piece)
    }

    /// **A declined piece declines its block**, the block having lost that
    /// piece's rows and being unable to state statistics over the rest
    /// (`docs/design/decisions.md`, "D85"); a block that declined while the
    /// window ran drops every piece it is handed instead of folding it.
    ///
    /// **A refusal outranks a decline, and the earlier row's outranks a later
    /// one's**: a block that refused drops every piece, its rows all
    /// following the one it refused, and one that has not takes a piece's,
    /// its line numbered past this block's rows. Every other piece's rows are
    /// counted, folded or dropped.
    fn absorb(&mut self, later: Box<dyn BlockObserver>) {
        let _attributed = StatisticsScope::enter();
        let mut later = later.into_any().downcast::<Gatherer>().expect("a piece of this observer");
        if self.refused.is_some() {
            drop(later);
            return;
        }
        if let Some(mut refusal) = later.refused.take() {
            drop(later);
            refusal.line += self.rows;
            return self.refuse(refusal);
        }
        let mut ignored = later.ignored.take();
        for column in ignored.iter_mut().flat_map(|ignored| &mut ignored.columns) {
            column.first.line += self.rows;
        }
        IgnoredRefusals::fold(&mut self.ignored, ignored);
        self.rows += later.rows;
        if !self.gathers {
            return drop(later);
        }
        match (self.declined, later.declined) {
            (Some(_), _) => drop(later),
            (None, Some(allowance)) => {
                drop(later);
                self.decline(allowance);
            }
            (None, None) => {
                self.join(*later);
                self.fit_cap(false);
                self.decline_if_over();
            }
        }
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    fn take_ignored(&mut self) -> Option<IgnoredRefusals> {
        self.ignored.take()
    }

    fn checks_every_field(&self) -> bool {
        self.checks.is_some()
    }
}
