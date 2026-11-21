//! What a block's gatherer and a piece's share: the groups and the columns
//! over them, their charge, the pairwise merge and the decline
//! (`docs/design/decisions.md`, "D81", "D82", "D85").

use std::mem;

use pgdt_core::internal::StatisticsScope;
use pgdt_model::{GroupSizing, RowGroup, internal::vec_heap};

use super::bounds::{Refused, merge_adjacent};
use super::charge::push_charged;
use super::clip::LongRead;
use super::column::{ColumnGatherer, Observed};
use super::read::Gather;
use crate::statistics::account::{Charge, STATISTICS_ACCOUNT_CHARGE_STEP};
use crate::statistics::request::StatisticsBackfill;

/// The group a row is being added to.
pub(super) struct OpenGroup {
    pub(super) index: u64,
    pub(super) first_start: u64,
    pub(super) rows: u64,
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

/// How [`Groups::close_through`] closes a group: a block's, merging past the
/// cap it merges past now ([`BlockGatherer::merging_cap`](super::block::BlockGatherer::merging_cap)),
/// or a piece's, holding its first group open in its head
/// ([`PieceGatherer::head`](super::piece::PieceGatherer::head)) and merging
/// nothing.
pub(super) enum Closing<'a> {
    Block(Option<usize>),
    Piece(&'a mut Option<(OpenGroup, u64)>),
}

/// **A block's or a piece's groups and the columns gathered over them**,
/// charged to the pass's account as they grow
/// (`docs/design/decisions.md`, "D81"). Empty once it has stopped: a decline
/// or a refusal frees everything it holds, so no column answers a field.
pub(super) struct Groups {
    pub(super) group_size: u64,
    /// Every closed group in order, from the block's first — or, in a piece,
    /// from the one after its head.
    pub(super) groups: Vec<RowGroup>,
    pub(super) open: Option<OpenGroup>,
    pub(super) columns: Vec<Option<ColumnGatherer>>,
    /// What the open groups have grown by since [`Self::charge_held`] last
    /// ran, which it runs again once this passes [`STATISTICS_ACCOUNT_CHARGE_STEP`].
    uncharged: i64,
    /// Whether the account stood over its allowance at the last charge update
    /// — read at a row's end and at a fold's, never mid-merge, so an observer
    /// declines only where its structures are whole.
    over: bool,
    /// What a piece a block is folding in still holds, which the block's
    /// charge carries until the fold ends ([`Self::carry`]).
    carried: (u64, u64),
    /// The allowance this declined under, once the account passed it:
    /// everything gathered is freed, no further row is gathered, and a block
    /// answers [`BlockGathered::Declined`](pgdt_model::BlockGathered::Declined)
    /// (`docs/design/decisions.md`, "D85"). A piece carries it into the block
    /// it folds into, the block having lost that piece's rows.
    pub(super) declined: Option<u64>,
    /// What these hold, in the pass's account. **Last**, so it is released
    /// after everything above is freed ([`Charge`]).
    pub(super) charge: Charge,
}

impl Groups {
    /// `columns` gathered at `group_size`, `declined` under an allowance
    /// already where the block they gather for has, and charged to `charge`
    /// as they are made.
    pub(super) fn new(
        group_size: u64,
        columns: Vec<Option<ColumnGatherer>>,
        declined: Option<u64>,
        charge: Charge,
    ) -> Self {
        let mut groups = Self {
            group_size,
            groups: Vec::new(),
            open: None,
            columns,
            uncharged: 0,
            over: false,
            carried: (0, 0),
            declined,
            charge,
        };
        groups.charge_held();
        groups
    }

    /// Free everything these hold, the charge going to nothing in the same
    /// update: what a decline and a refusal share.
    fn release(&mut self) {
        debug_assert_eq!(self.carried, (0, 0), "a fold is not a place to stop");
        drop(mem::take(&mut self.columns));
        drop(mem::take(&mut self.groups));
        self.open = None;
        self.uncharged = 0;
        self.charge.set(0, 0);
    }

