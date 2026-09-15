//! Per-row-group column statistics: what a mapping pass may be asked to
//! gather for a `COPY` block, the persisted shape it gathers into, and the
//! account of every statistic a pass holds alive ([`StatisticsAccount`]).
//!
//! L1 vocabulary only (`docs/design/decisions.md`, "D74"): column names, the
//! declared type text and `COLLATE` clause a column's statistics were computed
//! under, counts, and bounds as unescaped field text — what `pg_dump` wrote
//! where the value fits [`STORED_VALUE_CAP`], and a prefix or a successor of it
//! where it does not ([`Bounds::max_exact`]). Which
//! column gets which statistic, and how a value is ordered, is decided above
//! this layer by whatever implements [`BlockObserver`]; the mapping pass hands
//! it every row and stays type-blind.
//!
//! **A row group is a byte range** (`docs/design/decisions.md`, "D34"): group
//! `k` of a block is the rows whose first byte lies in `[k·N, (k+1)·N)` of the
//! block's data, `N` being [`BlockStatistics::group_size`]. A row longer than
//! `N` leaves groups in which no row starts, which are listed, empty.

use std::mem::size_of;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Deserializer, Serialize};

use crate::copy::CopyHeader;
use crate::index::CopyBlock;
use crate::instrument;

/// The group size a request that states none gathers at: one mebibyte of a
/// block's data per group.
pub const DEFAULT_STATISTICS_GROUP_SIZE: u64 = 1 << 20;

/// The longest text any stored bound or dictionary entry may be, in bytes.
pub const STORED_VALUE_CAP: usize = 256;

/// The most distinct texts one group's dictionary may hold; a group with more
/// has none on that column.
pub const DICTIONARY_CAP: usize = 64;

/// What a mapping pass is asked to gather: which columns, at what group size
/// (`docs/design/decisions.md`, "D77").
/// An argument of [`crate::stream::map_file`] alone — a query never gathers.
///
/// **The default gathers every statistic** ([`Self::ALL`]), a parse carrying
/// the intent to do all work a later query could use; gathering less is stated,
/// [`Self::NONE`] gathering nothing (`docs/design/roadmap.md`, "A parse does
/// all the work a later query could use").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatisticsRequest {
    /// The columns statistics are gathered for.
    pub selection: StatisticsSelection,
    /// The group size, `None` when the caller stated none and
    /// [`DEFAULT_STATISTICS_GROUP_SIZE`] applies. Kept apart from the default
    /// because a stated size and an unstated one are different requests to a
    /// block already gathered at another. In a block the request does not
    /// track ([`Self::tracked_columns`] answering `None`, as it always does
    /// for [`StatisticsSelection::None`]) it sizes nothing and is ignored.
    pub group_size: Option<NonZeroU64>,
}

impl StatisticsRequest {
    /// Every column of every table, at the default group size — the default.
    pub const ALL: Self = Self { selection: StatisticsSelection::All, group_size: None };

    /// Nothing gathered: what a query's mapping pass always asks.
    pub const NONE: Self = Self { selection: StatisticsSelection::None, group_size: None };

    /// Whether this request may track a column at all — false for
    /// [`StatisticsSelection::None`] alone.
    pub fn gathers(&self) -> bool {
        self.selection != StatisticsSelection::None
    }

    /// The group size this request gathers at.
    pub fn group_size(&self) -> u64 {
        self.group_size.map_or(DEFAULT_STATISTICS_GROUP_SIZE, NonZeroU64::get)
    }

