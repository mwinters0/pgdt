//! The statistics gatherer: [`crate::statistics::BlockObserver`] implemented
//! at L4, where a column's comparison is known
//! (`docs/design/decisions.md`, "D74").
//!
//! [`observer_for`] builds one per `COPY` block from the block's resolved
//! schema. Every tracked column counts its NULLs per group; a scalar column
//! also keeps per-group bounds and the block's row order in each order some
//! semantics compares it by exactly, under the key a filter orders by, a
//! float's zeros told apart ([`ValueKey::stored_order`]) — one set where the
//! orders coincide, two where PostgreSQL's is exact and Arrow's is another
//! (`crate::ResolvedSchema::bounds_kinds`);
//! a column its comparison equates exactly keeps a dictionary per group; and a
//! column the typed read emits as an integer or a `Decimal128` keeps a sum per
//! group; and every column keeps its values' text bytes per group. **A column
//! whose type cannot hold every value counts those per group**, and its
//! bounds and row order are over the values it holds, each other view kept
//! apart only where a group holds one it reads otherwise
//! (`docs/design/decisions.md`, "D97"; [`KeyedGroup`], [`ViewOrders`]).
//!
//! **A leader piece gathers into an observer of its own, and the pieces join
//! in file order into exactly what one observer handed every row gathers**
//! ([`Gatherer::join`]), unless the block declines, which a stated allowance
//! decides over every piece charging at once (`docs/design/decisions.md`,
//! "D85"). Two things cross a join: the group a cut falls
//! inside, which a piece holds open rather than closing ([`Gatherer::head`]),
//! and each ordered column's first value, which the rows before the piece
//! place their last value against ([`RowOrder`]).
//!
//! **Every observer charges what it holds to the pass's
//! [`StatisticsAccount`]** (`docs/design/decisions.md`, "D81"): a block's
//! observer as [`Term::Gathering`], a piece's as [`Term::Pieces`], each
//! dictionary's interning map as [`Term::Interned`]. What it holds is
//! recomputed, O(1) a column, whenever its rows' growth passes
//! [`STATISTICS_ACCOUNT_CHARGE_STEP`], as each column closes a group and through a piece's fold;
//! a vector or a map is charged ahead of the allocation growing it.
//!
//! **A block past its cap merges its closed groups pairwise into exactly what
//! gathering at twice the size gathers** (`docs/design/decisions.md`, "D82"),
//! never mid-scan while a piece it made is alive; at the block's end it merges
//! whatever is outstanding, a piece alive there joining nothing more
//! ([`Gatherer::fit_cap`]). **A finished
//! block short of its density minimum merges the same way**
//! (`docs/design/decisions.md`, "D82"; [`Gatherer::fit_density`]).
//!
//! **A block whose gathering passes the pass's statistics allowance declines**
//! ([`Gatherer::decline`]): it frees what it holds, reads its remaining rows
//! for the census alone, and answers [`BlockGathered::Declined`] with the
//! allowance, which the map records (`docs/design/decisions.md`, "D85"). The
//! close [`Gatherer::finish`] makes is kept even where it passes the
//! allowance; the next block declines at its first charge.

use std::any::Any;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::mem::{self, align_of, size_of};
use std::sync::{Arc, OnceLock};

use arrow::datatypes::DataType;

use crate::copy::{CopyHeader, decode_field, split_fields};
use crate::decode::{decimal_unscaled_digits, decode_bytea, decode_bytea_escape, render_bytea};
use crate::index::{Unrepresentable, UnrepresentableTier};
use crate::instrument::StatisticsScope;
use crate::pgtype::{CompareKind, ComparisonPlan, ComparisonSemantics, NestedPlan};
use crate::preamble::{ColumnDef, DumpMetadata};
use crate::predicate::ValueKey;
use crate::resolve::{ResolvedSchema, SchemaMode, resolve_columns};
use crate::statistics::{
    BlockGathered, BlockObserver, BlockStatistics, Bounds, BoundsView, Charge, ColumnBounds,
    ColumnDictionary, ColumnStatistics, DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES,
    GroupSizing, RowGroup, STATISTICS_ACCOUNT_CHARGE_STEP, Sortedness, StatisticsAccount,
    StatisticsBackfill, StatisticsRequest, Term, max_rows_group, min_rows_group, text_heap,
    vec_heap,
};
use crate::unrepresentable::{ColumnTier, column_tiers};

/// The observer for one block, or `None` when `request` tracks nothing in it,
/// charging what it holds to `account`.
///
/// Resolved against an empty census: the census moves only an array column,
/// and a nested column gets neither bounds nor a dictionary.
pub(crate) fn observer_for(
    request: &StatisticsRequest,
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    account: &Arc<StatisticsAccount>,
) -> Option<Box<dyn BlockObserver>> {
    let gathering = request.gathering(request.tracked_columns(header)?);
    Some(observer_tracking(&gathering, header, metadata, database, account))
}

/// The observer for one block gathering `plan`'s columns — positional to
/// `header` — at its group size, merged pairwise past its cap and short of its
/// minimum: what [`observer_for`] builds from a request, and what a back-fill
/// builds from [`StatisticsRequest::backfill`]'s answer.
pub(crate) fn observer_tracking(
    plan: &StatisticsBackfill,
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    account: &Arc<StatisticsAccount>,
) -> Box<dyn BlockObserver> {
    let _attributed = StatisticsScope::enter();
    // Opened before anything the observer allocates, and charged once the
    // resolution's scratch is freed, so neither update reads either as held.
    let charge = Charge::new(Arc::clone(account), Term::Gathering);
    let qualified = header.qualified_name();
    let resolved = stored_resolution(header, metadata, database);
    let tiers = column_tiers(&resolved);
    let columns: Vec<Option<ColumnGatherer>> = header
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            plan.columns.get(i).is_some_and(|&t| t).then(|| {
                let def = declared_column(metadata, database, &qualified, name);
                ColumnGatherer::new(
                    def.map(|d| d.declared_type.clone()),
                    def.and_then(|d| d.collation.clone()),
                    resolved.bounds_kinds(i),
                    &resolved.comparisons[i],
                    &resolved.plans[i],
                    resolved.schema.field(i).data_type(),
                    tiers[i].clone(),
                )
            })
        })
        .collect();
    drop((resolved, tiers, qualified));
    let mut gatherer = Gatherer::block(Sizing::of(plan), columns, charge);
    gatherer.charge_held();
    // An account already full declines this block before its first row, so a
    // pass past its allowance gathers no row of any block after it
    // (`docs/design/decisions.md`, "D85").
    gatherer.decline_if_over();
    Box::new(gatherer)
}