    /// **Drop everything these hold and gather no more**, the account
    /// having passed its allowance: the groups and columns are freed, the
    /// charge goes to nothing in the same update, and the allowance declined
    /// under is what the map records
    /// (`pgdt_model::BlockRecord::statistics_declined`). Called with the
    /// caller's [`StatisticsScope`] open, so what it frees is attributed as
    /// what allocated it was.
    ///
    /// **Nothing else declines with it**: the scan goes on, and a piece still
    /// gathering is freed as it is folded in or dropped
    /// (`docs/design/decisions.md`, "D85").
    ///
    /// deficiency: KD33 — the account this tests is cumulative, and nothing
    /// releases `Term::Retained` while the pass gathers forward (the back-fill
    /// does, replacing the statistics of a block this run gathered,
    /// `crate::backfill::backfill_statistics`), so
    /// once a long dump's retained statistics reach the allowance every block
    /// from there on declines on its first charge update. Statistics are then a *prefix* of
    /// the file rather than a sample of it, and a query prunes nothing over
    /// the tail; how much is covered depends on the allowance the box
    /// resolved. Coarsening under that pressure is refused by "D85" — a cache
    /// would depend on its container — so the remedy owned by P23 is a
    /// granularity derived from the dump's length, which is known up front and
    /// is the same on every machine.
    pub(super) fn decline(&mut self, allowance: u64) {
        self.declined = Some(allowance);
        self.release();
    }

    /// Decline where the last charge update found the account over its
    /// allowance, which [`Self::charge_held`] records. Called only of
    /// gathering that has not refused.
    pub(super) fn decline_if_over(&mut self) {
        if self.over && self.declined.is_none() {
            let allowance = self.charge.allowance().expect("only an allowance can be passed");
            self.decline(allowance);
        }
    }

    /// The heap these hold, as their structure and their interning maps.
    pub(super) fn held(&self) -> (u64, u64) {
        let own = vec_heap(&self.groups) + vec_heap(&self.columns);
        self.columns
            .iter()
            .flatten()
            .map(ColumnGatherer::held)
            .fold((own, 0), |(structure, interned), (s, i)| (structure + s, interned + i))
    }

    /// Charge what these hold now to the account, with what a piece being
    /// folded in still holds.
    pub(super) fn charge_held(&mut self) {
        let (structure, interned) = self.held();
        self.over = self.charge.set(structure + self.carried.0, interned + self.carried.1);
        self.uncharged = 0;
    }

    /// Charge the open groups' growth, where they have grown since the last
    /// charge.
    pub(super) fn charge_growth(&mut self) {
        if self.uncharged != 0 {
            self.charge_held();
        }
    }

    /// **What a piece being folded in still holds is `held`**, charged here
    /// in the same step.
    pub(super) fn carry(&mut self, held: (u64, u64)) {
        self.carried = held;
        self.charge_held();
    }

    /// **Take `later`'s charge onto this one in one update**, from where
    /// this charges what `later` still holds beside what it holds itself
    /// ([`Self::carry`]) until [`Self::hand_back`].
    pub(super) fn take_over(&mut self, later: &mut Groups) {
        later.charge_held();
        self.charge.take_over(&mut later.charge);
        self.carry(later.held());
    }

    /// Hand what is left of `later` back to its own charge in one update,
    /// released as it is freed.
    pub(super) fn hand_back(&mut self, later: &mut Groups) {
        self.charge.hand_back(&mut later.charge, mem::take(&mut self.carried));
    }

    /// A row begins at `offset`: counted into the open group where it
    /// starts in it, or else the groups before it are closed
    /// ([`Self::close_through`]) and one opened for it, answering `true`.
    /// Called inside a [`StatisticsScope`].
    pub(super) fn open_row(&mut self, offset: u64, closing: Closing<'_>) -> bool {
        let index = offset / self.group_size;
        match &mut self.open {
            Some(group) if group.index == index => {
                group.rows += 1;
                false
            }
            _ => {
                self.close_through(offset, index, closing);
                // Closing may have merged the groups, coarsening the index.
                let index = offset / self.group_size;
                self.open = Some(OpenGroup { index, first_start: offset, rows: 1 });
                true
            }
        }
    }