    /// Which of `header`'s columns this request tracks, positionally — `None`
    /// when it tracks nothing in the block, which then gathers no statistics
    /// at all. A block whose header names no columns is tracked with no
    /// column, its groups still counted.
    pub fn tracked_columns(&self, header: &CopyHeader) -> Option<Vec<bool>> {
        let targets = match &self.selection {
            StatisticsSelection::All => return Some(vec![true; header.columns.len()]),
            StatisticsSelection::None => return None,
            StatisticsSelection::Only(targets) => targets,
        };
        let mut whole_table = false;
        let mut tracked = vec![false; header.columns.len()];
        for target in targets {
            match target {
                StatisticsTarget::Table(table) if header.matches(table) => whole_table = true,
                StatisticsTarget::Column { table, column } if header.matches(table) => {
                    if let Some(i) = header.columns.iter().position(|c| c == column) {
                        tracked[i] = true;
                    }
                }
                _ => {}
            }
        }
        if whole_table {
            return Some(vec![true; header.columns.len()]);
        }
        tracked.contains(&true).then_some(tracked)
    }

    /// What re-reading `block`, already mapped, must gather for it to hold
    /// what this request asks of it — `None` when it holds that already, or
    /// when the request tracks nothing in it.
    ///
    /// **A block lacks the requested statistics** where it holds none, where a
    /// column the request tracks was not gathered, or where the request
    /// **states** a group size other than the one the block was gathered at;
    /// an unstated size lacks nothing a gathered block holds
    /// (`docs/design/decisions.md`, "D34").
    pub fn backfill(&self, block: &CopyBlock) -> Option<StatisticsBackfill> {
        let requested = self.tracked_columns(&block.header)?;
        let Some(held) = block.statistics.as_deref() else {
            return Some(StatisticsBackfill { columns: requested, group_size: self.group_size() });
        };
        let resized = self.group_size.is_some_and(|size| size.get() != held.group_size);
        let missing = requested
            .iter()
            .enumerate()
            .any(|(i, &wanted)| wanted && held.columns.get(i).is_none_or(Option::is_none));
        if !resized && !missing {
            return None;
        }
        let columns = requested
            .iter()
            .enumerate()
            .map(|(i, &wanted)| wanted || held.columns.get(i).is_some_and(Option::is_some))
            .collect();
        let group_size = self.group_size.map_or(held.group_size, NonZeroU64::get);
        Some(StatisticsBackfill { columns, group_size })
    }
}

/// What one mapped block is re-read to gather, from
/// [`StatisticsRequest::backfill`]: **everything the block already held as
/// well as everything asked for**, so a back-fill never loses a statistic an
/// earlier pass gathered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatisticsBackfill {
    /// The columns gathered, positionally to the block's header.
    pub columns: Vec<bool>,
    /// The group size gathered at: the request's stated size, or else the
    /// size the block already held, or else [`DEFAULT_STATISTICS_GROUP_SIZE`].
    pub group_size: u64,
}

/// The columns a [`StatisticsRequest`] names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StatisticsSelection {
    /// Every column of every table.
    #[default]
    All,
    /// Only these tables and columns.
    Only(Vec<StatisticsTarget>),
    /// No column of any table.
    None,
}

/// One entry of a narrowed selection. A table is named as
/// [`CopyHeader::matches`] reads it, qualified or bare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatisticsTarget {
    /// Every column of the table.
    Table(String),
    /// One column of the table.
    Column { table: String, column: String },
}

/// Receives every row of one `COPY` block from the mapping pass and answers
/// the block's statistics once it closes. Defined here and implemented at L4,
/// the shape `docs/design/decisions.md`, "D74" gives a cross-layer trait.
///
/// **A block the leader splits is observed piece by piece**: each piece's rows
/// go to an observer [`Self::piece`] made, and the pieces are handed back in
/// file order to [`Self::absorb`], after which the block's observer answers
/// what it would have had it been handed every row itself.
pub(crate) trait BlockObserver: Send {
    /// One row, `offset` being where its first byte sits relative to the
    /// block's first data byte, and `raw` the still-escaped line without its
    /// terminator.
    fn observe_row(&mut self, offset: u64, raw: &[u8]);

    /// The block's statistics. `end` is the terminator line's offset relative
    /// to the block's first data byte — where the last row's line ends.
    fn finish(self: Box<Self>, end: u64) -> BlockStatistics;

