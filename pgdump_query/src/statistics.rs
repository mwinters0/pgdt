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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Every statistic a mapping pass holds alive, in bytes of heap, by term
/// (`docs/design/decisions.md`, "D81").
///
/// **An account, not a bound**: each term is the sizes the allocator was asked
/// for, summed as the structures change — a finished block once, as it is
/// retained or loaded; an observer still gathering at every column it closes a
/// group on and every piece it folds in, a dictionary's interning map ahead of
/// the table it grows into. Shared by every observer of one pass, the leader's
/// pieces on the blocking pool included, so the terms are atomics and the
/// account is read whole only by [`Self::held`].
///
/// **What it does not see**, each bounded by a group rather than by the dump:
/// an open group's growth past the largest a close of that column has
/// measured — all of it, on a column's first group — and a row's own decode
/// scratch while a column observes it. Nor does it hold a save's encode buffer
/// or a load's file bytes, which carry statistics serialized for as long as
/// the save or the load runs.
#[derive(Debug, Default)]
pub(crate) struct StatisticsAccount {
    terms: [AtomicU64; TERMS],
    term_peaks: [AtomicU64; TERMS],
    peak: AtomicU64,
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
        for &(term, delta) in changes {
            if delta == 0 {
                continue;
            }
            let now = self.terms[term as usize]
                .fetch_add(delta as u64, Ordering::Relaxed)
                .wrapping_add(delta as u64);
            if delta > 0 {
                self.term_peaks[term as usize].fetch_max(now, Ordering::Relaxed);
            }
        }
        let total = self.terms.iter().map(|t| t.load(Ordering::Relaxed)).sum::<u64>();
        self.peak.fetch_max(total, Ordering::Relaxed);
        instrument::statistics_account_updated(total);
    }

    /// What the account holds now, and the most it has held.
    pub(crate) fn held(&self) -> StatisticsHeld {
        let read = |atomics: &[AtomicU64; TERMS]| {
            let [retained, loaded, gathering, pieces, interned] =
                atomics.each_ref().map(|a| a.load(Ordering::Relaxed));
            StatisticsTerms { retained, loaded, gathering, pieces, interned }
        };
        StatisticsHeld {
            now: read(&self.terms),
            term_peaks: read(&self.term_peaks),
            peak: self.peak.load(Ordering::Relaxed),
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

    /// Move `delta` bytes onto [`Term::Interned`] between two [`Self::set`]s:
    /// a table allocated, or freed, ahead of the observer's next update.
    pub(crate) fn adjust_interned(&mut self, delta: i64) {
        self.interned = self.interned.wrapping_add(delta as u64);
        self.account.apply(&[(Term::Interned, delta)]);
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
        if self.structure != 0 || self.interned != 0 {
            self.set(0, 0);
        }
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
    /// Every block observer still gathering: its closed groups, and each
    /// column's open group at the largest a close has measured it.
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