    /// A row has ended: its growth charged once it passes the step, and the
    /// gathering declined where the account is over its allowance.
    pub(super) fn end_row(&mut self) {
        let _attributed = StatisticsScope::enter();
        if self.uncharged >= STATISTICS_ACCOUNT_CHARGE_STEP as i64 {
            self.charge_held();
        }
        // **The row is whole before the decline**, so nothing is freed
        // half-observed; every path above that charges leaves `over` set for
        // it (`docs/design/decisions.md`, "D85").
        self.decline_if_over();
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
    /// before [`BlockGatherer::fit_cap`](super::block::BlockGatherer::fit_cap).
    /// A piece's first group is held open in its head rather than closed,
    /// and a piece lists nothing ahead of its first row.
    pub(super) fn close_through(&mut self, end: u64, next: u64, closing: Closing<'_>) {
        self.charge_growth();
        let mut next = next;
        let (cap, mut head) = match closing {
            Closing::Block(cap) => (cap, None),
            Closing::Piece(head) => (None, Some(head)),
        };
        let mut at = match self.open.take() {
            Some(group) => {
                let mut after = group.index + 1;
                match head.as_mut() {
                    Some(head) if head.is_none() => {
                        for column in self.columns.iter_mut().flatten() {
                            column.hold_head();
                        }
                        **head = Some((group, end));
                    }
                    _ => {
                        self.push_closed(group, end);
                        self.merge_whole_pairs(&mut after, &mut next, cap);
                    }
                }
                after
            }
            // A piece before its first row lists nothing ahead of it.
            None if head.is_some() => next,
            None => 0,
        };
        while at < next {
            push_charged(&mut self.groups, RowGroup { rows: 0, bytes: 0 }, &mut self.charge);
            for column in self.columns.iter_mut().flatten() {
                column.close_group(&mut self.charge);
            }
            at += 1;
            self.merge_whole_pairs(&mut at, &mut next, cap);
        }
        self.charge_held();
    }

    /// Merge the closed groups pairwise if they are past `cap` and even in
    /// number, halving `at` and `next` — group indices — with them.
    fn merge_whole_pairs(&mut self, at: &mut u64, next: &mut u64, cap: Option<usize>) {
        let Some(cap) = cap else { return };
        let closed = self.groups.len();
        if closed > cap && closed.is_multiple_of(2) {
            self.merge_pairs();
            (*at, *next) = (*at / 2, *next / 2);
        }
    }

    /// Take the last closed group back into the open group following it, as
    /// though both groups' rows had been observed into one.
    pub(super) fn reopen_last(&mut self) {
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
    pub(super) fn merge_pairs(&mut self) {
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

    pub(super) fn push_closed(&mut self, group: OpenGroup, end: u64) {
        let closed = RowGroup { rows: group.rows, bytes: end - group.first_start };
        push_charged(&mut self.groups, closed, &mut self.charge);
        for column in self.columns.iter_mut().flatten() {
            column.close_group(&mut self.charge);
        }
    }

    /// The growth `observed` answered, counted towards the next charge.
    fn grown(&mut self, observed: Result<Observed, Refused>) -> Result<bool, Refused> {
        let Observed { grew, ignored } = observed?;
        self.uncharged += grew;
        Ok(ignored)
    }

    /// Under the instrument, what these allocated in a statistics scope is
    /// freed in one, as it was allocated (`pgdt_core::instrument`); the
    /// charge, declared last, is released after.
    #[cfg(feature = "introspect")]
    pub(super) fn free_attributed(&mut self) {
        let _attributed = StatisticsScope::enter();
        drop(mem::take(&mut self.columns));
        drop(mem::take(&mut self.groups));
        self.open = None;
    }
}

impl Gather for Groups {
    fn long(&self, column: usize) -> Option<LongRead> {
        self.columns.get(column)?.as_ref().map(ColumnGatherer::long)
    }

    fn observe(&mut self, column: usize, field: &[u8]) -> Option<Result<bool, Refused>> {
        let observed = self.columns.get_mut(column)?.as_mut()?.observe(field);
        Some(self.grown(observed))
    }

    fn observe_long(
        &mut self,
        column: usize,
        long: LongRead,
        escaped: usize,
    ) -> Option<Result<bool, Refused>> {
        let gatherer = self.columns.get_mut(column)?.as_mut()?;
        let observed = gatherer.observe_long(long.finish(), escaped);
        Some(self.grown(observed))
    }

    fn declared_type(&self, column: usize) -> String {
        let column = self.columns.get(column).and_then(Option::as_ref);
        column.and_then(|column| column.declared_type.clone()).unwrap_or_default()
    }

    /// **Stop at a field PostgreSQL refuses**: the pass fails on it, so what
    /// was gathered is freed rather than finished.
    fn free(&mut self) {
        self.release();
    }
}