    /// An observer for a piece of this block: rows starting anywhere after the
    /// ones this observer has been handed, to be given back to
    /// [`Self::absorb`] rather than finished.
    fn piece(&self) -> Box<dyn BlockObserver>;

    /// Fold `later`, made by [`Self::piece`] and handed rows that all follow
    /// every row this observer has seen, into this one.
    fn absorb(&mut self, later: Box<dyn BlockObserver>);

    /// This observer as [`Any`](std::any::Any), which is how [`Self::absorb`]
    /// recovers the concrete piece it made.
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;
}

/// One block's statistics, held by [`crate::index::CopyBlock::statistics`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockStatistics {
    /// `N`, the bytes of block data each group covers.
    pub group_size: u64,
    /// Every group from the first to the one the last row starts in, in order.
    pub groups: Vec<RowGroup>,
    /// One entry per column of the block's header, `None` for a column the
    /// request did not track.
    pub columns: Vec<Option<ColumnStatistics>>,
}

/// One group's extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowGroup {
    /// Rows whose first byte lies in the group. Zero for a group no row
    /// starts in.
    pub rows: u64,
    /// From the group's first row's first byte to its last row's line end,
    /// which a row straddling the group's end carries past `N`.
    pub bytes: u64,
}

/// One column's statistics over a block, every per-group vector as long as
/// [`BlockStatistics::groups`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnStatistics {
    /// The declared type text the statistics were computed under, as the
    /// preamble recorded it; `None` where the column was not declared.
    pub declared_type: Option<String>,
    /// The column's `COLLATE` clause, verbatim; `None` for no clause.
    pub collation: Option<String>,
    /// NULLs per group.
    pub null_counts: Vec<u64>,
    /// Present for a column its comparison orders exactly.
    pub bounds: Option<ColumnBounds>,
    /// Present for a column its comparison equates exactly.
    pub dictionary: Option<ColumnDictionary>,
}

/// Bounds per group and the block's row order, for one column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnBounds {
    /// The non-NULL values' order over the whole block, row by row.
    pub sortedness: Sortedness,
    /// Per group; `None` where the group holds no non-NULL value or a value
    /// the bounds could not cover.
    pub groups: Vec<Option<Bounds>>,
}

/// A lower and an upper bound on one group's non-NULL values, as unescaped
/// field text no longer than [`STORED_VALUE_CAP`] — a `character` value's
/// without the trailing blanks its comparison ignores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds {
    /// No value orders below it.
    pub min: String,
    /// No value orders above it.
    pub max: String,
    /// Whether `max` is a value the group holds. A truncated upper bound is
    /// above every value and is never read as one.
    pub max_exact: bool,
}

/// Whether a column's non-NULL values are in order row by row over a block.
/// Equal neighbours are in either order; a column with at most one distinct
/// value is `Ascending`. A value gathering cannot place against its neighbour —
/// one that does not key, a keyed one past [`STORED_VALUE_CAP`], or a bytewise
/// one agreeing with it past what a bound reads — makes the column `Unsorted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sortedness {
    Ascending,
    Descending,
    Unsorted,
}

/// Every distinct text per group, interned once per block and column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDictionary {
    /// Each distinct text once, in first-seen order — a `character` value's
    /// without the trailing blanks its comparison ignores, as with [`Bounds`].
    pub entries: Vec<String>,
    /// Per group, indices into `entries`; `None` where the group held more than
    /// [`DICTIONARY_CAP`] distinct texts, a text longer than
    /// [`STORED_VALUE_CAP`], or a field that is not text.
    pub groups: Vec<Option<Vec<u32>>>,
}

/// Bytes of heap a `Vec` holds: its capacity, not its length, since no
/// gathering vector is shrunk.
pub(crate) fn vec_heap<T>(v: &Vec<T>) -> u64 {
    (v.capacity() * size_of::<T>()) as u64
}

/// Bytes of heap a `String` holds.
pub(crate) fn text_heap(text: &String) -> u64 {
    text.capacity() as u64
}