/// Which of `header`'s columns gathering keeps bounds for, positionally, under
/// the DDL `metadata` states for it — what a block's held statistics are read
/// against to find a column gathered by a build that kept none
/// ([`crate::StatisticsRequest::backfill`]).
pub(crate) fn bounded_columns(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> Vec<bool> {
    stored_bounds_kinds(header, metadata, database).iter().map(|kinds| kinds[0].is_some()).collect()
}

/// The kinds each of `header`'s columns' stored sets of bounds are ordered
/// by, positionally, resolved as [`observer_tracking`] resolves them — so for
/// a column whose statistics record the declared type and collation
/// `metadata` states now, the kinds its sets were gathered under
/// (`crate::prune`; `docs/design/decisions.md`, "D79").
pub(crate) fn stored_bounds_kinds(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> Vec<[Option<CompareKind>; 2]> {
    let resolved = stored_resolution(header, metadata, database);
    (0..header.columns.len()).map(|i| resolved.bounds_kinds(i)).collect()
}

/// `header`'s columns resolved as [`observer_tracking`] resolves them, under
/// the DDL `metadata` states for it now: for a column whose statistics record
/// that declared type and collation, the resolution they were gathered under.
pub(crate) fn stored_resolution(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> ResolvedSchema {
    resolve_columns(
        &header.qualified_name(),
        &header.columns,
        metadata,
        database,
        SchemaMode::Typed,
        &[],
    )
}

/// Whether gathering keeps column `i` of `stored` a dictionary whose entries
/// are each field's own text, as [`ColumnGatherer::new`] and
/// [`DictionaryGatherer::observe`] decide it: every dictionary kept but a
/// `character` column's, whose entries give up their trailing blanks.
pub(crate) fn dictionary_holds_field_text(stored: &ResolvedSchema, i: usize) -> bool {
    let comparison = &stored.comparisons[i];
    stored.plans[i] == NestedPlan::Scalar
        && comparison.dictionary_answers_in(ComparisonSemantics::Postgres)
        && !matches!(comparison, ComparisonPlan::Compared { kind: CompareKind::PaddedText, .. })
}

/// Whether gathering keeps sums for a column the typed read emits as
/// `data_type` ([`crate::statistics::ColumnStatistics::sums`]) — and so
/// whether a column a query emits so is the one they were summed as.
pub(crate) fn keeps_sums(data_type: &DataType) -> bool {
    Summand::of(data_type).is_some()
}

/// The declaration `metadata` holds for column `name` of the table
/// `qualified` in `database`, wherever the table takes it from
/// ([`crate::preamble::DatabaseMetadata::declared_column`]) — what a column's
/// statistics record their declared type and collation from, and what a
/// query compares those against (`crate::prune`).
pub(crate) fn declared_column<'m>(
    metadata: Option<&'m DumpMetadata>,
    database: Option<&str>,
    qualified: &str,
    name: &str,
) -> Option<&'m ColumnDef> {
    metadata
        .and_then(|m| m.databases.iter().find(|db| db.name.as_deref() == database))
        .and_then(|db| db.declared_column(qualified, name))
}

/// The group a row is being added to.
struct OpenGroup {
    index: u64,
    first_start: u64,
    rows: u64,
}

/// How a block observer sizes its groups: a [`StatisticsBackfill`] less its
/// columns.
#[derive(Clone, Copy)]
struct Sizing {
    group_size: u64,
    cap: Option<usize>,
    min_rows: Option<u64>,
    max_rows: Option<u64>,
    record: GroupSizing,
}

impl Sizing {
    fn of(plan: &StatisticsBackfill) -> Self {
        let (group_size, cap) = (plan.group_size, plan.group_cap);
        let (min_rows, max_rows) = (plan.min_rows, plan.max_rows);
        Self { group_size, cap, min_rows, max_rows, record: plan.sizing }
    }
}

struct Gatherer {
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
    columns: Vec<Option<ColumnGatherer>>,
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
    /// it: everything gathered is freed, no further row is read, and a block
    /// answers [`BlockGathered::Declined`]
    /// (`docs/design/decisions.md`, "D85"). A piece carries it into the block
    /// it folds into, the block having lost that piece's rows.
    declined: Option<u64>,
    /// What this observer holds, in the pass's account. **Last**, so it is
    /// released after everything above is freed ([`Charge`]).
    charge: Charge,
}

impl Gatherer {
    fn block(sizing: Sizing, columns: Vec<Option<ColumnGatherer>>, charge: Charge) -> Self {
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
            columns,
            uncharged: 0,
            over: false,
            carried: (0, 0),
            declined: None,
            charge,
        }
    }

    /// **Drop everything this observer holds and gather no more**, the account
    /// having passed its allowance: the block's groups and columns are freed,
    /// its charge goes to nothing in the same update, and the allowance it
    /// declined under is what the map records
    /// (`crate::index::CopyBlock::statistics_declined`). Called with the
    /// caller's [`StatisticsScope`] open, so what it frees is attributed as
    /// what allocated it was.
    ///
    /// **Nothing else declines with it**: the scan goes on, and a piece still
    /// gathering is freed as it is folded in or dropped
    /// (`docs/design/decisions.md`, "D85").
    ///
    /// deficiency: KD33 — the account this tests is cumulative, and nothing
    /// releases `Term::Retained` while the pass gathers forward (the back-fill
    /// does, replacing a block's statistics, `crate::stream::backfill_statistics`), so
    /// once a long dump's retained statistics reach the allowance every block
    /// from there on declines on its first charge update. Statistics are then a *prefix* of
    /// the file rather than a sample of it, and a query prunes nothing over
    /// the tail; how much is covered depends on the allowance the box
    /// resolved. Coarsening under that pressure is refused by "D85" — a cache
    /// would depend on its container — so the remedy owned by P23 is a
    /// granularity derived from the dump's length, which is known up front and
    /// is the same on every machine.
    fn decline(&mut self, allowance: u64) {
        debug_assert_eq!(self.carried, (0, 0), "a fold is not a place to decline");
        self.declined = Some(allowance);
        drop(mem::take(&mut self.columns));
        drop(mem::take(&mut self.groups));
        (self.head, self.open, self.splits) = (None, None, false);
        self.uncharged = 0;
        self.charge.set(0, 0);
    }

    /// Decline where the last charge update found the account over its
    /// allowance, which [`Self::charge_held`] records.
    fn decline_if_over(&mut self) {
        if self.over && self.declined.is_none() {
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
    fn charge_held(&mut self) {
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
    /// Pieces are folded with none of them gathering (`crate::leader`), so no
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
            let state = match first_end {
                Some(_) => theirs.head.take().expect("a piece past its first group holds it"),
                None => theirs.take_group(),
            };
            if continues {
                mine.group.absorb(state);
            } else {
                mine.group = state;
            }
            for (mine, theirs) in mine.bounds.iter_mut().zip(&mut theirs.bounds) {
                if let (Some(mine), Some(theirs)) = (mine, theirs) {
                    mine.rows.absorb(mem::take(&mut theirs.rows));
                }
            }
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
/// (`crate::instrument`); its charge, declared last, is released after.
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
        if self.declined.is_some() {
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
        for (field, column) in split_fields(raw).zip(self.columns.iter_mut()) {
            if let Some(column) = column {
                self.uncharged += column.observe(field);
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
        // A piece of a block that has already declined gathers nothing: it
        // still reads its rows for the census and the block's extent, and
        // holds no statistic while it does.
        piece.declined = self.declined;
        let pieces = self.pieces.get_or_init(Arc::default);
        piece.pieces = OnceLock::from(Arc::clone(pieces));
        piece.charge_held();
        Box::new(piece)
    }

    /// **A declined piece declines its block**, the block having lost that
    /// piece's rows and being unable to state statistics over the rest
    /// (`docs/design/decisions.md`, "D85"); a block that declined while the
    /// window ran drops every piece it is handed instead of folding it.
    fn absorb(&mut self, later: Box<dyn BlockObserver>) {
        let _attributed = StatisticsScope::enter();
        let later = later.into_any().downcast::<Gatherer>().expect("a piece of this observer");
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
}

/// How many times a block whose groups hold `rows` merges pairwise before its
/// **upper middle** group ([`min_rows_group`]) holds `min_rows`, or it is one
/// group: at each merge the rows per group are the sums of adjacent pairs, a
/// last odd group standing alone.
///
/// Reading the upper middle group rather than the nearest-rank `⌈G/2⌉`-th is
/// what makes the predicate **monotone in size**: a merged group falls short
/// only where both its parts did, so of `S ≤ ⌊G/2⌋` short groups at most
/// `⌊S/2⌋` pairs fall short, plus a short odd tail — within `⌊⌈G/2⌉/2⌋` at
/// every `G` (by cases on `G` mod 4), the same predicate at the coarser size.
/// `the_density_predicate_is_monotone_in_size` holds it.
///
/// **`max_rows` stops the merging first**, wherever the next size would put
/// more than that many rows in its 90th-percentile group ([`max_rows_group`]):
/// so a stated maximum outranks the minimum. Nothing here can make a block
/// *finer*: a block breaking `max_rows` at the size it gathered from is
/// re-read instead ([`StatisticsRequest::backfill`]).
///
/// deficiency: KD43 — the maximum's predicate is monotone in size only where
/// every group pairs. A last odd group stands alone, so a pairwise sum can
/// rank below the parts it replaces: at 19, 39, 59… groups a block already
/// past the maximum can merge once (`[10, 10, 0 × 17]` under a maximum of 5
/// becomes `[20, 0 × 9]`, whose 90th-percentile group is 0), and stopping at
/// the first merge that breaks the predicate can miss a coarser size that
/// would hold it. `an_unpaired_last_group_lets_a_block_past_the_maximum_merge`
/// pins the example. **(c) unowned**; promoted by a stated maximum seen to
/// leave a block's groups past it.
fn density_merges(mut rows: Vec<u64>, min_rows: u64, max_rows: Option<u64>) -> u32 {
    let mut merges = 0;
    while rows.len() > 1 && min_rows_group(&rows) < min_rows {
        let coarser: Vec<u64> = rows.chunks(2).map(|pair| pair.iter().sum()).collect();
        if max_rows.is_some_and(|max_rows| max_rows_group(&coarser) > max_rows) {
            break;
        }
        rows = coarser;
        merges += 1;
    }
    merges
}

/// How a column's values are summed: parsed as the typed read parses them
/// (`crate::batch`), and widened to the 128 bits every sum is kept in
/// (`docs/design/decisions.md`, "D91").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Summand {
    Int16,
    Int32,
    Int64,
    UInt32,
    /// A `numeric(p,s)`'s value unscaled at `s` places.
    Decimal128 {
        scale: i8,
    },
}

impl Summand {
    /// How a column the typed read emits as `data_type` is summed, or `None`
    /// for a type that keeps no sum.
    fn of(data_type: &DataType) -> Option<Self> {
        match data_type {
            DataType::Int16 => Some(Self::Int16),
            DataType::Int32 => Some(Self::Int32),
            DataType::Int64 => Some(Self::Int64),
            DataType::UInt32 => Some(Self::UInt32),
            DataType::Decimal128(_, scale) => Some(Self::Decimal128 { scale: *scale }),
            _ => None,
        }
    }

    /// `text` as the integer the typed read appends, or `None` where the read
    /// would refuse it.
    fn value(self, text: &str) -> Option<i128> {
        match self {
            Self::Int16 => text.parse::<i16>().ok().map(i128::from),
            Self::Int32 => text.parse::<i32>().ok().map(i128::from),
            Self::Int64 => text.parse::<i64>().ok().map(i128::from),
            Self::UInt32 => text.parse::<u32>().ok().map(i128::from),
            Self::Decimal128 { scale } => decimal_unscaled_digits(text, scale)?.parse().ok(),
        }
    }
}

struct ColumnGatherer {
    declared_type: Option<String>,
    collation: Option<String>,
    null_counts: Vec<u64>,
    /// How a value is summed, for a column keeping sums.
    summand: Option<Summand>,
    /// Each closed group's sum, `None` for a column keeping none and once a
    /// value has failed to decode ([`ColumnStatistics::sums`]).
    sums: Option<Vec<i128>>,
    /// Each closed group's text bytes ([`ColumnStatistics::value_bytes`]).
    value_bytes: Vec<u64>,
    /// Which tier a value is past, for a column whose type cannot hold every
    /// value its declared type can (`crate::unrepresentable`).
    tier: Option<ColumnTier>,
    /// Each closed group's count of such values, for a column `tier` tests
    /// ([`ColumnStatistics::unrepresentable`]).
    unrepresentable: Option<Vec<Unrepresentable>>,
    /// One per [`BoundsSet`], in its order.
    bounds: [Option<BoundsGatherer>; 2],
    dictionary: Option<DictionaryGatherer>,
    /// The column over the open group.
    group: GroupState,
    /// The column over a piece's held first group ([`Gatherer::head`]).
    head: Option<GroupState>,
}

impl ColumnGatherer {
    /// A set of bounds under each of `bounds`, the kinds some semantics
    /// orders the column by exactly (`crate::ResolvedSchema::bounds_kinds`),
    /// and a dictionary where the comparison equates exactly
    /// (`docs/design/decisions.md`, "D79"); a sum per group where the typed
    /// read emits `data_type` as an integer or a `Decimal128`; a count per
    /// group of the values past `tier`, where the type cannot hold every one;
    /// and, as for every tracked column, its NULLs and text bytes per group
    /// ([`ColumnStatistics`]).
    fn new(
        declared_type: Option<String>,
        collation: Option<String>,
        bounds: [Option<CompareKind>; 2],
        comparison: &ComparisonPlan,
        plan: &NestedPlan,
        data_type: &DataType,
        tier: Option<ColumnTier>,
    ) -> Self {
        let postgres = ComparisonSemantics::Postgres;
        let dictionary = match comparison {
            ComparisonPlan::Compared { kind, .. } if *plan == NestedPlan::Scalar => {
                comparison.dictionary_answers_in(postgres).then(|| DictionaryGatherer::new(kind))
            }
            _ => None,
        };
        let bounds = bounds.map(|kind| kind.map(BoundsGatherer::new));
        let summand = Summand::of(data_type);
        Self::with(declared_type, collation, bounds, dictionary, summand, tier)
    }

    fn with(
        declared_type: Option<String>,
        collation: Option<String>,
        bounds: [Option<BoundsGatherer>; 2],
        dictionary: Option<DictionaryGatherer>,
        summand: Option<Summand>,
        tier: Option<ColumnTier>,
    ) -> Self {
        let group = GroupState::fresh(&bounds);
        Self {
            declared_type,
            collation,
            null_counts: Vec::new(),
            summand,
            sums: summand.map(|_| Vec::new()),
            value_bytes: Vec::new(),
            unrepresentable: tier.as_ref().map(|_| Vec::new()),
            tier,
            bounds,
            dictionary,
            group,
            head: None,
        }
    }

    /// The heap this column holds, as its structure and its interning map,
    /// its open group and a held head included. O(1) but for a keyed bound's
    /// key, which [`DICTIONARY_ENTRY_MAX_BYTES`] bounds.
    fn held(&self) -> (u64, u64) {
        let named = self.declared_type.iter().chain(&self.collation).map(text_heap).sum::<u64>();
        let head = self.head.as_ref().map_or(0, GroupState::heap_bytes);
        let per_group = vec_heap(&self.null_counts)
            + self.sums.as_ref().map_or(0, vec_heap)
            + vec_heap(&self.value_bytes)
            + self.unrepresentable.as_ref().map_or(0, vec_heap);
        let mut structure = named + per_group + self.group.heap_bytes() + head;
        for bounds in self.bounds.iter().flatten() {
            structure += vec_heap(&bounds.groups)
                + vec_heap(&bounds.flags)
                + vec_heap(&bounds.tiers)
                + bounds.stored
                + bounds.rows.heap_bytes();
        }
        let mut interned = 0;
        if let Some(dictionary) = &self.dictionary {
            let (entries, map) = dictionary.held();
            structure += entries;
            interned = map;
        }
        (structure, interned)
    }

    /// The heap a row can change: the open group and the row order.
    fn open_heap(&self) -> u64 {
        self.group.heap_bytes()
            + self.bounds.iter().flatten().map(|b| b.rows.heap_bytes()).sum::<u64>()
    }

    /// A column of this one's kind that has gathered nothing: what a piece
    /// starts from.
    fn fresh(&self) -> Self {
        Self::with(
            self.declared_type.clone(),
            self.collation.clone(),
            self.bounds.each_ref().map(|b| b.as_ref().map(BoundsGatherer::fresh)),
            self.dictionary.as_ref().map(DictionaryGatherer::fresh),
            self.summand,
            self.tier.clone(),
        )
    }

    /// The open group's state, leaving a fresh one open.
    fn take_group(&mut self) -> GroupState {
        let fresh = GroupState::fresh(&self.bounds);
        mem::replace(&mut self.group, fresh)
    }

    fn hold_head(&mut self) {
        self.head = Some(self.take_group());
    }

    /// Observe one field, answering what the column's heap grew by.
    fn observe(&mut self, field: &[u8]) -> i64 {
        let before = self.open_heap();
        match decode_field(field) {
            Ok(None) => {
                self.group.nulls += 1;
                return 0;
            }
            Ok(Some(text)) => {
                let tier = self.tier.as_ref().and_then(|tier| tier.of(&text));
                if let Some(tier) = tier {
                    self.group.unrepresentable.add(tier);
                }
                self.group.value_bytes += text.len() as u64;
                // A value the type cannot hold is left out, as a NULL is: the
                // sum is of the values the type holds, read only in a view
                // taking the rest as NULL (`docs/design/decisions.md`, "D98").
                if let Some(summand) = self.summand.filter(|_| tier.is_none()) {
                    self.group.sum = self
                        .group
                        .sum
                        .zip(summand.value(&text))
                        .map(|(sum, value)| sum.wrapping_add(value));
                }
                for (bounds, group) in self.bounds.iter_mut().zip(&mut self.group.bounds) {
                    if let (Some(bounds), Some(group)) = (bounds, group) {
                        bounds.observe(group, &text, tier);
                    }
                }
                if let Some(dictionary) = &self.dictionary {
                    dictionary.observe(&mut self.group, &text);
                }
            }
            // Not text at all, so neither a key nor an entry: the group can
            // cover the row with neither, and the block's order is lost. No
            // decoding is longer than the escaped field, and nothing sums it.
            Err(_) => {
                self.group.value_bytes += field.len() as u64;
                self.group.sum = None;
                for (bounds, group) in self.bounds.iter_mut().zip(&mut self.group.bounds) {
                    if let (Some(bounds), Some(group)) = (bounds, group) {
                        bounds.lose_value(group, None);
                    }
                }
                self.group.lose_texts();
            }
        }
        self.open_heap() as i64 - before as i64
    }

    /// Close the open group, and move what it changed onto `charge` before
    /// the next column closes: **a close's growth is charged column by
    /// column**, so a wide block's account never waits on all its columns.
    /// A dictionary's interning map growing here is charged as it grows
    /// (`DictionaryGatherer::intern`), which the column's own difference then
    /// takes in rather than adding twice.
    fn close_group(&mut self, charge: &mut Charge) {
        let before = self.held();
        let base = charge.charged();
        let group = self.take_group();
        push_charged(&mut self.null_counts, group.nulls, charge);
        push_charged(&mut self.value_bytes, group.value_bytes, charge);
        if let Some(counts) = &mut self.unrepresentable {
            push_charged(counts, group.unrepresentable, charge);
        }
        match (&mut self.sums, group.sum) {
            (Some(sums), Some(sum)) => push_charged(sums, sum, charge),
            // The column held a value that is not its type: it has no sum.
            (sums, None) => *sums = None,
            (None, Some(_)) => {}
        }
        for (bounds, group) in self.bounds.iter_mut().zip(group.bounds) {
            if let (Some(bounds), Some(group)) = (bounds, group) {
                bounds.close_group(group, charge);
            }
        }
        if let Some(dictionary) = &mut self.dictionary {
            dictionary.close_group(group.texts, charge);
        }
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
    }

    /// Merge every closed group with the one after it
    /// ([`Gatherer::merge_pairs`]), moving `charge` by what the column frees
    /// before its dictionary renumbers, and again after.
    fn merge_pairs(&mut self, charge: &mut Charge) {
        let (before, base) = (self.held(), charge.charged());
        merge_adjacent(&mut self.null_counts, |a, b| a + b);
        if let Some(sums) = &mut self.sums {
            merge_adjacent(sums, i128::wrapping_add);
        }
        merge_adjacent(&mut self.value_bytes, |a, b| a + b);
        if let Some(counts) = &mut self.unrepresentable {
            merge_adjacent(counts, |mut a, b| {
                a.merge(&b);
                a
            });
        }
        for bounds in self.bounds.iter_mut().flatten() {
            bounds.merge_pairs();
        }
        if let Some(dictionary) = &mut self.dictionary {
            merge_adjacent(&mut dictionary.groups, union_indices);
            dictionary.indices = dictionary.groups.iter().flatten().map(vec_heap).sum();
        }
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
        if let Some(dictionary) = &mut self.dictionary {
            let (before, base) = (dictionary.held(), charge.charged());
            dictionary.renumber(charge);
            let after = dictionary.held();
            charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
        }
    }

    /// Take the last closed group back into the open group, which follows it
    /// ([`Gatherer::reopen_last`]), moving `charge` by the difference.
    fn reopen_last(&mut self, charge: &mut Charge) {
        let (before, base) = (self.held(), charge.charged());
        let nulls = self.null_counts.pop().expect("a closed group counts its NULLs");
        // A column that has lost its sums reopens a group without one, which
        // loses nothing further.
        let sum = self.sums.as_mut().map(|sums| sums.pop().expect("a closed group sums"));
        let value_bytes = self.value_bytes.pop().expect("a closed group counts its text");
        let unrepresentable =
            self.unrepresentable.as_mut().map_or_else(Unrepresentable::default, |counts| {
                counts.pop().expect("a closed group counts what its type cannot hold")
            });
        let bounds = self.bounds.each_mut().map(|b| b.as_mut().map(BoundsGatherer::reopen_last));
        let texts = match &mut self.dictionary {
            Some(dictionary) => dictionary.reopen_last(),
            None => Some(Vec::new()),
        };
        let text_bytes = texts.iter().flatten().map(text_heap).sum();
        let mut group =
            GroupState { nulls, sum, value_bytes, unrepresentable, bounds, texts, text_bytes };
        group.absorb(mem::replace(&mut self.group, GroupState::fresh(&[None, None])));
        self.group = group;
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
    }

    /// Append `later`'s closed groups, which follow this column's, and take
    /// its open group as this one's — **draining `later` in place**, and after
    /// each part moves or is freed moving `charge` by the pair's difference,
    /// so the account follows the piece's structures across rather than
    /// releasing them all at the end.
    fn append(&mut self, later: &mut ColumnGatherer, charge: &mut Charge) {
        let pair = |mine: &Self, theirs: &Self| {
            let ((a, b), (c, d)) = (mine.held(), theirs.held());
            (a + c, b + d)
        };
        let before = pair(self, later);
        let base = charge.charged();
        let resync = |now: (u64, u64), charge: &mut Charge| {
            charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
        };
        reserve_charged(&mut self.null_counts, later.null_counts.len(), charge);
        self.null_counts.extend_from_slice(&later.null_counts);
        later.null_counts = Vec::new();
        resync(pair(self, later), charge);
        reserve_charged(&mut self.value_bytes, later.value_bytes.len(), charge);
        self.value_bytes.extend_from_slice(&later.value_bytes);
        later.value_bytes = Vec::new();
        resync(pair(self, later), charge);
        if let (Some(mine), Some(theirs)) = (&mut self.unrepresentable, &mut later.unrepresentable)
        {
            reserve_charged(mine, theirs.len(), charge);
            mine.append(theirs);
            *theirs = Vec::new();
            resync(pair(self, later), charge);
        }
        match (&mut self.sums, &mut later.sums) {
            (Some(mine), Some(theirs)) => {
                reserve_charged(mine, theirs.len(), charge);
                mine.append(theirs);
                *theirs = Vec::new();
            }
            // The piece met a value that is not the column's type.
            (sums, None) => *sums = None,
            (None, Some(_)) => {}
        }
        resync(pair(self, later), charge);
        for set in 0..self.bounds.len() {
            let (Some(mine), Some(theirs)) = (&mut self.bounds[set], &mut later.bounds[set]) else {
                continue;
            };
            mine.append_tiers(theirs, charge);
            reserve_charged(&mut mine.groups, theirs.groups.len(), charge);
            reserve_charged(&mut mine.flags, theirs.flags.len(), charge);
            mine.groups.append(&mut theirs.groups);
            mine.flags.append(&mut theirs.flags);
            mine.stored += mem::take(&mut theirs.stored);
            (theirs.groups, theirs.flags) = (Vec::new(), Vec::new());
            resync(pair(self, later), charge);
        }
        let fresh = GroupState::fresh(&later.bounds);
        self.group = mem::replace(&mut later.group, fresh);
        if let (Some(mine), Some(theirs)) = (&mut self.dictionary, &mut later.dictionary) {
            mine.append(theirs, charge);
        }
        resync(pair(self, later), charge);
    }

    fn finish(self) -> ColumnStatistics {
        let [primary, datafusion] = self.bounds;
        ColumnStatistics {
            declared_type: self.declared_type,
            collation: self.collation,
            null_counts: self.null_counts,
            sums: self.sums,
            value_bytes: self.value_bytes,
            bounds: primary.map(BoundsGatherer::finish),
            datafusion_bounds: datafusion.map(BoundsGatherer::finish),
            dictionary: self.dictionary.map(DictionaryGatherer::finish),
            unrepresentable: self
                .unrepresentable
                .filter(|counts| counts.iter().any(|count| !count.is_zero())),
        }
    }
}

/// One column over one group while the group is open.
struct GroupState {
    nulls: u64,
    /// The values' wrapping sum so far, `None` once one did not decode as
    /// the column's type; read only for a column keeping sums.
    sum: Option<i128>,
    /// The values' text bytes so far.
    value_bytes: u64,
    /// The values past what the column's type holds so far, by tier.
    unrepresentable: Unrepresentable,
    /// One per [`BoundsSet`], `None` for a set the column does not keep.
    bounds: [Option<GroupBounds>; 2],
    /// The group's distinct texts in first-seen order, `None` once past a cap
    /// or at a field that is not text;
    /// read only for a column keeping a dictionary.
    texts: Option<Vec<String>>,
    /// The heap `texts`' strings hold, summed as each is pushed.
    text_bytes: u64,
}

impl GroupState {
    fn fresh(bounds: &[Option<BoundsGatherer>; 2]) -> Self {
        Self {
            nulls: 0,
            sum: Some(0),
            value_bytes: 0,
            unrepresentable: Unrepresentable::default(),
            bounds: bounds.each_ref().map(|b| b.as_ref().map(BoundsGatherer::fresh_group)),
            texts: Some(Vec::new()),
            text_bytes: 0,
        }
    }

    /// No distinct texts for this group: a cap passed, or a field not text.
    fn lose_texts(&mut self) {
        self.texts = None;
        self.text_bytes = 0;
    }

    /// The heap this group's state holds: its distinct texts and its running
    /// bounds.
    fn heap_bytes(&self) -> u64 {
        let texts = self.texts.as_ref().map_or(0, |texts| vec_heap(texts) + self.text_bytes);
        let bounds: u64 = self
            .bounds
            .iter()
            .flatten()
            .map(|bounds| match bounds {
                GroupBounds::Bytewise { min, max, .. } => {
                    [min, max].into_iter().flatten().map(|c| text_heap(&c.head)).sum()
                }
                GroupBounds::Keyed(group) => group.heap_bytes(),
            })
            .sum();
        texts + bounds
    }

    /// Fold `later`, the same column over the same group's following rows.
    /// The texts fold as the rows would have: each new one appended until a
    /// cap is passed.
    fn absorb(&mut self, later: GroupState) {
        self.nulls += later.nulls;
        self.sum = self.sum.zip(later.sum).map(|(sum, more)| sum.wrapping_add(more));
        self.value_bytes += later.value_bytes;
        self.unrepresentable.merge(&later.unrepresentable);
        for (mine, theirs) in self.bounds.iter_mut().zip(later.bounds) {
            if let (Some(mine), Some(theirs)) = (mine, theirs) {
                mine.absorb(theirs);
            }
        }
        let Some(theirs) = later.texts else {
            self.lose_texts();
            return;
        };
        let Some(mine) = &mut self.texts else { return };
        for text in theirs {
            if mine.contains(&text) {
                continue;
            }
            if mine.len() == DICTIONARY_MAX_ENTRIES {
                self.lose_texts();
                return;
            }
            self.text_bytes += text_heap(&text);
            mine.push(text);
        }
    }
}

/// How a column's values are ordered while gathering.
#[derive(Clone)]
enum Order {
    /// By the text itself, bytewise, once put in [`Canonical`] form: never
    /// keyed, since a key copies the whole value and a value may be hundreds
    /// of megabytes, and a bound past [`DICTIONARY_ENTRY_MAX_BYTES`] is truncated.
    Bytewise(Canonical),
    /// By [`ValueKey::stored_order`]: the key a filter orders by, a float's
    /// zeros told apart as Arrow's order tells them. A value past
    /// [`DICTIONARY_ENTRY_MAX_BYTES`] leaves its group unbounded and its block unordered,
    /// no truncation of it being a bound.
    Keyed(CompareKind),
}

/// The bytewise kinds, each by the text whose bytes order as its key does.
#[derive(Clone, Copy)]
enum Canonical {
    /// `text`, `varchar`, `name`, bounded bytewise in any collation: the text.
    Text,
    /// `character`: the text without its trailing blanks, which its comparison
    /// ignores — so a bound is stored unpadded.
    PaddedText,
    /// `bytea`: `\x` and lowercase hex pairs, which order as the bytes do —
    /// an `escape` value's head put in that form, as much as a bound reads
    /// ([`BYTEA_ESCAPE_HEAD_BYTES`]). A value in any other spelling is not
    /// placed.
    Bytea,
}

impl Canonical {
    /// The text whose bytes order `text` among its column's values, `None`
    /// where this kind cannot place it.
    fn of(self, text: &str) -> Option<Cow<'_, str>> {
        match self {
            Self::Text => Some(Cow::Borrowed(text)),
            Self::PaddedText => Some(Cow::Borrowed(text.trim_end_matches(' '))),
            Self::Bytea => match text.strip_prefix("\\x") {
                Some(hex) => (hex.len() % 2 == 0
                    && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
                .then_some(Cow::Borrowed(text)),
                None => decode_bytea_escape(text, BYTEA_ESCAPE_HEAD_BYTES)
                    .map(|head| Cow::Owned(render_bytea(&head))),
            },
        }
    }
}

/// How many of an `escape` value's bytes [`Canonical::of`] renders: enough
/// that a value cut there renders past [`CLIP_BYTES`], so its head is the
/// whole value's and it reads as no whole value, as the value itself does
/// (`docs/design/decisions.md`, "D76").
const BYTEA_ESCAPE_HEAD_BYTES: usize = CLIP_BYTES / 2;

/// How much of a bytewise value is kept: past the cap by one character's
/// width, so a kept head longer than the cap says its value is too.
const CLIP_BYTES: usize = DICTIONARY_ENTRY_MAX_BYTES + char::MAX.len_utf8();

/// A bytewise value's first [`CLIP_BYTES`], which is all a stored bound is
/// taken from (`docs/design/decisions.md`, "D76").
///
/// **Two values whose heads cannot be told apart store the same bound.** A
/// head that is not its whole value is at least `CLIP_BYTES` less one
/// character long, so two values it cannot place against each other share
/// more bytes than [`clipped_bounds`] reads of either. That is what lets a
/// group's bounds be joined from two halves' heads ([`GroupBounds::absorb`])
/// and still store what one pass over the group's values stores: each keeps a
/// value no other value in its half is certainly beyond, and that value can
/// differ from the true extreme only by bytes no stored bound holds.
#[derive(Clone)]
struct Clipped {
    head: String,
    /// Whether `head` is the whole value.
    whole: bool,
}

impl Clipped {
    fn of(text: &str) -> Self {
        let head = text_prefix(text, CLIP_BYTES);
        Self { head: head.to_owned(), whole: head.len() == text.len() }
    }

    /// Where `text` orders against the value this was clipped from, `None`
    /// when `text` begins with a head that is not the whole value. Two such
    /// values share every byte a stored bound reads, so either stands for
    /// the other in the group's bounds; only their row order is unknown.
    fn locate(&self, text: &str) -> Option<Ordering> {
        self.locate_bytes(text.as_bytes(), text.len())
    }

    /// [`Self::locate`] for a text known by its length and its first
    /// [`CLIP_BYTES`] bytes (all of them, if it is shorter) — as much as the
    /// answer reads — so a piece's first value is placed exactly as a row is.
    fn locate_bytes(&self, prefix: &[u8], len: usize) -> Option<Ordering> {
        let head = self.head.as_bytes();
        let shared = head.len().min(len);
        match prefix[..shared].cmp(&head[..shared]) {
            Ordering::Equal if len > head.len() => self.whole.then_some(Ordering::Greater),
            Ordering::Equal if len == head.len() && self.whole => Some(Ordering::Equal),
            Ordering::Equal => Some(Ordering::Less),
            unequal => Some(unequal),
        }
    }

    /// Where `other`'s value orders against this one's, known only by their
    /// heads: `None` exactly where the shorter head agrees with the longer and
    /// is not its whole value, which [`Clipped`] says stores the same bound.
    fn order(&self, other: &Clipped) -> Option<Ordering> {
        let (mine, theirs) = (self.head.as_bytes(), other.head.as_bytes());
        let shared = mine.len().min(theirs.len());
        match theirs[..shared].cmp(&mine[..shared]) {
            Ordering::Equal => {}
            unequal => return Some(unequal),
        }
        match theirs.len().cmp(&mine.len()) {
            Ordering::Less => other.whole.then_some(Ordering::Less),
            Ordering::Greater => self.whole.then_some(Ordering::Greater),
            Ordering::Equal => match (self.whole, other.whole) {
                (true, true) => Some(Ordering::Equal),
                (true, false) => Some(Ordering::Greater),
                (false, true) => Some(Ordering::Less),
                (false, false) => None,
            },
        }
    }
}

struct BoundsGatherer {
    order: Order,
    /// Per closed group, **as a merge needs it rather than as it is stored**:
    /// a keyed group's stored bounds, and a bytewise group's two extremes'
    /// heads ([`Clipped`]), `min_exact` and `max_exact` saying whether each
    /// head is its whole value — stored bounds only once [`Self::finish`]
    /// clips them, since a clipped upper bound no longer orders as its value
    /// does (`docs/design/decisions.md`, "D82"). **Over the values every
    /// layer's type holds**: a value past a tier is kept apart, in
    /// [`Self::tiers`].
    groups: Vec<Option<Bounds>>,
    /// Per closed group, [`LOST`].
    flags: Vec<u8>,
    /// Per closed group, its values past each tier as `groups` and `flags`
    /// hold them, `None` for a group holding none — **empty until a group
    /// holds one**, then one per closed group, so a column never meeting such
    /// a value pays nothing for the views (`docs/design/decisions.md`, "D97").
    tiers: Vec<Option<Box<ClosedTiers>>>,
    /// The text every entry of `groups` and `tiers` holds, summed as they are
    /// pushed, and each boxed entry of `tiers`.
    stored: u64,
    rows: ViewOrders,
}

/// One closed group's values past each tier, in [`tier_slot`] order, as
/// [`BoundsGatherer::groups`] and [`BoundsGatherer::flags`] hold a group's:
/// `(None, 0)` for a tier it holds no value past.
type ClosedTiers = [(Option<Bounds>, u8); 2];

/// A closed group held a value its bounds could not cover, so it has none —
/// where a group with `None` and no flag held no non-NULL value.
const LOST: u8 = 1;

/// Where a tier's extremes sit among a group's: the engine's, then the
/// format spec's.
fn tier_slot(tier: UnrepresentableTier) -> usize {
    match tier {
        UnrepresentableTier::Engine => 0,
        UnrepresentableTier::Format => 1,
    }
}

/// One group's running bounds.
enum GroupBounds {
    Bytewise { min: Option<Clipped>, max: Option<Clipped>, lost: bool },
    Keyed(KeyedGroup),
}

/// A keyed group's running bounds: over the values every layer holds, and
/// apart from them each tier's, which only a group holding one allocates.
#[derive(Default)]
struct KeyedGroup {
    values: Extremes,
    tiers: Option<Box<[Extremes; 2]>>,
}

/// The extremes of some keyed values, each with its text.
#[derive(Default)]
struct Extremes {
    min: Option<(ValueKey, String)>,
    max: Option<(ValueKey, String)>,
    lost: bool,
}

impl Extremes {
    fn observe(&mut self, key: &ValueKey, text: &str) {
        if self.min.as_ref().is_none_or(|(m, _)| key.stored_order(m) == Ordering::Less) {
            self.min = Some((key.clone(), text.to_owned()));
        }
        if self.max.as_ref().is_none_or(|(m, _)| key.stored_order(m) == Ordering::Greater) {
            self.max = Some((key.clone(), text.to_owned()));
        }
    }

    /// Fold `later`, the extremes of the values following these. The earliest
    /// of equal keys is kept, as a pass does.
    fn absorb(&mut self, later: Extremes) {
        self.lost |= later.lost;
        if let Some(value) = later.min
            && self.min.as_ref().is_none_or(|(m, _)| value.0.stored_order(m) == Ordering::Less)
        {
            self.min = Some(value);
        }
        if let Some(value) = later.max
            && self.max.as_ref().is_none_or(|(m, _)| value.0.stored_order(m) == Ordering::Greater)
        {
            self.max = Some(value);
        }
    }

    fn heap_bytes(&self) -> u64 {
        [&self.min, &self.max]
            .into_iter()
            .flatten()
            .map(|(key, text)| key.heap_bytes() + text_heap(text))
            .sum()
    }

    fn closed(self) -> (Option<Bounds>, u8) {
        match self {
            Extremes { lost: false, min: Some((_, min)), max: Some((_, max)) } => {
                (Some(Bounds { min, max, min_exact: true, max_exact: true }), 0)
            }
            Extremes { lost, .. } => (None, if lost { LOST } else { 0 }),
        }
    }

    /// Closed extremes as they stood before they closed.
    fn reopen(kind: &CompareKind, bounds: Option<Bounds>, flags: u8) -> Self {
        match bounds {
            Some(Bounds { min, max, .. }) => {
                let key = |text: &str| ValueKey::of(kind, text).expect("a stored bound keyed");
                Extremes { min: Some((key(&min), min)), max: Some((key(&max), max)), lost: false }
            }
            None => Extremes { lost: flags & LOST != 0, ..Extremes::default() },
        }
    }
}

impl KeyedGroup {
    /// The extremes a value past `tier` joins — a value no tier's joining
    /// [`Self::values`].
    fn extremes(&mut self, tier: Option<UnrepresentableTier>) -> &mut Extremes {
        match tier {
            None => &mut self.values,
            Some(tier) => &mut self.tiers.get_or_insert_with(Box::default)[tier_slot(tier)],
        }
    }

    fn absorb(&mut self, later: KeyedGroup) {
        self.values.absorb(later.values);
        let Some(theirs) = later.tiers else { return };
        match &mut self.tiers {
            Some(mine) => {
                let [engine, format] = *theirs;
                mine[0].absorb(engine);
                mine[1].absorb(format);
            }
            None => self.tiers = Some(theirs),
        }
    }

    fn heap_bytes(&self) -> u64 {
        let tiers = self.tiers.as_ref().map_or(0, |tiers| {
            size_of::<[Extremes; 2]>() as u64 + tiers.iter().map(Extremes::heap_bytes).sum::<u64>()
        });
        self.values.heap_bytes() + tiers
    }
}

impl GroupBounds {
    /// Fold `later`, the same group's bounds over its following rows. Keyed
    /// extremes keep the earliest of equal keys, as a pass does; bytewise ones
    /// are placed by their heads alone ([`Clipped`]).
    fn absorb(&mut self, later: GroupBounds) {
        match (self, later) {
            (
                Self::Bytewise { min, max, lost },
                Self::Bytewise { min: later_min, max: later_max, lost: later_lost },
            ) => {
                *lost |= later_lost;
                if let Some(value) = later_min
                    && min.as_ref().is_none_or(|m| m.order(&value) == Some(Ordering::Less))
                {
                    *min = Some(value);
                }
                if let Some(value) = later_max
                    && max.as_ref().is_none_or(|m| m.order(&value) == Some(Ordering::Greater))
                {
                    *max = Some(value);
                }
            }
            (Self::Keyed(mine), Self::Keyed(later)) => mine.absorb(later),
            _ => unreachable!("one column's groups keep one kind of bounds"),
        }
    }
}

/// A column's row order over the rows observed so far: the block's
/// [`Sortedness`] once every row is in.
#[derive(Clone)]
struct RowOrder {
    /// The first value placed, carried to a join as much as places it against
    /// the value before it.
    first: Option<FirstValue>,
    /// The last value placed.
    previous: Option<Previous>,
    never_decreased: bool,
    never_increased: bool,
    /// A value the order cannot place was seen.
    lost: bool,
}

/// A column's first value, kept for the join.
#[derive(Clone)]
enum FirstValue {
    /// Its length and its first [`CLIP_BYTES`] bytes, cut at a byte rather
    /// than a character: [`Clipped::locate_bytes`] reads no more.
    Bytewise {
        prefix: Vec<u8>,
        len: usize,
    },
    Keyed(ValueKey),
}

#[derive(Clone)]
enum Previous {
    Bytewise(Clipped),
    Keyed(ValueKey),
}

impl Default for RowOrder {
    fn default() -> Self {
        Self {
            first: None,
            previous: None,
            never_decreased: true,
            never_increased: true,
            lost: false,
        }
    }
}

impl RowOrder {
    /// The heap the first and the last value placed hold.
    fn heap_bytes(&self) -> u64 {
        let first = match &self.first {
            None => 0,
            Some(FirstValue::Bytewise { prefix, .. }) => vec_heap(prefix),
            Some(FirstValue::Keyed(key)) => key.heap_bytes(),
        };
        let previous = match &self.previous {
            None => 0,
            Some(Previous::Bytewise(clipped)) => text_heap(&clipped.head),
            Some(Previous::Keyed(key)) => key.heap_bytes(),
        };
        first + previous
    }

    /// Fold one step of the row order: `step` is where the value orders
    /// against the previous one, `None` where that is unknown.
    fn step(&mut self, step: Option<Ordering>) {
        match step {
            Some(Ordering::Less) => self.never_decreased = false,
            Some(Ordering::Greater) => self.never_increased = false,
            Some(Ordering::Equal) => {}
            None => self.lost = true,
        }
    }

    /// Place a keyed value after the one placed before it.
    fn place_keyed(&mut self, key: ValueKey) {
        let step = match &self.previous {
            Some(Previous::Keyed(previous)) => Some(key.stored_order(previous)),
            Some(Previous::Bytewise(_)) => unreachable!("a keyed order places no head"),
            None => {
                self.first = Some(FirstValue::Keyed(key.clone()));
                None
            }
        };
        self.previous = Some(Previous::Keyed(key));
        if let Some(step) = step {
            self.step(Some(step));
        }
    }

    /// Fold `later`, the order over the rows following these: one step across
    /// the join, then its own.
    fn absorb(&mut self, later: RowOrder) {
        if let (Some(previous), Some(first)) = (&self.previous, &later.first) {
            let step = match (previous, first) {
                (Previous::Bytewise(previous), FirstValue::Bytewise { prefix, len }) => {
                    previous.locate_bytes(prefix, *len)
                }
                (Previous::Keyed(previous), FirstValue::Keyed(first)) => {
                    Some(first.stored_order(previous))
                }
                _ => unreachable!("one column's values are ordered one way"),
            };
            self.step(step);
        }
        self.never_decreased &= later.never_decreased;
        self.never_increased &= later.never_increased;
        self.lost |= later.lost;
        if self.first.is_none() {
            self.first = later.first;
        }
        if later.previous.is_some() {
            self.previous = later.previous;
        }
    }

    fn sortedness(&self) -> Sortedness {
        if self.lost {
            Sortedness::Unsorted
        } else if self.never_decreased {
            Sortedness::Ascending
        } else if self.never_increased {
            Sortedness::Descending
        } else {
            Sortedness::Unsorted
        }
    }
}

/// A column's row order in each view of its values
/// (`crate::statistics::StatisticsView`): over the values every layer holds,
/// and — **once the rows observed hold a value past a tier, and not before** —
/// over those and the engine's tier, and over every value. Until then the
/// three are one order, which is what a fork copies.
#[derive(Default)]
struct ViewOrders {
    plain: RowOrder,
    /// Over the values the format spec holds, then over every value.
    wider: Option<Box<[RowOrder; 2]>>,
}

impl ViewOrders {
    fn fork(&mut self) -> &mut [RowOrder; 2] {
        let plain = &self.plain;
        self.wider.get_or_insert_with(|| Box::new([plain.clone(), plain.clone()]))
    }

    /// Place `key`, a value past `tier`, in each order that takes it.
    fn place_keyed(&mut self, key: ValueKey, tier: Option<UnrepresentableTier>) {
        match tier {
            None => {
                if let Some([representable, every]) = self.wider.as_deref_mut() {
                    representable.place_keyed(key.clone());
                    every.place_keyed(key.clone());
                }
                self.plain.place_keyed(key);
            }
            Some(UnrepresentableTier::Engine) => {
                let [representable, every] = self.fork();
                representable.place_keyed(key.clone());
                every.place_keyed(key);
            }
            Some(UnrepresentableTier::Format) => self.fork()[1].place_keyed(key),
        }
    }

    /// A value past `tier` no order that takes it can place.
    fn lose(&mut self, tier: Option<UnrepresentableTier>) {
        match tier {
            None => {
                self.plain.lost = true;
                for order in self.wider.iter_mut().flat_map(|wider| wider.iter_mut()) {
                    order.lost = true;
                }
            }
            Some(UnrepresentableTier::Engine) => {
                for order in self.fork() {
                    order.lost = true;
                }
            }
            Some(UnrepresentableTier::Format) => self.fork()[1].lost = true,
        }
    }

    fn absorb(&mut self, mut later: ViewOrders) {
        if self.wider.is_some() || later.wider.is_some() {
            self.fork();
            later.fork();
        }
        self.plain.absorb(later.plain);
        if let (Some(mine), Some(theirs)) = (&mut self.wider, later.wider) {
            let [representable, every] = *theirs;
            mine[0].absorb(representable);
            mine[1].absorb(every);
        }
    }

    fn heap_bytes(&self) -> u64 {
        let wider = self.wider.as_ref().map_or(0, |wider| {
            size_of::<[RowOrder; 2]>() as u64 + wider.iter().map(RowOrder::heap_bytes).sum::<u64>()
        });
        self.plain.heap_bytes() + wider
    }

    /// Each view's sortedness: over the values the format spec holds, over
    /// every value, and over those the engine holds as well.
    fn sortedness(&self) -> [Sortedness; 3] {
        let plain = self.plain.sortedness();
        match self.wider.as_deref() {
            None => [plain; 3],
            Some([representable, every]) => [representable.sortedness(), every.sortedness(), plain],
        }
    }
}

impl BoundsGatherer {
    fn new(kind: CompareKind) -> Self {
        let order = match kind {
            CompareKind::Text => Order::Bytewise(Canonical::Text),
            CompareKind::PaddedText => Order::Bytewise(Canonical::PaddedText),
            CompareKind::Bytea => Order::Bytewise(Canonical::Bytea),
            kind => Order::Keyed(kind),
        };
        Self::of(order)
    }

    fn of(order: Order) -> Self {
        Self {
            order,
            groups: Vec::new(),
            flags: Vec::new(),
            tiers: Vec::new(),
            stored: 0,
            rows: ViewOrders::default(),
        }
    }

    fn fresh(&self) -> Self {
        Self::of(self.order.clone())
    }

    fn fresh_group(&self) -> GroupBounds {
        match self.order {
            Order::Bytewise(_) => GroupBounds::Bytewise { min: None, max: None, lost: false },
            Order::Keyed(_) => GroupBounds::Keyed(KeyedGroup::default()),
        }
    }

    /// A value past `tier` that does not key, or that no stored bound could
    /// cover: no bounds for its group, and no order for its block, in each
    /// view that takes it.
    fn lose_value(&mut self, group: &mut GroupBounds, tier: Option<UnrepresentableTier>) {
        match group {
            GroupBounds::Bytewise { lost, .. } => *lost = true,
            GroupBounds::Keyed(group) => group.extremes(tier).lost = true,
        }
        self.rows.lose(tier);
    }

    /// Observe `text`, a value past `tier` where it is past one — which only a
    /// keyed kind's column can hold.
    fn observe(&mut self, group: &mut GroupBounds, text: &str, tier: Option<UnrepresentableTier>) {
        match &self.order {
            Order::Bytewise(canonical) => {
                debug_assert!(tier.is_none(), "a bytewise kind's type holds every value");
                let Some(text) = canonical.of(text) else { return self.lose_value(group, None) };
                let text = text.as_ref();
                let GroupBounds::Bytewise { min, max, .. } = group else {
                    unreachable!("a bytewise order keeps bytewise bounds")
                };
                if min.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Less)) {
                    *min = Some(Clipped::of(text));
                }
                if max.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Greater)) {
                    *max = Some(Clipped::of(text));
                }
                let rows = &mut self.rows.plain;
                let step = match &rows.previous {
                    Some(Previous::Bytewise(previous)) => Some(previous.locate(text)),
                    Some(Previous::Keyed(_)) => unreachable!("a bytewise order places no key"),
                    None => {
                        let prefix = text.as_bytes()[..text.len().min(CLIP_BYTES)].to_vec();
                        rows.first = Some(FirstValue::Bytewise { prefix, len: text.len() });
                        None
                    }
                };
                rows.previous = Some(Previous::Bytewise(Clipped::of(text)));
                if let Some(step) = step {
                    rows.step(step);
                }
            }
            Order::Keyed(kind) => {
                if text.len() > DICTIONARY_ENTRY_MAX_BYTES {
                    return self.lose_value(group, tier);
                }
                let Some(key) = ValueKey::of(kind, text) else {
                    return self.lose_value(group, tier);
                };
                let GroupBounds::Keyed(group) = group else {
                    unreachable!("a keyed order keeps keyed bounds")
                };
                group.extremes(tier).observe(&key, text);
                self.rows.place_keyed(key, tier);
            }
        }
    }

    fn close_group(&mut self, group: GroupBounds, charge: &mut Charge) {
        let (bounds, flags, tiers) = closed(group);
        self.stored += closed_heap(&bounds) + tiers_heap(&tiers);
        push_charged(&mut self.groups, bounds, charge);
        push_charged(&mut self.flags, flags, charge);
        if tiers.is_some() || !self.tiers.is_empty() {
            let wanted = self.groups.len() - self.tiers.len();
            reserve_charged(&mut self.tiers, wanted, charge);
            self.tiers.resize_with(self.groups.len() - 1, || None);
            self.tiers.push(tiers);
        }
    }

    /// A closed group's running bounds again, as they stood when it closed.
    fn reopen(
        &self,
        bounds: Option<Bounds>,
        flags: u8,
        tiers: Option<Box<ClosedTiers>>,
    ) -> GroupBounds {
        match &self.order {
            Order::Bytewise(_) => match bounds {
                Some(Bounds { min, max, min_exact, max_exact }) => GroupBounds::Bytewise {
                    min: Some(Clipped { head: min, whole: min_exact }),
                    max: Some(Clipped { head: max, whole: max_exact }),
                    lost: false,
                },
                None => GroupBounds::Bytewise { min: None, max: None, lost: flags & LOST != 0 },
            },
            Order::Keyed(kind) => GroupBounds::Keyed(KeyedGroup {
                values: Extremes::reopen(kind, bounds, flags),
                tiers: tiers.map(|tiers| {
                    Box::new((*tiers).map(|(bounds, flags)| Extremes::reopen(kind, bounds, flags)))
                }),
            }),
        }
    }

    /// Closed group `index`, taken out of the lists, which keep a hole.
    fn take_closed(&mut self, index: usize) -> (Option<Bounds>, u8, Option<Box<ClosedTiers>>) {
        let tiers = self.tiers.get_mut(index).and_then(Option::take);
        (self.groups[index].take(), mem::take(&mut self.flags[index]), tiers)
    }

    /// The last closed group's running bounds, no longer listed.
    fn reopen_last(&mut self) -> GroupBounds {
        let bounds = self.groups.pop().expect("a closed group lists its bounds");
        let flags = self.flags.pop().expect("a closed group flags its bounds");
        let tiers = self.tiers.pop().flatten();
        self.stored -= closed_heap(&bounds) + tiers_heap(&tiers);
        self.reopen(bounds, flags, tiers)
    }

    /// Merge every closed group with the one after it, as one pass over both
    /// groups' values closes them.
    fn merge_pairs(&mut self) {
        let closed_groups = self.groups.len();
        let merged = closed_groups.div_ceil(2);
        let tiered = !self.tiers.is_empty();
        for j in 0..merged {
            let first = self.take_closed(2 * j);
            let (bounds, flags, tiers) = if 2 * j + 1 < closed_groups {
                let second = self.take_closed(2 * j + 1);
                let mut group = self.reopen(first.0, first.1, first.2);
                group.absorb(self.reopen(second.0, second.1, second.2));
                closed(group)
            } else {
                first
            };
            (self.groups[j], self.flags[j]) = (bounds, flags);
            if tiered {
                self.tiers[j] = tiers;
            }
        }
        self.groups.truncate(merged);
        self.flags.truncate(merged);
        if tiered {
            self.tiers.truncate(merged);
        }
        self.stored = self.groups.iter().map(closed_heap).sum::<u64>()
            + self.tiers.iter().map(tiers_heap).sum::<u64>();
    }

    /// Append `later`'s tiers, which follow this gatherer's, ahead of its
    /// groups: both lists keep one per closed group where either holds any.
    fn append_tiers(&mut self, later: &mut BoundsGatherer, charge: &mut Charge) {
        if self.tiers.is_empty() && later.tiers.is_empty() {
            return;
        }
        let total = self.groups.len() + later.groups.len();
        let wanted = total - self.tiers.len();
        reserve_charged(&mut self.tiers, wanted, charge);
        self.tiers.resize_with(self.groups.len(), || None);
        if later.tiers.is_empty() {
            self.tiers.resize_with(total, || None);
        } else {
            self.tiers.append(&mut later.tiers);
        }
        later.tiers = Vec::new();
    }

    fn finish(self) -> ColumnBounds {
        let [sortedness, every_order, displayable_order] = self.rows.sortedness();
        let mut groups = self.groups;
        match &self.order {
            Order::Bytewise(canonical) => {
                for group in groups.iter_mut() {
                    if let Some(Bounds { min, max, min_exact, max_exact }) = group.take() {
                        let min = Clipped { head: min, whole: min_exact };
                        *group = clipped_bounds(
                            *canonical,
                            min,
                            Clipped { head: max, whole: max_exact },
                        );
                    }
                }
                ColumnBounds { sortedness, groups, every: None, displayable: None }
            }
            Order::Keyed(_) if self.tiers.is_empty() => {
                ColumnBounds { sortedness, groups, every: None, displayable: None }
            }
            Order::Keyed(kind) => {
                let seen =
                    |(bounds, flags): &(Option<Bounds>, u8)| bounds.is_some() || flags & LOST != 0;
                let (mut every, mut displayable) = (Vec::new(), Vec::new());
                let (mut any_every, mut any_displayable) = (false, false);
                let mut flags = self.flags.into_iter();
                for (group, tiers) in groups.iter_mut().zip(self.tiers) {
                    let base_flags = flags.next().expect("a closed group flags its bounds");
                    let Some(tiers) = tiers else {
                        every.push(None);
                        displayable.push(None);
                        continue;
                    };
                    let [engine, format] = *tiers;
                    let (engine_seen, format_seen) = (seen(&engine), seen(&format));
                    let values = group.take();
                    let mut representable = Extremes::reopen(kind, values.clone(), base_flags);
                    representable.absorb(Extremes::reopen(kind, engine.0, engine.1));
                    let (representable, representable_flags) = representable.closed();
                    displayable.push(values.filter(|_| engine_seen));
                    any_displayable |= engine_seen;
                    every.push(
                        format_seen
                            .then(|| {
                                let mut all = Extremes::reopen(
                                    kind,
                                    representable.clone(),
                                    representable_flags,
                                );
                                all.absorb(Extremes::reopen(kind, format.0, format.1));
                                all.closed().0
                            })
                            .flatten(),
                    );
                    any_every |= format_seen;
                    *group = representable;
                }
                let view = |any: bool, sortedness, groups| {
                    any.then_some(BoundsView { sortedness, groups })
                };
                ColumnBounds {
                    sortedness,
                    every: view(any_every, every_order, every),
                    displayable: view(any_displayable, displayable_order, displayable),
                    groups,
                }
            }
        }
    }
}

/// The text a closed group's bounds hold.
fn closed_heap(bounds: &Option<Bounds>) -> u64 {
    bounds.as_ref().map_or(0, |b| text_heap(&b.min) + text_heap(&b.max))
}

/// The heap a closed group's tiers hold: their box and their bounds' text.
fn tiers_heap(tiers: &Option<Box<ClosedTiers>>) -> u64 {
    tiers.as_ref().map_or(0, |tiers| {
        size_of::<ClosedTiers>() as u64
            + tiers.iter().map(|(bounds, _)| closed_heap(bounds)).sum::<u64>()
    })
}

/// The bounds one group of `values` is stored with under `kind`'s order, or
/// `None` where it keeps none — for a test outside this module that holds
/// stored bounds against another order.
#[cfg(test)]
pub(crate) fn one_group_bounds(kind: CompareKind, values: &[&str]) -> Option<Bounds> {
    let mut gatherer = BoundsGatherer::new(kind);
    let mut group = gatherer.fresh_group();
    for value in values {
        gatherer.observe(&mut group, value, None);
    }
    let mut charge = Charge::new(Arc::default(), Term::Gathering);
    gatherer.close_group(group, &mut charge);
    gatherer.finish().groups.pop().flatten()
}

/// A closed group's bounds as [`BoundsGatherer::groups`],
/// [`BoundsGatherer::flags`] and [`BoundsGatherer::tiers`] hold them.
fn closed(group: GroupBounds) -> (Option<Bounds>, u8, Option<Box<ClosedTiers>>) {
    match group {
        GroupBounds::Bytewise { lost: false, min: Some(min), max: Some(max) } => {
            let (min_exact, max_exact) = (min.whole, max.whole);
            (Some(Bounds { min: min.head, max: max.head, min_exact, max_exact }), 0, None)
        }
        GroupBounds::Bytewise { lost, .. } => (None, if lost { LOST } else { 0 }, None),
        GroupBounds::Keyed(KeyedGroup { values, tiers }) => {
            let (bounds, flags) = values.closed();
            (bounds, flags, tiers.map(|tiers| Box::new((*tiers).map(Extremes::closed))))
        }
    }
}

/// Merge every entry of `items` with the one after it, a last one alone, in
/// place.
fn merge_adjacent<T: Default>(items: &mut Vec<T>, mut merge: impl FnMut(T, T) -> T) {
    let len = items.len();
    for j in 0..len.div_ceil(2) {
        let first = mem::take(&mut items[2 * j]);
        let pair = match items.get_mut(2 * j + 1) {
            Some(second) => merge(first, mem::take(second)),
            None => first,
        };
        items[j] = pair;
    }
    items.truncate(len.div_ceil(2));
}

/// A bytewise group's bounds as stored: each exact where its value fits the
/// cap, otherwise a prefix below and a successor above.
fn clipped_bounds(canonical: Canonical, min: Clipped, max: Clipped) -> Option<Bounds> {
    let fits = |c: &Clipped| c.whole && c.head.len() <= DICTIONARY_ENTRY_MAX_BYTES;
    let min_exact = fits(&min);
    let lower = if min_exact {
        min.head
    } else {
        text_prefix(&min.head, DICTIONARY_ENTRY_MAX_BYTES).to_owned()
    };
    if fits(&max) {
        return Some(Bounds { min: lower, max: max.head, min_exact, max_exact: true });
    }
    let upper = match canonical {
        Canonical::Text => text_upper(&max.head, false)?,
        Canonical::PaddedText => text_upper(&max.head, true)?,
        Canonical::Bytea => bytea_upper(&max.head)?,
    };
    Some(Bounds { min: lower, max: upper, min_exact, max_exact: false })
}

/// The longest prefix of `text` no longer than `cap` bytes that ends on a
/// character boundary — a lower bound on `text` bytewise.
fn text_prefix(text: &str, cap: usize) -> &str {
    let mut end = cap.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A text no longer than [`DICTIONARY_ENTRY_MAX_BYTES`] ordering above `text` bytewise:
/// a prefix with its last character replaced by the next one. UTF-8 orders
/// bytewise as it orders code points, so the successor is above every text
/// the prefix begins. `padded` is `character`, whose comparison drops trailing
/// blanks first: the prefix gives its own up, and a successor that is a blank
/// is skipped. `None` when no character of the prefix has a successor.
fn text_upper(text: &str, padded: bool) -> Option<String> {
    // Room for a successor one byte longer than the character it replaces.
    let mut prefix: String =
        text_prefix(text, DICTIONARY_ENTRY_MAX_BYTES - char::MAX.len_utf8() + 1).to_owned();
    if padded {
        prefix.truncate(prefix.trim_end_matches(' ').len());
    }
    while let Some(last) = prefix.pop() {
        let Some(next) = successor(last) else { continue };
        let next = if padded && next == ' ' { '!' } else { next };
        prefix.push(next);
        return Some(prefix);
    }
    None
}

/// The next Unicode scalar value, skipping the surrogate range.
fn successor(c: char) -> Option<char> {
    match c {
        '\u{D7FF}' => Some('\u{E000}'),
        char::MAX => None,
        _ => char::from_u32(c as u32 + 1),
    }
}

/// The most bytes a rendered `\x` bound can carry within the cap.
const BYTEA_CAP_BYTES: usize = (DICTIONARY_ENTRY_MAX_BYTES - 2) / 2;

/// A rendered `bytea` above the value: its prefix with trailing `0xFF` bytes
/// dropped and the last byte incremented. `None` when every byte of that
/// prefix is `0xFF`.
fn bytea_upper(text: &str) -> Option<String> {
    let mut bytes = decode_bytea(text)?;
    bytes.truncate(BYTEA_CAP_BYTES);
    while bytes.last() == Some(&0xFF) {
        bytes.pop();
    }
    *bytes.last_mut()? += 1;
    Some(render_bytea(&bytes))
}

/// Make room in `vec` for `additional` more, **charging the growth ahead of
/// the allocation** ([`Charge::ahead`]): at least doubling, as the vector's
/// own growth would, but reserved exactly so the charge is the capacity.
fn reserve_charged<T>(vec: &mut Vec<T>, additional: usize, charge: &mut Charge) {
    let wanted = vec.len() + additional;
    let capacity = vec.capacity();
    if wanted <= capacity {
        return;
    }
    let grown = wanted.max(capacity * 2).max(4);
    let bytes = ((grown - capacity) * size_of::<T>()) as u64;
    charge.ahead((bytes, 0), (0, 0), || vec.reserve_exact(grown - vec.len()));
}

/// Push `item` onto `vec`, charging a growth it forces ahead of it.
fn push_charged<T>(vec: &mut Vec<T>, item: T, charge: &mut Charge) {
    reserve_charged(vec, 1, charge);
    vec.push(item);
}

/// The heap a `HashMap<K, V>` of `capacity` allocates, as the standard
/// library's table lays one out: a slot per bucket, then a control byte per
/// bucket and one probe group's worth past the end. The layout is the
/// toolchain's, not a guarantee (`docs/design/runtime-invariants.md`, "RT11").
fn map_heap<K, V>(capacity: usize) -> u64 {
    if capacity == 0 {
        return 0;
    }
    let buckets = if capacity < 8 { capacity + 1 } else { (capacity * 8 / 7).next_power_of_two() };
    table_heap::<K, V>(buckets)
}

/// The heap the table a full `HashMap<K, V>` of `capacity` grows into
/// allocates: the table sized for one more entry than it holds.
fn grown_map_heap<K, V>(capacity: usize) -> u64 {
    let wanted = capacity + 1;
    let buckets = if wanted < 15 {
        match wanted.max(3) {
            3 => 4,
            4..=7 => 8,
            _ => 16,
        }
    } else {
        (wanted * 8 / 7).next_power_of_two()
    };
    table_heap::<K, V>(buckets)
}

fn table_heap<K, V>(buckets: usize) -> u64 {
    let probe = if cfg!(any(target_arch = "x86", target_arch = "x86_64")) { 16 } else { 8 };
    let align = align_of::<(K, V)>().max(probe);
    let slots = (size_of::<(K, V)>() * buckets).next_multiple_of(align);
    (slots + buckets + probe) as u64
}

/// A column's dictionary. A `character` entry is its text without the
/// trailing blanks its comparison ignores, deduplicated and measured against
/// [`DICTIONARY_ENTRY_MAX_BYTES`] as such (`docs/design/decisions.md`, "D34").
struct DictionaryGatherer {
    /// Whether the column is `character`, whose entries give up their padding.
    padded: bool,
    entries: Vec<String>,
    /// The text every entry holds, summed as each is interned — and so the
    /// text the interning map's keys hold too.
    entry_text: u64,
    interned: HashMap<String, u32>,
    groups: Vec<Option<Vec<u32>>>,
    /// The heap every group's index list holds, summed as each is pushed.
    indices: u64,
}

impl DictionaryGatherer {
    fn new(kind: &CompareKind) -> Self {
        Self {
            padded: matches!(kind, CompareKind::PaddedText),
            entries: Vec::new(),
            entry_text: 0,
            interned: HashMap::new(),
            groups: Vec::new(),
            indices: 0,
        }
    }

    fn fresh(&self) -> Self {
        Self {
            padded: self.padded,
            entries: Vec::new(),
            entry_text: 0,
            interned: HashMap::new(),
            groups: Vec::new(),
            indices: 0,
        }
    }

    /// The heap this dictionary holds, as its entries and index lists and as
    /// its interning map.
    fn held(&self) -> (u64, u64) {
        let entries =
            vec_heap(&self.entries) + self.entry_text + vec_heap(&self.groups) + self.indices;
        (entries, map_heap::<String, u32>(self.interned.capacity()) + self.entry_text)
    }

    /// Add `text` to `group`'s distinct texts, which a cap passed leaves
    /// `None`.
    fn observe(&self, group: &mut GroupState, text: &str) {
        let Some(texts) = &mut group.texts else { return };
        let text = if self.padded { text.trim_end_matches(' ') } else { text };
        if texts.iter().any(|seen| seen == text) {
            return;
        }
        if text.len() > DICTIONARY_ENTRY_MAX_BYTES || texts.len() == DICTIONARY_MAX_ENTRIES {
            group.lose_texts();
            return;
        }
        let text = text.to_owned();
        group.text_bytes += text_heap(&text);
        texts.push(text);
    }

    /// **A map about to grow is charged its larger table before the insert
    /// allocates it**, and released of the smaller once the insert has freed
    /// it: the two are allocated at once while the table rehashes, and a
    /// close interning into several full maps would otherwise hold each
    /// growth uncharged until its observer next updates the account.
    fn intern(&mut self, text: &str, charge: &mut Charge) -> u32 {
        if let Some(&id) = self.interned.get(text) {
            return id;
        }
        let id = self.entries.len() as u32;
        push_charged(&mut self.entries, text.to_owned(), charge);
        let capacity = self.interned.capacity();
        let key = text.to_owned();
        if self.interned.len() == capacity {
            let grown = grown_map_heap::<String, u32>(capacity);
            let smaller = map_heap::<String, u32>(capacity);
            charge.ahead((0, grown), (0, smaller), || self.interned.insert(key, id));
        } else {
            self.interned.insert(key, id);
        }
        self.entry_text += text.len() as u64;
        id
    }

    fn close_group(&mut self, texts: Option<Vec<String>>, charge: &mut Charge) {
        let indices: Option<Vec<u32>> =
            texts.map(|texts| texts.iter().map(|text| self.intern(text, charge)).collect());
        self.indices += indices.as_ref().map_or(0, vec_heap);
        push_charged(&mut self.groups, indices, charge);
    }

    /// Append `later`'s groups, which follow this dictionary's, interning
    /// their entries here in the order a pass closing them would have —
    /// draining `later`'s index lists as they are re-interned, and moving
    /// `charge` by the pair's difference whenever what this one gained passes
    /// [`STATISTICS_ACCOUNT_CHARGE_STEP`]. `later`'s entries and map stay until it is dropped.
    fn append(&mut self, later: &mut DictionaryGatherer, charge: &mut Charge) {
        let pair = |mine: &Self, theirs: &Self| {
            let ((a, b), (c, d)) = (mine.held(), theirs.held());
            (a + c, b + d)
        };
        let before = pair(self, later);
        let base = charge.charged();
        let mut synced = self.entry_text * 2 + self.indices;
        for at in 0..later.groups.len() {
            let indices: Option<Vec<u32>> = later.groups[at].take().map(|ids| {
                later.indices -= vec_heap(&ids);
                ids.iter().map(|&id| self.intern(&later.entries[id as usize], charge)).collect()
            });
            self.indices += indices.as_ref().map_or(0, vec_heap);
            push_charged(&mut self.groups, indices, charge);
            let gained = self.entry_text * 2 + self.indices;
            if gained - synced >= STATISTICS_ACCOUNT_CHARGE_STEP {
                let now = pair(self, later);
                charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
                synced = gained;
            }
        }
        later.groups = Vec::new();
        let now = pair(self, later);
        charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
    }

    /// The last closed group's distinct texts, no longer listed.
    fn reopen_last(&mut self) -> Option<Vec<String>> {
        let ids = self.groups.pop().expect("a closed group lists its texts")?;
        self.indices -= vec_heap(&ids);
        Some(ids.iter().map(|&id| self.entries[id as usize].clone()).collect())
    }

    /// **Renumber the entries the groups still name in the order a pass
    /// closing these groups interns them, and let every other go**: a merge
    /// past [`DICTIONARY_MAX_ENTRIES`] leaves entries no group names, and a merged
    /// group lists its second half's new texts after its first's. The
    /// renumbering's scratch is charged ahead of it, and **the interning map
    /// is rebuilt rather than pruned**, its new table charged ahead and the old
    /// one released in one update with every text let go: a table pruned in
    /// place keeps its size while its reported capacity stops naming it
    /// (`docs/design/runtime-invariants.md`, "RT11").
    fn renumber(&mut self, charge: &mut Charge) {
        let scratch = (self.entries.len() * size_of::<u32>()) as u64;
        let mut to = charge.ahead((scratch, 0), (0, 0), || vec![u32::MAX; self.entries.len()]);
        let mut kept = 0u32;
        for id in self.groups.iter_mut().flatten().flatten() {
            let slot = &mut to[*id as usize];
            if *slot == u32::MAX {
                *slot = kept;
                kept += 1;
            }
            *id = *slot;
        }
        let old_table = map_heap::<String, u32>(self.interned.capacity());
        let new_table = match kept {
            0 => 0,
            kept => grown_map_heap::<String, u32>(kept as usize - 1),
        };
        charge.ahead_freeing((0, new_table), || {
            let mut interned = HashMap::with_capacity(kept as usize);
            for (key, id) in mem::take(&mut self.interned) {
                if to[id as usize] != u32::MAX {
                    interned.insert(key, to[id as usize]);
                }
            }
            self.interned = interned;
            // Every entry no group names goes past the kept ones, then the
            // permutation is applied in place, cycle by cycle.
            for (past, slot) in (kept..).zip(to.iter_mut().filter(|slot| **slot == u32::MAX)) {
                *slot = past;
            }
            for at in 0..to.len() {
                while to[at] as usize != at {
                    let there = to[at] as usize;
                    self.entries.swap(at, there);
                    to.swap(at, there);
                }
            }
            drop(to);
            let dropped: u64 =
                self.entries[kept as usize..].iter().map(|entry| entry.len() as u64).sum();
            self.entries.truncate(kept as usize);
            self.entry_text -= dropped;
            ((), (scratch + dropped, old_table + dropped))
        });
    }

    fn finish(self) -> ColumnDictionary {
        ColumnDictionary { entries: self.entries, groups: self.groups }
    }
}

/// One group's distinct texts, as indices, merged with the following group's
/// as one pass over both groups' values gathers them: the first's, then the
/// second's not already listed, and none past [`DICTIONARY_MAX_ENTRIES`].
fn union_indices(first: Option<Vec<u32>>, second: Option<Vec<u32>>) -> Option<Vec<u32>> {
    let (first, second) = (first?, second?);
    let added = second.iter().filter(|id| !first.contains(id)).count();
    if added == 0 {
        return Some(first);
    }
    if first.len() + added > DICTIONARY_MAX_ENTRIES {
        return None;
    }
    let mut union = Vec::with_capacity(first.len() + added);
    union.extend_from_slice(&first);
    union.extend(second.iter().filter(|id| !first.contains(id)));
    Some(union)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::StatisticsTerms;

    /// SplitMix64, seeded, so a failing case reproduces.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A value of `kind` drawn to collide: short and long, sharing long
    /// prefixes, padded, and — for text — past the ASCII range.
    fn value(rng: &mut Rng, kind: &CompareKind) -> String {
        let length = match rng.below(4) {
            0 => rng.below(4) as usize,
            1 => DICTIONARY_ENTRY_MAX_BYTES - 2 + rng.below(8) as usize,
            _ => 250 + rng.below(40) as usize,
        };
        let alphabet: &[char] = &['a', 'b', ' ', 'é', '\u{10FFFF}', '\u{1f}'];
        match kind {
            CompareKind::Int => (rng.below(20) as i64 - 10).to_string(),
            // Finite dates either side of the calendar's end, and the
            // infinities, which no format holds.
            CompareKind::Date => match rng.below(10) {
                0 => "infinity".into(),
                1 => "-infinity".into(),
                2 => format!("{}-01-0{}", 262142 + rng.below(3), 1 + rng.below(9)),
                _ => format!("20{:02}-0{}-1{}", rng.below(30), 1 + rng.below(9), rng.below(10)),
            },
            // As a date, and past `i64` microseconds from 1970 too, which
            // keys from PostgreSQL's epoch (I49).
            CompareKind::Timestamp { .. } => match rng.below(12) {
                0 => "infinity".into(),
                1 => "-infinity".into(),
                2 => format!("262143-01-0{} 00:00:00", 1 + rng.below(9)),
                3 => format!("29427{}-12-31 23:59:59", 1 + rng.below(5)),
                _ => format!("20{:02}-01-01 0{}:00:00", rng.below(30), rng.below(10)),
            },
            CompareKind::Bytea => {
                let bytes: Vec<u8> = (0..length / 2)
                    .map(|_| [0x00, 0x7f, 0xfe, 0xff][rng.below(4) as usize])
                    .collect();
                render_bytea(&bytes)
            }
            _ => {
                let mut text = String::new();
                while text.len() < length {
                    text.push(alphabet[rng.below(alphabet.len() as u64) as usize]);
                }
                text
            }
        }
    }

    /// **Every stored bound is on the right side of every value in its group,
    /// under the key a filter orders by, and a stated block order holds row by
    /// row** — over random groups of the three bytewise kinds and a keyed one,
    /// whose values collide on long shared prefixes past the cap.
    #[test]
    fn stored_bounds_and_order_hold_every_value_under_its_key() {
        let mut rng = Rng(0x5eed);
        let kinds =
            [CompareKind::Text, CompareKind::PaddedText, CompareKind::Bytea, CompareKind::Int];
        let (mut bounded, mut ordered) = (0usize, 0usize);
        let mut charge = Charge::new(Arc::default(), Term::Gathering);
        for round in 0..400 {
            let kind = &kinds[round % kinds.len()];
            let mut gatherer = BoundsGatherer::new(kind.clone());
            let base = value(&mut rng, kind);
            let groups: Vec<Vec<String>> = (0..1 + rng.below(4))
                .map(|_| {
                    (0..rng.below(6))
                        .map(|_| {
                            let v = value(&mut rng, kind);
                            // Half share a long prefix with one base value.
                            if rng.below(2) == 0
                                && !matches!(kind, CompareKind::Int | CompareKind::Bytea)
                            {
                                format!("{base}{v}")
                            } else {
                                v
                            }
                        })
                        .collect()
                })
                .collect();
            if round % 3 == 0 {
                // Sorted input, so a stated order is sometimes Ascending.
                let mut all: Vec<String> = groups.concat();
                all.sort_by(|a, b| {
                    ValueKey::of(kind, a).unwrap().compare(&ValueKey::of(kind, b).unwrap())
                });
                let mut state = gatherer.fresh_group();
                for v in &all {
                    gatherer.observe(&mut state, v, None);
                }
                gatherer.close_group(state, &mut charge);
                let column = gatherer.finish();
                assert_ne!(column.sortedness, Sortedness::Descending, "{kind:?}: sorted input");
                continue;
            }
            for group in &groups {
                let mut state = gatherer.fresh_group();
                for v in group {
                    gatherer.observe(&mut state, v, None);
                }
                gatherer.close_group(state, &mut charge);
            }
            let all: Vec<&String> = groups.iter().flatten().collect();
            let column = gatherer.finish();
            for (group, stored) in groups.iter().zip(&column.groups) {
                let Some(b) = stored else {
                    assert!(
                        group.is_empty() || matches!(kind, CompareKind::Int),
                        "{kind:?}: a group of {} values lost its bounds",
                        group.len()
                    );
                    continue;
                };
                bounded += 1;
                assert!(
                    b.min.len() <= DICTIONARY_ENTRY_MAX_BYTES
                        && b.max.len() <= DICTIONARY_ENTRY_MAX_BYTES
                );
                let low = ValueKey::of(kind, &b.min).expect("a stored min keys");
                let high = ValueKey::of(kind, &b.max).expect("a stored max keys");
                for v in group {
                    let key = ValueKey::of(kind, v).unwrap();
                    assert_ne!(
                        low.compare(&key),
                        Ordering::Greater,
                        "{kind:?}: min {:?} above {v:?}",
                        b.min
                    );
                    assert_ne!(
                        high.compare(&key),
                        Ordering::Less,
                        "{kind:?}: max {:?} below {v:?}",
                        b.max
                    );
                }
                if b.max_exact {
                    assert!(
                        group
                            .iter()
                            .any(|v| ValueKey::of(kind, v).unwrap().compare(&high)
                                == Ordering::Equal)
                    );
                }
            }
            let keys: Vec<ValueKey> = all.iter().map(|v| ValueKey::of(kind, v).unwrap()).collect();
            let pairs = keys.windows(2).map(|w| w[1].compare(&w[0]));
            match column.sortedness {
                Sortedness::Ascending => {
                    ordered += 1;
                    assert!(pairs.clone().all(Ordering::is_ge), "{kind:?}: not ascending");
                }
                Sortedness::Descending => {
                    ordered += 1;
                    assert!(pairs.clone().all(Ordering::is_le), "{kind:?}: not descending");
                }
                Sortedness::Unsorted => {}
            }
        }
        assert!(bounded > 300, "only {bounded} groups bounded");
        assert!(ordered > 50, "only {ordered} blocks ordered");
    }

    /// **Each view's stored bounds are the extremes of the values it takes,
    /// and its stated order holds over them row by row**, read back through
    /// [`ColumnStatistics::group_bounds`] and [`ColumnStatistics::sortedness`]
    /// — over random groups of dates and timestamps holding values past each
    /// tier, one past the format's not keying at all. A view kept for no
    /// group is kept for none.
    #[test]
    fn each_view_bounds_and_orders_the_values_it_takes() {
        use crate::statistics::{BoundsSet, StatisticsView};
        use crate::unrepresentable::scalar_tier;
        use UnrepresentableTier::Format;

        let takes = |view: StatisticsView, tier: Option<UnrepresentableTier>| match view {
            StatisticsView::Every => true,
            StatisticsView::Representable => tier != Some(Format),
            StatisticsView::Displayable => tier.is_none(),
        };
        let views =
            [StatisticsView::Every, StatisticsView::Representable, StatisticsView::Displayable];
        let kinds = [CompareKind::Date, CompareKind::Timestamp { with_tz: false }];
        let mut rng = Rng(0x7135);
        let (mut bounded, mut ordered, mut kept) = ([0usize; 3], [0usize; 3], [0usize; 2]);
        let mut charge = Charge::new(Arc::default(), Term::Gathering);
        for round in 0..900 {
            let kind = &kinds[round % kinds.len()];
            let tier = scalar_tier(&join_type(kind)).expect("a date or timestamp is tested");
            let mut values: Vec<String> =
                (0..rng.below(16)).map(|_| value(&mut rng, kind)).collect();
            if round % 3 == 0 {
                let key = |v: &String| ValueKey::of(kind, v);
                values.sort_by(|a, b| match (key(a), key(b)) {
                    (Some(a), Some(b)) => a.compare(&b),
                    (a, b) => a.is_none().cmp(&b.is_none()),
                });
                if rng.below(2) == 0 {
                    values.reverse();
                }
            }
            let mut groups: Vec<Vec<String>> = vec![Vec::new()];
            for v in values {
                if rng.below(4) == 0 {
                    groups.push(Vec::new());
                }
                groups.last_mut().unwrap().push(v);
            }
            let mut gatherer = BoundsGatherer::new(kind.clone());
            let mut counts = Vec::new();
            for group in &groups {
                let mut state = gatherer.fresh_group();
                let mut count = Unrepresentable::default();
                for v in group {
                    let past = tier.of(v);
                    if let Some(past) = past {
                        count.add(past);
                    }
                    gatherer.observe(&mut state, v, past);
                }
                gatherer.close_group(state, &mut charge);
                counts.push(count);
            }
            let bounds = gatherer.finish();
            assert_eq!(
                bounds.every.is_some(),
                counts.iter().any(|c| c.format > 0),
                "round {round}"
            );
            assert_eq!(bounds.displayable.is_some(), counts.iter().any(|c| c.engine > 0));
            kept[0] += usize::from(bounds.every.is_some());
            kept[1] += usize::from(bounds.displayable.is_some());
            let column = ColumnStatistics {
                declared_type: None,
                collation: None,
                null_counts: vec![0; groups.len()],
                bounds: Some(bounds),
                datafusion_bounds: None,
                dictionary: None,
                sums: None,
                value_bytes: vec![0; groups.len()],
                unrepresentable: Some(counts.clone()),
            };
            for (v, view) in views.into_iter().enumerate() {
                let taken = |group: &Vec<String>| -> Vec<(String, Option<ValueKey>)> {
                    group
                        .iter()
                        .filter(|value| takes(view, tier.of(value)))
                        .map(|value| (value.clone(), ValueKey::of(kind, value)))
                        .collect()
                };
                for (g, group) in groups.iter().enumerate() {
                    let what = format!("round {round}, {view:?}, group {g}: {group:?}");
                    let taken = taken(group);
                    let stored = column.group_bounds(BoundsSet::Primary, view, g);
                    let keys: Option<Vec<&ValueKey>> =
                        taken.iter().map(|(_, key)| key.as_ref()).collect();
                    let Some(keys) = keys.filter(|keys| !keys.is_empty()) else {
                        assert!(stored.is_none(), "{what}: no value to bound, or one unkeyed");
                        continue;
                    };
                    let b = stored.unwrap_or_else(|| panic!("{what}: keyed values unbounded"));
                    bounded[v] += 1;
                    let (low, high) =
                        (ValueKey::of(kind, &b.min).unwrap(), ValueKey::of(kind, &b.max).unwrap());
                    assert!(
                        keys.iter()
                            .all(|key| low.compare(key).is_le() && high.compare(key).is_ge()),
                        "{what}: {b:?}"
                    );
                    assert!(keys.iter().any(|key| low.compare(key).is_eq()), "{what}: min {b:?}");
                    assert!(keys.iter().any(|key| high.compare(key).is_eq()), "{what}: max {b:?}");
                    assert_eq!(
                        column.null_count(g, view),
                        Some(match view {
                            StatisticsView::Every => 0,
                            StatisticsView::Representable => counts[g].format,
                            StatisticsView::Displayable => counts[g].format + counts[g].engine,
                        }),
                        "{what}: NULLs"
                    );
                }
                let row: Vec<Option<ValueKey>> =
                    groups.iter().flat_map(taken).map(|(_, key)| key).collect();
                let expected =
                    match row.iter().map(Option::as_ref).collect::<Option<Vec<&ValueKey>>>() {
                        None => Sortedness::Unsorted,
                        Some(keys) if keys.windows(2).all(|w| w[1].compare(w[0]).is_ge()) => {
                            Sortedness::Ascending
                        }
                        Some(keys) if keys.windows(2).all(|w| w[1].compare(w[0]).is_le()) => {
                            Sortedness::Descending
                        }
                        Some(_) => Sortedness::Unsorted,
                    };
                let found = column.sortedness(BoundsSet::Primary, view);
                assert_eq!(found, Some(expected), "round {round}, {view:?}: {groups:?}");
                ordered[v] += usize::from(expected != Sortedness::Unsorted && row.len() > 3);
            }
        }
        assert!(bounded.iter().all(|&n| n > 600), "groups bounded per view: {bounded:?}");
        assert!(ordered.iter().all(|&n| n > 30), "blocks ordered per view: {ordered:?}");
        assert!(kept.iter().all(|&n| n > 200), "views kept: {kept:?}");
    }

    /// The kinds the join test's columns are compared by, a bytewise kind of
    /// each canonical form and three keyed ones, two of whose types cannot
    /// hold every value.
    const JOIN_KINDS: [CompareKind; 6] = [
        CompareKind::Text,
        CompareKind::PaddedText,
        CompareKind::Bytea,
        CompareKind::Int,
        CompareKind::Date,
        CompareKind::Timestamp { with_tz: false },
    ];

    /// The type the typed read emits a [`JOIN_KINDS`] column as.
    fn join_type(kind: &CompareKind) -> DataType {
        match kind {
            CompareKind::Int => DataType::Int64,
            CompareKind::Bytea => DataType::Binary,
            CompareKind::Date => DataType::Date32,
            CompareKind::Timestamp { .. } => {
                DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, None)
            }
            _ => DataType::Utf8View,
        }
    }

    /// A block's columns: one per [`JOIN_KINDS`], each keeping bounds, a
    /// dictionary and text bytes — the integer sums too, and the date and
    /// the timestamp count what their types cannot hold — and an untracked
    /// one.
    fn join_columns() -> Vec<Option<ColumnGatherer>> {
        let plan =
            |kind: &CompareKind| ComparisonPlan::Compared { kind: kind.clone(), divergence: None };
        JOIN_KINDS
            .iter()
            .map(|kind| {
                let plan = plan(kind);
                let bounds = plan.bounds_kinds();
                let data_type = join_type(kind);
                Some(ColumnGatherer::new(
                    None,
                    None,
                    bounds,
                    &plan,
                    &NestedPlan::Scalar,
                    &data_type,
                    crate::unrepresentable::scalar_tier(&data_type),
                ))
            })
            .chain([None])
            .collect()
    }

    /// One field as `COPY` writes it: a backslash doubled.
    fn escaped(text: &str) -> Vec<u8> {
        text.replace('\\', "\\\\").into_bytes()
    }

    /// One random block's rows as the mapping pass hands them over.
    struct RandomBlock {
        group_size: u64,
        lines: Vec<Vec<u8>>,
        offsets: Vec<u64>,
        end: u64,
        short: bool,
    }

    /// A random block whose groups a row can straddle or skip, whose bytewise
    /// values share heads past the cap, and which holds NULLs, values that do
    /// not key or are not text, dictionaries on both sides of their cap, and
    /// sorted columns. Every fourth round is short values in wide groups, so a
    /// dictionary on each side of a cut can pass the count cap between them.
    fn random_block(rng: &mut Rng, round: usize) -> RandomBlock {
        let short = round % 4 == 1;
        let group_size =
            if short { 4096 } else { [16u64, 64, 256, 700, 4096][rng.below(5) as usize] };
        let sorted = round.is_multiple_of(3);
        let rows = if short { 60 + rng.below(140) } else { rng.below(120) } as usize;
        // A small pool per column for some rounds, so dictionaries fit.
        let pool = 2 + rng.below(if round.is_multiple_of(2) { 6 } else { 200 });
        let columns: Vec<Vec<Option<Vec<u8>>>> = JOIN_KINDS
            .iter()
            .map(|kind| {
                let base = value(rng, kind);
                let values: Vec<String> = (0..pool)
                    .map(|_| {
                        let v = match kind {
                            CompareKind::Bytea if short => {
                                render_bytea(&(rng.below(1 << 16) as u16).to_be_bytes())
                            }
                            CompareKind::Date | CompareKind::Timestamp { .. } => value(rng, kind),
                            _ if short => rng.below(1000).to_string(),
                            _ => value(rng, kind),
                        };
                        match kind {
                            CompareKind::Text | CompareKind::PaddedText if rng.below(2) == 0 => {
                                format!("{base}{v}")
                            }
                            _ => v,
                        }
                    })
                    .collect();
                let mut drawn: Vec<String> =
                    (0..rows).map(|_| values[rng.below(pool) as usize].clone()).collect();
                if sorted {
                    // A value that does not key sorts last.
                    let key = |v: &String| ValueKey::of(kind, v);
                    drawn.sort_by(|a, b| match (key(a), key(b)) {
                        (Some(a), Some(b)) => a.compare(&b),
                        (a, b) => a.is_none().cmp(&b.is_none()),
                    });
                    if rng.below(2) == 0 {
                        drawn.reverse();
                    }
                }
                drawn
                    .into_iter()
                    .map(|v| match rng.below(40) {
                        0..=4 => None,
                        5 if !sorted && !short => Some(vec![0xff, b'a']),
                        6 if !sorted && matches!(kind, CompareKind::Int) => Some(b"x".to_vec()),
                        6 if !sorted && matches!(kind, CompareKind::Bytea) => {
                            Some(escaped("\\xABC"))
                        }
                        _ => Some(escaped(&v)),
                    })
                    .collect()
            })
            .collect();
        let lines: Vec<Vec<u8>> = (0..rows)
            .map(|r| {
                let mut line = Vec::new();
                for column in &columns {
                    line.extend(column[r].clone().unwrap_or_else(|| b"\\N".to_vec()));
                    line.push(b'\t');
                }
                line.extend(b"untracked");
                line
            })
            .collect();
        let mut offsets = Vec::with_capacity(rows);
        let mut end = 0u64;
        for line in &lines {
            offsets.push(end);
            end += line.len() as u64 + 1;
        }
        RandomBlock { group_size, lines, offsets, end, short }
    }

    /// Every observer's charge is released into the block it became, pieces
    /// included, and nothing else is left in the account.
    fn assert_only_retained(account: &StatisticsAccount, statistics: &BlockStatistics, at: &str) {
        let retained = statistics.heap_bytes();
        let expected = StatisticsTerms { retained, ..StatisticsTerms::default() };
        assert_eq!(account.held().now, expected, "{at}");
    }

    /// Gathering at `group_size`, merging past `cap` and short of `min_rows`,
    /// every block recording the same request so that only its statistics
    /// tell two apart.
    fn sized(group_size: u64, cap: Option<usize>, min_rows: Option<u64>) -> Sizing {
        bounded(group_size, cap, min_rows, None)
    }

    /// [`sized`], under a density maximum as well.
    fn bounded(
        group_size: u64,
        cap: Option<usize>,
        min_rows: Option<u64>,
        max_rows: Option<u64>,
    ) -> Sizing {
        Sizing { group_size, cap, min_rows, max_rows, record: GroupSizing::Stated }
    }

    /// `block` handed to one observer sized by `sizing`.
    fn gathered_serially(block: &RandomBlock, sizing: Sizing) -> BlockStatistics {
        let account = Arc::new(StatisticsAccount::default());
        let charge = Charge::new(Arc::clone(&account), Term::Gathering);
        let mut serial = Gatherer::block(sizing, join_columns(), charge);
        for (line, &offset) in block.lines.iter().zip(&block.offsets) {
            serial.observe_row(offset, line);
        }
        let statistics = Box::new(serial)
            .finish(block.end)
            .gathered()
            .expect("an unbounded account declines nothing");
        assert_only_retained(&account, &statistics, "serial");
        statistics
    }

    /// `block` handed to pieces cut at random, sized by `sizing`, folded in file order a window
    /// of pieces at a time — made before any of them is folded, as the leader
    /// makes them — with an empty piece now and then and one left unfolded
    /// past the block's end. Answers the statistics, and how many cuts fell
    /// inside a group.
    fn gathered_in_pieces(
        rng: &mut Rng,
        block: &RandomBlock,
        sizing: Sizing,
    ) -> (BlockStatistics, usize) {
        let group_size = sizing.group_size;
        let account = Arc::new(StatisticsAccount::default());
        let charge = Charge::new(Arc::clone(&account), Term::Gathering);
        let mut observer: Box<dyn BlockObserver> =
            Box::new(Gatherer::block(sizing, join_columns(), charge));
        let cut_odds = 1 + rng.below(30);
        let window_pieces = 1 + rng.below(4) as usize;
        let mut window = vec![observer.piece()];
        let mut straddles = 0;
        for (r, (line, &offset)) in block.lines.iter().zip(&block.offsets).enumerate() {
            if r > 0 && rng.below(cut_odds) == 0 {
                if window.len() == window_pieces {
                    for piece in window.drain(..) {
                        observer.absorb(piece);
                    }
                    if rng.below(4) == 0 {
                        observer.absorb(observer.piece());
                    }
                }
                window.push(observer.piece());
                // Only the rows' own groups say where a cut fell; the block
                // may since have merged, which moves no row between groups
                // it straddled at the base size.
                if block.offsets[r - 1] / group_size == offset / group_size {
                    straddles += 1;
                }
            }
            window.last_mut().expect("a window holds a piece").observe_row(offset, line);
        }
        for piece in window {
            observer.absorb(piece);
        }
        let past_the_end = (rng.below(2) == 0).then(|| observer.piece());
        let statistics =
            observer.finish(block.end).gathered().expect("an unbounded account declines nothing");
        drop(past_the_end);
        assert_only_retained(&account, &statistics, "pieces");
        (statistics, straddles)
    }

    /// **A block observed in pieces, joined in file order, gathers exactly what
    /// one observer handed every row gathers** — over [`random_block`]'s
    /// blocks, whose groups straddle the cuts. The fixture sweep does not
    /// guard a join's order step, a block of `pg_dump` output rarely turning
    /// on it: this test does.
    #[test]
    fn pieces_joined_in_file_order_gather_what_one_pass_gathers() {
        let mut rng = Rng(0x0001_0105);
        let (mut straddles, mut ordered, mut inexact, mut dictionaries, mut overflowed) =
            (0, 0, 0, 0, 0);
        let (mut summed, mut unsummed, mut measured) = (0, 0, 0);
        let mut views = [0; 2];
        for round in 0..600 {
            let block = random_block(&mut rng, round);
            let exact = sized(block.group_size, None, None);
            let serial = gathered_serially(&block, exact);
            let (joined, straddled) = gathered_in_pieces(&mut rng, &block, exact);
            straddles += straddled;
            assert_eq!(joined, serial, "round {round}");

            for (kind, column) in JOIN_KINDS.iter().zip(&joined.columns) {
                let Some(column) = column else { continue };
                let bounds = column.bounds.as_ref().unwrap();
                if bounds.sortedness != Sortedness::Unsorted && block.lines.len() > 10 {
                    ordered += 1;
                }
                inexact += bounds.groups.iter().flatten().filter(|b| !b.max_exact).count();
                if let Some(dictionary) = &column.dictionary {
                    dictionaries +=
                        dictionary.groups.iter().flatten().filter(|g| g.len() > 1).count();
                    if block.short {
                        overflowed += dictionary.groups.iter().filter(|g| g.is_none()).count();
                    }
                }
                if let Some(counts) = &column.unrepresentable {
                    assert_eq!(counts.len(), joined.groups.len(), "round {round}: a count a group");
                    views[0] += usize::from(bounds.every.is_some());
                    views[1] += usize::from(bounds.displayable.is_some());
                }
                match &column.sums {
                    Some(sums) if sums.iter().any(|&sum| sum != 0) => summed += 1,
                    None if *kind == CompareKind::Int => unsummed += 1,
                    _ => {}
                }
                measured += usize::from(column.value_bytes.iter().any(|&bytes| bytes > 0));
            }
        }
        assert!(summed > 100, "only {summed} columns summed");
        assert!(views[0] > 300, "only {} columns kept every value apart", views[0]);
        assert!(views[1] > 300, "only {} columns kept the displayable apart", views[1]);
        assert!(unsummed > 20, "only {unsummed} columns lost their sums to a value not their type");
        assert!(measured > 500, "only {measured} columns measured");
        assert!(straddles > 500, "only {straddles} cuts fell inside a group");
        assert!(ordered > 200, "only {ordered} columns ordered");
        assert!(inexact > 500, "only {inexact} truncated upper bounds");
        assert!(dictionaries > 500, "only {dictionaries} dictionaries of several entries");
        assert!(overflowed > 50, "only {overflowed} dictionaries past their count cap");
    }

    /// **A block past its cap gathers exactly what gathering at the size it
    /// reaches gathers**, handed its rows by one observer or by pieces folded
    /// in file order, and holds no more groups than the cap — over
    /// [`random_block`]'s blocks at caps of a handful of groups, so a block
    /// merges many times, with an odd count mid-scan, between windows of
    /// pieces and while a row skips groups.
    #[test]
    fn a_capped_block_gathers_what_the_size_it_reaches_gathers() {
        let mut rng = Rng(0x0020_0003);
        let (mut coarsened, mut dropped_entries, mut overflowed) = (0, 0, 0);
        for round in 0..600 {
            let block = random_block(&mut rng, round);
            let cap = 1 + rng.below(6) as usize;
            let sizing = sized(block.group_size, Some(cap), None);
            let capped = gathered_serially(&block, sizing);
            assert!(capped.groups.len() <= cap, "round {round}: {} groups", capped.groups.len());
            let exact = gathered_serially(&block, sized(capped.group_size, None, None));
            assert_eq!(capped, exact, "round {round}: serial at cap {cap}");
            let (joined, _) = gathered_in_pieces(&mut rng, &block, sizing);
            assert_eq!(joined, capped, "round {round}: pieces at cap {cap}");

            if capped.group_size > block.group_size {
                coarsened += 1;
                let base = gathered_serially(&block, sized(block.group_size, None, None));
                for (fine, coarse) in base.columns.iter().zip(&capped.columns) {
                    let (Some(fine), Some(coarse)) = (fine, coarse) else { continue };
                    let entries =
                        |c: &ColumnStatistics| c.dictionary.as_ref().unwrap().entries.len();
                    if entries(coarse) < entries(fine) {
                        dropped_entries += 1;
                    }
                    let lost = |c: &ColumnStatistics| {
                        c.dictionary.as_ref().unwrap().groups.iter().filter(|g| g.is_none()).count()
                    };
                    if lost(coarse) > 0 && lost(fine) == 0 {
                        overflowed += 1;
                    }
                }
            }
        }
        assert!(coarsened > 300, "only {coarsened} blocks merged");
        assert!(dropped_entries > 30, "only {dropped_entries} dictionaries let entries go");
        assert!(overflowed > 20, "only {overflowed} merged groups passed the count cap");
    }

    /// The size [`density_merges`] must choose, written the way
    /// `scripts/row_density.py` chooses it: every size from the gathered one
    /// to a single group, and the first whose upper middle group holds the
    /// minimum, else the single group.
    fn chosen_merges(rows: &[u64], min_rows: u64) -> u32 {
        if rows.is_empty() {
            return 0;
        }
        let mut ladder = vec![rows.to_vec()];
        while ladder.last().unwrap().len() > 1 {
            let last = ladder.last().unwrap();
            let coarser =
                (0..last.len()).step_by(2).map(|i| last[i..last.len().min(i + 2)].iter().sum());
            ladder.push(coarser.collect());
        }
        let median = |level: &Vec<u64>| {
            let mut sorted = level.clone();
            sorted.sort_unstable();
            sorted[sorted.len() / 2]
        };
        let reaches = ladder.iter().position(|level| median(level) >= min_rows);
        reaches.unwrap_or(ladder.len() - 1) as u32
    }

    /// **The density rule chooses what the ladder of sizes chooses**: the
    /// finest size whose upper middle group holds the minimum, and the single
    /// group where no size reaches it.
    #[test]
    fn density_merges_choose_the_finest_size_whose_median_holds_the_minimum() {
        assert_eq!(density_merges(vec![], 5, None), 0);
        assert_eq!(density_merges(vec![3], 5, None), 0, "one group is as coarse as a block gets");
        assert_eq!(density_merges(vec![5, 0, 5], 5, None), 0, "the second smallest of three");
        assert_eq!(density_merges(vec![0, 5, 0, 5], 5, None), 0, "the third smallest of four");
        assert_eq!(density_merges(vec![0, 0, 0, 5], 5, None), 1, "three of four fall short");
        assert_eq!(density_merges(vec![1, 1, 1, 1, 1], 5, None), 3, "never reached: one group");
        assert_eq!(
            density_merges(vec![9, 9, 9, 9], 0, None),
            0,
            "a minimum of zero merges nothing"
        );
        let mut rng = Rng(0x0020_0004);
        for round in 0..2000 {
            let groups = 1 + rng.below(40) as usize;
            let skew = 1 + rng.below(4);
            let rows: Vec<u64> =
                (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
            let min_rows = rng.below(30);
            assert_eq!(
                density_merges(rows.clone(), min_rows, None),
                chosen_merges(&rows, min_rows),
                "round {round}: {rows:?} at {min_rows}"
            );
        }
    }

    /// **The predicate is monotone in size**, which is what lets a block the
    /// cap has already coarsened read its minimum from the size the cap left:
    /// once the upper middle group holds the minimum, no coarser size falls
    /// back below it. The odd tail is the shape that breaks it for the
    /// nearest-rank median — `[m, m, m, m, 0, 0, 0]` meets the minimum, the
    /// pairs the cap would leave it do not, and the block cascades to one
    /// group — so it is asserted at both sizes, and the sweep holds every
    /// level of the ladder.
    #[test]
    fn the_density_predicate_is_monotone_in_size() {
        assert_eq!(density_merges(vec![5, 5, 5, 5, 0, 0, 0], 5, None), 0, "the odd tail, gathered");
        assert_eq!(density_merges(vec![10, 10, 0, 0], 5, None), 0, "the same block, once capped");
        let mut rng = Rng(0x0020_0011);
        let mut reached_early = 0;
        for round in 0..4000 {
            let groups = 1 + rng.below(40) as usize;
            let skew = 1 + rng.below(4);
            let rows: Vec<u64> =
                (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
            let min_rows = rng.below(30);
            let first = chosen_merges(&rows, min_rows) as usize;
            let mut level = rows.clone();
            for merges in 0..groups {
                // Every level at or past the first one that holds is asked
                // again from there, as a capped block's `fit_density` does.
                let from_here = density_merges(level.clone(), min_rows, None) as usize;
                assert_eq!(
                    merges + from_here,
                    first.max(merges),
                    "round {round}: {rows:?} at {min_rows}, from {merges} merges in"
                );
                level = level.chunks(2).map(|pair| pair.iter().sum()).collect();
            }
            reached_early += usize::from(first > 0 && first < groups);
        }
        assert!(reached_early > 500, "only {reached_early} rounds coarsened short of one group");
    }

    /// **A block sized by its density minimum gathers exactly what gathering
    /// at the size it reaches gathers**, handed its rows by one observer or by
    /// pieces folded in file order, with or without a cap ahead of it — and
    /// uncapped, that size is the one the ladder over the gathered size's rows
    /// chooses, so a block no merge reached is at the size it was gathered at.
    ///
    /// **Capped, the size is the coarser of the cap's and that one**, which is
    /// the guarantee the monotone predicate buys: a block the cap coarsened
    /// past the size its own base distribution reached keeps the cap's size,
    /// and one the cap left finer reaches exactly the size it would have
    /// uncapped.
    #[test]
    fn a_block_sized_by_its_minimum_gathers_what_the_size_it_reaches_gathers() {
        let mut rng = Rng(0x0020_0005);
        let (mut coarsened, mut after_cap, mut finer_than_cap) = (0, 0, 0);
        for round in 0..600 {
            let block = random_block(&mut rng, round);
            let min_rows = rng.below(40);
            let cap = (rng.below(2) == 0).then(|| 1 + rng.below(6) as usize);
            let sizing = sized(block.group_size, cap, Some(min_rows));
            let chosen = gathered_serially(&block, sizing);
            let exact = gathered_serially(&block, sized(chosen.group_size, None, None));
            assert_eq!(chosen, exact, "round {round}: serial at {min_rows} rows, cap {cap:?}");
            let (joined, _) = gathered_in_pieces(&mut rng, &block, sizing);
            assert_eq!(joined, chosen, "round {round}: pieces at {min_rows} rows, cap {cap:?}");

            let base = gathered_serially(&block, sized(block.group_size, None, None));
            let rows: Vec<u64> = base.groups.iter().map(|g| g.rows).collect();
            match cap {
                None => {
                    let merges = chosen_merges(&rows, min_rows);
                    assert_eq!(chosen.group_size, block.group_size << merges, "round {round}");
                    coarsened += usize::from(merges > 0);
                }
                Some(cap) => {
                    let capped =
                        gathered_serially(&block, sized(block.group_size, Some(cap), None));
                    let uncapped = block.group_size << chosen_merges(&rows, min_rows);
                    assert_eq!(
                        chosen.group_size,
                        uncapped.max(capped.group_size),
                        "round {round}: cap {cap}, {min_rows} rows"
                    );
                    after_cap += usize::from(chosen.group_size > capped.group_size);
                    finer_than_cap += usize::from(uncapped < capped.group_size);
                }
            }
        }
        assert!(coarsened > 100, "only {coarsened} uncapped blocks coarsened");
        assert!(after_cap > 30, "only {after_cap} capped blocks coarsened past their cap");
        assert!(finer_than_cap > 30, "only {finer_than_cap} capped blocks reached it finer");
    }

    /// The size [`density_merges`] must choose under a maximum as well as a
    /// minimum: the ladder walked as [`chosen_merges`] walks it, stopping at
    /// the last size whose 90th-percentile group is within the maximum.
    fn chosen_merges_between(rows: &[u64], min_rows: u64, max_rows: u64) -> u32 {
        let mut level = rows.to_vec();
        let mut merges = 0;
        while level.len() > 1 && min_rows_group(&level) < min_rows {
            let coarser: Vec<u64> = level.chunks(2).map(|pair| pair.iter().sum()).collect();
            if max_rows_group(&coarser) > max_rows {
                break;
            }
            (level, merges) = (coarser, merges + 1);
        }
        merges
    }

    /// **A stated maximum stops the merging, whatever the minimum asks.** A
    /// block already past the maximum at the size it holds, its groups pairing
    /// evenly, merges nothing — nothing here makes a block finer. Past the
    /// maximum, a size whose groups all pair stays past it at the next: of `G`
    /// groups, `⌊G/10⌋+1` past it leave at least `⌊G/20⌋+1` pairs past it. A
    /// size whose last group stands unpaired is exempt, and can let one merge
    /// through (`an_unpaired_last_group_lets_a_block_past_the_maximum_merge`).
    #[test]
    fn a_stated_maximum_stops_the_merging_whatever_the_minimum_asks() {
        assert_eq!(density_merges(vec![1, 1, 1, 1], 5, Some(2)), 1, "one merge fits, two do not");
        assert_eq!(density_merges(vec![1, 1, 1, 1], 5, Some(9)), 2, "the minimum is reached");
        assert_eq!(density_merges(vec![9, 9, 9, 9], 5, Some(2)), 0, "past the maximum already");
        assert_eq!(density_merges(vec![1, 1, 1, 1], 5, None), 2, "no maximum stops nothing");
        let mut rng = Rng(0x0020_0050);
        let (mut stopped, mut agreed_with_minimum) = (0, 0);
        for round in 0..4000 {
            let groups = 1 + rng.below(40) as usize;
            let skew = 1 + rng.below(4);
            let rows: Vec<u64> =
                (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
            let (min_rows, max_rows) = (rng.below(30), rng.below(60));
            let merges = density_merges(rows.clone(), min_rows, Some(max_rows));
            assert_eq!(
                merges,
                chosen_merges_between(&rows, min_rows, max_rows),
                "round {round}: {rows:?} between {min_rows} and {max_rows}"
            );
            let alone = chosen_merges(&rows, min_rows);
            assert!(merges <= alone, "round {round}: a maximum never merges further");
            stopped += usize::from(merges < alone);
            agreed_with_minimum += usize::from(merges == alone);
            // Monotone from every level past the maximum whose groups pair.
            let mut level = rows.clone();
            while level.len() > 1 {
                let coarser: Vec<u64> = level.chunks(2).map(|pair| pair.iter().sum()).collect();
                if level.len().is_multiple_of(2) && max_rows_group(&level) > max_rows {
                    assert!(
                        max_rows_group(&coarser) > max_rows,
                        "round {round}: {rows:?} came back within {max_rows} from {level:?}"
                    );
                }
                level = coarser;
            }
        }
        assert!(stopped > 300, "only {stopped} rounds were stopped by their maximum");
        assert!(agreed_with_minimum > 300, "only {agreed_with_minimum} rounds reached it");
    }

    /// **`KD43`, pinned**: nineteen groups, two of them past a maximum of 5,
    /// put the block past it, and the merge that pairs them leaves ten whose
    /// 90th-percentile group is empty, so the merge goes through and the block
    /// holds a group of 20. Closing the deficiency makes this block merge
    /// nothing, and flips the assertion.
    #[test]
    fn an_unpaired_last_group_lets_a_block_past_the_maximum_merge() {
        let rows: Vec<u64> = [10, 10].into_iter().chain([0; 17]).collect();
        assert!(max_rows_group(&rows) > 5, "the block is past the maximum as gathered");
        assert_eq!(density_merges(rows, 1, Some(5)), 1);
    }

    /// **A block sized between its bounds gathers exactly what gathering at
    /// the size it reaches gathers**, serially and by pieces, and that size is
    /// the one the ladder over its base distribution chooses — the maximum
    /// stopping where the minimum would have gone further.
    #[test]
    fn a_block_sized_between_its_bounds_gathers_what_the_size_it_reaches_gathers() {
        let mut rng = Rng(0x0020_0051);
        let (mut stopped, mut coarsened) = (0, 0);
        for round in 0..600 {
            let block = random_block(&mut rng, round);
            let (min_rows, max_rows) = (rng.below(40), 1 + rng.below(30));
            let sizing = bounded(block.group_size, None, Some(min_rows), Some(max_rows));
            let chosen = gathered_serially(&block, sizing);
            let exact = gathered_serially(&block, sized(chosen.group_size, None, None));
            assert_eq!(chosen, exact, "round {round}: serial between {min_rows} and {max_rows}");
            let (joined, _) = gathered_in_pieces(&mut rng, &block, sizing);
            assert_eq!(joined, chosen, "round {round}: pieces");

            let base = gathered_serially(&block, sized(block.group_size, None, None));
            let rows: Vec<u64> = base.groups.iter().map(|g| g.rows).collect();
            let merges = chosen_merges_between(&rows, min_rows, max_rows);
            assert_eq!(chosen.group_size, block.group_size << merges, "round {round}: {rows:?}");
            stopped += usize::from(merges < chosen_merges(&rows, min_rows));
            coarsened += usize::from(merges > 0);
        }
        assert!(stopped > 50, "only {stopped} blocks were stopped by their maximum");
        assert!(coarsened > 50, "only {coarsened} blocks coarsened at all");
    }

    /// Every row of `block` handed to one observer bounded by `allowance`,
    /// answering what it finished with and the account it charged.
    fn gathered_within(
        block: &RandomBlock,
        sizing: Sizing,
        allowance: Option<u64>,
    ) -> (BlockGathered, Arc<StatisticsAccount>) {
        let account = Arc::new(StatisticsAccount::bounded_by(allowance));
        let charge = Charge::new(Arc::clone(&account), Term::Gathering);
        let mut serial = Gatherer::block(sizing, join_columns(), charge);
        for (line, &offset) in block.lines.iter().zip(&block.offsets) {
            serial.observe_row(offset, line);
        }
        (Box::new(serial).finish(block.end), account)
    }

    /// **A block whose statistics pass the pass's allowance declines**: it
    /// answers the allowance rather than statistics, frees what it had
    /// gathered — the account is empty after it, and never reached the peak
    /// an unbounded pass over the block reaches — and the scan reads on
    /// (`docs/design/decisions.md`, "D85").
    ///
    /// **Not vacuous**: the same block, the same rows and an allowance that
    /// fits gathers exactly what an unbounded account gathers, so the bound
    /// and nothing else is what declined it.
    #[test]
    fn a_block_past_its_allowance_declines_and_frees_what_it_held() {
        let mut rng = Rng(0x0020_0071);
        let (mut declined, mut gathered) = (0, 0);
        for round in 0..200 {
            let block = random_block(&mut rng, round);
            let sizing = sized(block.group_size, None, None);
            // The unbounded run is the control: what the block gathers, and
            // the peak the account reaches gathering it — the interning maps
            // included, which the retained heap alone does not carry.
            let (control, free) = gathered_within(&block, sizing, None);
            let whole = control.gathered().expect("an unbounded account declines nothing");
            let peak = free.held().peak;

            let allowance = peak / 4;
            let (short, account) = gathered_within(&block, sizing, Some(allowance));
            assert_eq!(short, BlockGathered::Declined { allowance }, "round {round}");
            assert_eq!(
                account.held().now,
                StatisticsTerms::default(),
                "round {round}: a decline holds nothing"
            );
            assert!(
                account.held().peak < peak,
                "round {round}: {} reached the unbounded peak, {peak}",
                account.held().peak
            );
            declined += 1;

            // Room for the block and its interning maps several times over.
            let (roomy, _) = gathered_within(&block, sizing, Some(peak * 64 + (1 << 20)));
            assert_eq!(
                roomy.gathered().as_ref(),
                Some(&whole),
                "round {round}: an allowance that fits"
            );
            gathered += 1;
        }
        assert_eq!((declined, gathered), (200, 200));
    }

    /// **A piece that declined declines its block**, the block having lost
    /// that piece's rows; and a block that declined while a window ran drops
    /// every piece it is handed, a piece made after the decline gathering
    /// nothing
    /// (`docs/design/decisions.md`, "D85").
    #[test]
    fn a_declined_piece_declines_its_block_and_a_declined_block_its_pieces() {
        let mut rng = Rng(0x0020_0072);
        let block = random_block(&mut rng, 2);
        let sizing = sized(block.group_size, None, None);
        let whole = gathered_serially(&block, sizing);
        let allowance = whole.heap_bytes() / 4;

        let account = Arc::new(StatisticsAccount::bounded_by(Some(allowance)));
        let charge = Charge::new(Arc::clone(&account), Term::Gathering);
        let mut observer: Box<dyn BlockObserver> =
            Box::new(Gatherer::block(sizing, join_columns(), charge));
        // One piece a row, folded a window of four at a time, as the leader
        // folds them.
        let mut window: Vec<Box<dyn BlockObserver>> = Vec::new();
        for (line, &offset) in block.lines.iter().zip(&block.offsets) {
            window.push(observer.piece());
            window.last_mut().expect("just pushed").observe_row(offset, line);
            if window.len() == 4 {
                for piece in window.drain(..) {
                    observer.absorb(piece);
                }
            }
        }
        for piece in window.drain(..) {
            observer.absorb(piece);
        }
        // A piece made after the block declined holds nothing, whatever it is
        // handed.
        let mut late = observer.piece();
        for (line, &offset) in block.lines.iter().zip(&block.offsets) {
            late.observe_row(offset, line);
        }
        let before = account.held().now;
        drop(late);
        assert_eq!(account.held().now, before, "a piece of a declined block holds nothing");

        assert_eq!(observer.finish(block.end), BlockGathered::Declined { allowance });
        assert_eq!(account.held().now, StatisticsTerms::default());
    }

    /// **A full map grows into the table [`grown_map_heap`] charges**, and
    /// every table's size follows from its capacity as [`map_heap`] reads it:
    /// the arithmetic the account's interned term is, against the capacities
    /// the standard library's map actually reports as it fills. That the
    /// arithmetic is the allocation is the instrument build's to show
    /// (`pgdt/tests/statistics_account.rs`).
    #[test]
    fn a_full_map_grows_into_the_table_it_is_charged() {
        let mut map: HashMap<String, u32> = HashMap::new();
        let mut growths = 0;
        for i in 0..100_000u32 {
            let capacity = map.capacity();
            let full = map.len() == capacity;
            map.insert(i.to_string(), i);
            if full {
                growths += 1;
                assert_eq!(
                    map_heap::<String, u32>(map.capacity()),
                    grown_map_heap::<String, u32>(capacity)
                );
            } else {
                assert_eq!(map.capacity(), capacity, "a map grew before it was full");
            }
        }
        assert!(growths > 10, "only {growths} growths");
    }

    /// **A map made to hold `k` entries allocates the table a full map of
    /// `k − 1` grows into**, which is what [`DictionaryGatherer::renumber`]
    /// charges ahead of rebuilding its interning map, and reports a capacity
    /// [`map_heap`] reads back as that table. A map pruned in place does not,
    /// which is why the map is rebuilt: whether an erase hands its slot back
    /// depends on the hash seeding, so a pruned map's capacity may fall or
    /// stand, but under every seeding the table [`map_heap`] reads from it is
    /// larger than the one a rebuild allocates for the entries it kept.
    #[test]
    fn a_map_made_to_hold_its_entries_allocates_the_table_it_is_charged() {
        for k in 1..5000usize {
            let map: HashMap<String, u32> = HashMap::with_capacity(k);
            let table = grown_map_heap::<String, u32>(k - 1);
            assert_eq!(map_heap::<String, u32>(map.capacity()), table, "{k} entries");
        }
        let mut pruned: HashMap<String, u32> = (0..100).map(|i| (i.to_string(), i)).collect();
        pruned.retain(|_, id| *id % 10 == 0);
        assert!(
            map_heap::<String, u32>(pruned.capacity())
                > grown_map_heap::<String, u32>(pruned.len() - 1),
            "a pruned table is no larger than the table its entries are rebuilt into"
        );
        pruned.retain(|_, _| false);
        assert!(map_heap::<String, u32>(pruned.capacity()) > 0, "an emptied map lets its table go");
    }

    /// **A vector pushed or extended through [`push_charged`] and
    /// [`reserve_charged`] grows into exactly the capacity it was charged**,
    /// the charge ahead of each growth being the capacity after it
    /// (`docs/design/runtime-invariants.md`, "RT12").
    #[test]
    fn a_vector_grows_into_the_capacity_it_is_charged() {
        let mut charge = Charge::new(Arc::default(), Term::Gathering);
        let (mut groups, mut bounds): (Vec<RowGroup>, Vec<Option<Bounds>>) = (vec![], vec![]);
        let mut growths = 0;
        for i in 0..100_000u64 {
            let before = groups.capacity();
            if i % 7 == 0 {
                reserve_charged(&mut groups, (i % 13) as usize, &mut charge);
                groups.extend((0..i % 13).map(|rows| RowGroup { rows, bytes: 0 }));
            } else {
                push_charged(&mut groups, RowGroup { rows: i, bytes: i }, &mut charge);
            }
            push_charged(&mut bounds, None, &mut charge);
            growths += usize::from(groups.capacity() != before);
            assert_eq!(charge.charged().0, vec_heap(&groups) + vec_heap(&bounds), "push {i}");
        }
        assert!(growths > 10, "only {growths} growths");
    }

    fn cmp_text(a: &str, b: &str) -> Ordering {
        a.as_bytes().cmp(b.as_bytes())
    }

    #[test]
    fn a_long_texts_stored_bounds_are_on_the_right_side_of_it() {
        let long = format!("{}é{}", "a".repeat(250), "z".repeat(40));
        let lower = text_prefix(&long, DICTIONARY_ENTRY_MAX_BYTES);
        assert!(lower.len() <= DICTIONARY_ENTRY_MAX_BYTES);
        assert_ne!(cmp_text(lower, &long), Ordering::Greater);
        let upper = text_upper(&long, false).unwrap();
        assert!(upper.len() <= DICTIONARY_ENTRY_MAX_BYTES);
        assert_eq!(cmp_text(&upper, &long), Ordering::Greater);

        // The last character has no successor, so the one before it moves.
        let top = format!("{}{}", "b".repeat(300), char::MAX);
        let upper = text_upper(&top, false).unwrap();
        assert_eq!(cmp_text(&upper, &top), Ordering::Greater);
        assert!(text_upper(&char::MAX.to_string().repeat(100), false).is_none());
    }

    #[test]
    fn a_padded_upper_bound_is_above_the_value_once_blanks_are_dropped() {
        let long = format!("{}\u{1f}{}", "a".repeat(252), " x".repeat(40));
        let upper = text_upper(&long, true).unwrap();
        let key = |t: &str| t.trim_end_matches(' ').to_owned();
        assert_eq!(cmp_text(&key(&upper), &key(&long)), Ordering::Greater);
        let blanks = format!("{}{}", "a".repeat(10), " ".repeat(300));
        let upper = text_upper(&blanks, true).unwrap();
        assert_eq!(cmp_text(&key(&upper), &key(&blanks)), Ordering::Greater);
    }

    #[test]
    fn a_long_byteas_stored_bounds_are_on_the_right_side_of_it() {
        let mut bytes = vec![0x10u8; 200];
        bytes.extend([0xFF; 3]);
        let text = render_bytea(&bytes);
        let value = decode_bytea(&text).unwrap();
        let lower = text_prefix(&text, DICTIONARY_ENTRY_MAX_BYTES);
        assert!(lower.len() <= DICTIONARY_ENTRY_MAX_BYTES);
        assert!(decode_bytea(lower).unwrap() <= value);
        let upper = bytea_upper(&text).unwrap();
        assert!(upper.len() <= DICTIONARY_ENTRY_MAX_BYTES);
        assert!(decode_bytea(&upper).unwrap() > value);
        assert!(bytea_upper(&render_bytea(&[0xFF; 300])).is_none());
    }

    /// **A `bytea` column gathers alike in `hex` and in `escape`**: bounds
    /// and row order over random groups, sorted in some rounds, of values
    /// short, either side of [`BYTEA_ESCAPE_HEAD_BYTES`] and past it, half
    /// sharing a head that long, over bytes each escaping differently.
    #[test]
    fn a_bytea_column_gathers_alike_in_either_output_form() {
        use crate::decode::render_bytea_escape;

        let mut rng = Rng(0x000b_17ea);
        let byte = |rng: &mut Rng| [0x00, 0x41, 0x5c, 0x7f, 0xff][rng.below(5) as usize];
        let (mut bounded, mut ordered) = (0usize, 0usize);
        for round in 0..300 {
            let shared: Vec<u8> = (0..BYTEA_ESCAPE_HEAD_BYTES).map(|_| byte(&mut rng)).collect();
            let mut groups: Vec<Vec<Vec<u8>>> = (0..1 + rng.below(4))
                .map(|_| {
                    (0..rng.below(6))
                        .map(|_| {
                            let len = match rng.below(3) {
                                0 => rng.below(4) as usize,
                                1 => BYTEA_ESCAPE_HEAD_BYTES - 2 + rng.below(5) as usize,
                                _ => 250 + rng.below(40) as usize,
                            };
                            let mut v: Vec<u8> = (0..len).map(|_| byte(&mut rng)).collect();
                            if rng.below(2) == 0 {
                                v.splice(0..0, shared.iter().copied());
                            }
                            v
                        })
                        .collect()
                })
                .collect();
            if round % 3 == 0 {
                let mut all = groups.concat();
                all.sort();
                groups = vec![all];
            }
            let gathered = |render: fn(&[u8]) -> String| {
                let mut gatherer = BoundsGatherer::new(CompareKind::Bytea);
                let mut charge = Charge::new(Arc::default(), Term::Gathering);
                for group in &groups {
                    let mut state = gatherer.fresh_group();
                    for v in group {
                        gatherer.observe(&mut state, &render(v), None);
                    }
                    gatherer.close_group(state, &mut charge);
                }
                gatherer.finish()
            };
            let hex = gathered(render_bytea);
            assert_eq!(gathered(render_bytea_escape), hex, "round {round}");
            bounded += hex.groups.iter().flatten().count();
            ordered += usize::from(hex.sortedness != Sortedness::Unsorted);
        }
        assert!(bounded > 300 && ordered > 50, "{bounded} groups bounded, {ordered} ordered");
    }
}