impl BlockStatistics {
    /// The heap one block's statistics hold, the allocation of the `Arc` that
    /// shares them included: what [`StatisticsAccount`] charges a retained or a
    /// loaded block. Each term is the size the allocator was asked for — a
    /// vector's capacity, not its length — which
    /// `tests/statistics_heap.rs` holds to what freeing a block gives back.
    pub fn heap_bytes(&self) -> u64 {
        let shared = size_of::<(usize, usize, Self)>() as u64;
        shared
            + vec_heap(&self.groups)
            + vec_heap(&self.columns)
            + self.columns.iter().flatten().map(ColumnStatistics::heap_bytes).sum::<u64>()
    }
}

/// Under the instrument, a block's statistics are freed inside a statistics
/// scope wherever they are dropped — a cache decoded only for its envelope
/// included — so what the decode attributed is given back the same way
/// (`crate::instrument`). Only the `Arc`'s own allocation is freed after.
#[cfg(feature = "introspect")]
impl Drop for BlockStatistics {
    fn drop(&mut self) {
        let _attributed = instrument::StatisticsScope::enter();
        drop(std::mem::take(&mut self.columns));
        drop(std::mem::take(&mut self.groups));
    }
}

impl ColumnStatistics {
    fn heap_bytes(&self) -> u64 {
        let named = self.declared_type.iter().chain(&self.collation).map(text_heap).sum::<u64>();
        let bounds = self.bounds.as_ref().map_or(0, |bounds| {
            vec_heap(&bounds.groups)
                + bounds
                    .groups
                    .iter()
                    .flatten()
                    .map(|b| text_heap(&b.min) + text_heap(&b.max))
                    .sum::<u64>()
        });
        let dictionary = self.dictionary.as_ref().map_or(0, |dictionary| {
            vec_heap(&dictionary.entries)
                + dictionary.entries.iter().map(text_heap).sum::<u64>()
                + vec_heap(&dictionary.groups)
                + dictionary.groups.iter().flatten().map(vec_heap).sum::<u64>()
        });
        named + vec_heap(&self.null_counts) + bounds + dictionary
    }
}

/// [`CopyBlock::statistics`] as the cache decodes it, with what the decode
/// allocates attributed to statistics where the instrument is built in
/// (`crate::instrument`) — and a plain decode where it is not.
pub(crate) fn deserialize_block_statistics<'de, D>(
    deserializer: D,
) -> Result<Option<Arc<BlockStatistics>>, D::Error>
where
    D: Deserializer<'de>,
{
    let _attributed = instrument::StatisticsScope::enter();
    Option::<Arc<BlockStatistics>>::deserialize(deserializer)
}

/// The most growth an observer holds uncharged: past it, the observer updates
/// the pass's [`StatisticsAccount`] before it observes another row. A
/// judgement setting only how often a wide observer updates.
pub(crate) const CHARGE_STEP: u64 = 64 << 10;

/// Every statistic a mapping pass holds alive, in bytes of heap, by term
/// (`docs/design/decisions.md`, "D81").
///
/// **An account, not a bound**: each term is the sizes the allocator was asked
/// for, summed as the structures change — a finished block once, as it is
/// retained or loaded; an observer still gathering whenever its uncharged
/// growth passes [`CHARGE_STEP`], at every column it closes a group on and
/// through every piece it folds in; and a vector or an interning map ahead of
/// the allocation it grows into. Shared by every observer of one pass, the
/// leader's pieces on the blocking pool included, so it is one lock over its
/// terms: an update, its peak and the instrument's check are read whole, and
/// no reader sees another thread's update half applied.
///
/// **What it does not see** is bounded by an observer rather than by the dump:
/// each open observer's growth until it passes [`CHARGE_STEP`], a row's own
/// decode scratch while a column observes it, and the observer's own
/// allocation. Nor does it hold a save's encode buffer or a load's file bytes,
/// which carry statistics serialized for as long as the save or the load runs.
#[derive(Debug, Default)]
pub(crate) struct StatisticsAccount {
    state: Mutex<AccountState>,
}

#[derive(Debug, Default)]
struct AccountState {
    terms: [u64; TERMS],
    term_peaks: [u64; TERMS],
    peak: u64,
    /// What the terms carry for allocations charged ahead and not yet made
    /// ([`Charge::ahead`]).
    announced: u64,
    /// Every [`Charge`] alive: the observers whose uncharged growth the
    /// account may be short of.
    observers: u64,
}

const TERMS: usize = 5;

/// One of [`StatisticsTerms`]' fields, as [`StatisticsAccount`] indexes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Term {
    Retained,
    Loaded,
    Gathering,
    Pieces,
    Interned,
}

impl StatisticsAccount {
    /// Apply every change in `changes` as one update: the peak and the
    /// instrument's check read the account once all of them are in, so a
    /// charge moving from one term to another never reads as both or neither.
    pub(crate) fn apply(&self, changes: &[(Term, i64)]) {
        self.update(changes, 0, 0);
    }

    /// [`Self::apply`], with `announced` moving what the terms carry for
    /// allocations not yet made and `observers` the count of charges alive,
    /// in the same update.
    fn update(&self, changes: &[(Term, i64)], announced: i64, observers: i64) {
        let mut state = self.lock();
        for &(term, delta) in changes {
            let term = term as usize;
            state.terms[term] = state.terms[term].wrapping_add(delta as u64);
            state.term_peaks[term] = state.term_peaks[term].max(state.terms[term]);
        }
        state.announced = state.announced.wrapping_add(announced as u64);
        // An observer closing is still open for its own update: what it has
        // not freed yet is freed after.
        let open = state.observers.max(state.observers.wrapping_add(observers as u64));
        state.observers = state.observers.wrapping_add(observers as u64);
        let total = state.terms.iter().sum::<u64>();
        state.peak = state.peak.max(total);
        let allowance = open * CHARGE_STEP;
        instrument::statistics_account_updated(total, state.announced, allowance);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AccountState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What the account holds now, and the most it has held.
    pub(crate) fn held(&self) -> StatisticsHeld {
        let state = self.lock();
        let read = |values: &[u64; TERMS]| {
            let [retained, loaded, gathering, pieces, interned] = *values;
            StatisticsTerms { retained, loaded, gathering, pieces, interned }
        };
        StatisticsHeld {
            now: read(&state.terms),
            term_peaks: read(&state.term_peaks),
            peak: state.peak,
        }
    }
}

/// An in-flight observer's share of a [`StatisticsAccount`], charged to one
/// term and to [`Term::Interned`], and released when it is dropped.
///
/// **Declared last in whatever holds it**, so the release follows the drop of
/// everything it charged for: an account released ahead of its memory reads
/// short of the heap for as long as the drop takes.
pub(crate) struct Charge {
    account: Arc<StatisticsAccount>,
    term: Term,
    structure: u64,
    interned: u64,
}

impl Charge {
    pub(crate) fn new(account: Arc<StatisticsAccount>, term: Term) -> Self {
        account.update(&[], 0, 1);
        Self { account, term, structure: 0, interned: 0 }
    }

    /// The account this charge is part of, for a piece of the same pass.
    pub(crate) fn account(&self) -> &Arc<StatisticsAccount> {
        &self.account
    }

    /// What this charge holds now, as its structure and its interned bytes.
    pub(crate) fn charged(&self) -> (u64, u64) {
        (self.structure, self.interned)
    }

    /// Charge `structure` bytes to this charge's term and `interned` to
    /// [`Term::Interned`], in place of what was charged before.
    pub(crate) fn set(&mut self, structure: u64, interned: u64) {
        let changes = [
            (self.term, structure as i64 - self.structure as i64),
            (Term::Interned, interned as i64 - self.interned as i64),
        ];
        (self.structure, self.interned) = (structure, interned);
        self.account.apply(&changes);
    }

    /// **Charge `grows` — structure and interned bytes — ahead of the
    /// allocation `allocate` makes, and release `frees` once it has freed
    /// them**: a vector or a table grown while the smaller one is live, or
    /// realloc'd, is charged before any other update can read the heap
    /// holding it. Until the second update, the instrument's check reads the
    /// allocation as made where it judges the account short and as not yet
    /// made where it judges it over (`crate::instrument`).
    pub(crate) fn ahead<R>(
        &mut self,
        grows: (u64, u64),
        frees: (u64, u64),
        allocate: impl FnOnce() -> R,
    ) -> R {
        let announced = (grows.0 + grows.1) as i64;
        self.structure += grows.0;
        self.interned += grows.1;
        let changes = [(self.term, grows.0 as i64), (Term::Interned, grows.1 as i64)];
        self.account.update(&changes, announced, 0);
        let made = allocate();
        self.structure -= frees.0;
        self.interned -= frees.1;
        let changes = [(self.term, -(frees.0 as i64)), (Term::Interned, -(frees.1 as i64))];
        self.account.update(&changes, -announced, 0);
        made
    }

    /// Take `other`'s charge onto this one's term in one update, leaving
    /// `other` charging nothing: a piece whose structures its block is about
    /// to hold.
    pub(crate) fn take_over(&mut self, other: &mut Charge) {
        let changes =
            [(other.term, -(other.structure as i64)), (self.term, other.structure as i64)];
        self.structure += other.structure;
        self.interned += other.interned;
        (other.structure, other.interned) = (0, 0);
        self.account.apply(&changes);
    }

    /// Hand `held` — structure and interned bytes of what this charge carries
    /// — back to `other` in one update: what is left of a folded piece, which
    /// its own charge then releases as the piece is freed.
    pub(crate) fn hand_back(&mut self, other: &mut Charge, held: (u64, u64)) {
        let changes = [(self.term, -(held.0 as i64)), (other.term, held.0 as i64)];
        self.structure -= held.0;
        self.interned -= held.1;
        other.structure += held.0;
        other.interned += held.1;
        self.account.apply(&changes);
    }

    /// Release this charge and credit `retained` bytes to [`Term::Retained`]
    /// in one update: an observer that has become its block's statistics.
    pub(crate) fn retain(&mut self, retained: u64) {
        let changes = [
            (self.term, -(self.structure as i64)),
            (Term::Interned, -(self.interned as i64)),
            (Term::Retained, retained as i64),
        ];
        (self.structure, self.interned) = (0, 0);
        self.account.apply(&changes);
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        let changes =
            [(self.term, -(self.structure as i64)), (Term::Interned, -(self.interned as i64))];
        self.account.update(&changes, 0, -1);
    }
}

/// What a [`StatisticsAccount`] held, by term, in bytes of heap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatisticsTerms {
    /// Finished blocks this pass gathered, which the map holds until it is
    /// dropped.
    pub retained: u64,
    /// Blocks' statistics decoded from the cache the pass loaded, less any a
    /// back-fill has replaced.
    pub loaded: u64,
    /// Every block observer still gathering: its closed groups and its open
    /// ones, and a piece it is folding in.
    pub gathering: u64,
    /// Every observer a parallel window made for a piece and has not folded
    /// into its block — a piece past the block's terminator until it is
    /// dropped — charged as a block observer is.
    pub pieces: u64,
    /// Every dictionary's interning map while it gathers, block or piece: the
    /// second copy of each distinct text, and the table holding it.
    pub interned: u64,
}

impl StatisticsTerms {
    /// Every term summed.
    pub fn total(&self) -> u64 {
        self.retained + self.loaded + self.gathering + self.pieces + self.interned
    }
}

/// A [`StatisticsAccount`] read whole: what one mapping pass's statistics
/// held when it returned, and the most they held
/// ([`crate::stream::MapRun::statistics`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatisticsHeld {
    /// Each term when the account was read.
    pub now: StatisticsTerms,
    /// The largest each term reached, each at its own moment.
    pub term_peaks: StatisticsTerms,
    /// The largest the sum of the terms reached, read at an update.
    pub peak: u64,
}
