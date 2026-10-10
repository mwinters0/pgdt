//! Per-row-group column statistics: what a mapping pass may be asked to
//! gather for a `COPY` block, and the account of every statistic a pass holds
//! alive ([`StatisticsAccount`]). The shape they persist in is
//! [`pgdt_model::model`]'s ([`pgdt_model::BlockStatistics`]); which column gets
//! which statistic, and how a value is ordered, is decided by whatever
//! implements [`pgdt_model::BlockObserver`], the mapping pass handing it
//! every row and staying type-blind.

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use pgdt_core::CopyHeader;
use pgdt_core::internal as instrument;
use pgdt_model::internal::BlockStatisticsExt;
use pgdt_model::{CopyBlock, GroupSizing};

/// The group size a request that states none gathers at: one mebibyte of a
/// block's data per group.
pub const ROW_GROUP_DEFAULT_SIZE_BYTES: u64 = 1 << 20;

/// The most groups a block's statistics hold under an unstated group size and
/// an unstated maximum: past it, adjacent groups merge pairwise and the
/// block's group size doubles, so what a long block holds grows with its
/// columns rather than its bytes (`docs/design/decisions.md`, "D82"). A stated
/// maximum turns the cap off ([`StatisticsRequest::group_cap`]), so a block
/// gathered under one may hold more groups than this. A judgement, not a
/// reading.
pub const BLOCK_MAX_ROW_GROUPS: usize = 4096;

/// The fewest rows a block's median group — its upper middle one, so that at
/// most half the groups fall short — holds under an unstated group size and an
/// unstated minimum: short of it, the finished block's groups merge
/// pairwise until that group reaches it, the block is one group, or the next
/// size would break a stated maximum, which outranks the minimum
/// (`gather::density_merges`; `docs/design/decisions.md`, "D82").
/// `ROW_GROUP_DEFAULT_SIZE_BYTES` over a row a kibibyte wide; a judgement,
/// not a reading.
pub const ROW_GROUP_DEFAULT_MIN_ROWS: u64 = 1 << 10;

/// What a mapping pass is asked to record: each table's and column's level,
/// at what group size and between what density bounds
/// (`docs/design/decisions.md`, "D77"). An argument of
/// [`crate::stream::map_file`] alone — a query never gathers.
///
/// **The default is the data level everywhere** ([`Self::DATA`]), a parse
/// carrying the intent to do all work a later query could use; recording less
/// is stated, [`Self::METADATA`] recording nothing drawn from any block's
/// rows (`docs/design/roadmap.md`, "A parse does all the work a later query
/// could use").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatisticsRequest {
    /// Each table's and column's level: which blocks are censused, and which
    /// columns statistics are gathered for.
    pub selection: StatisticsSelection,
    /// The group size, `None` when the caller stated none and
    /// [`ROW_GROUP_DEFAULT_SIZE_BYTES`] applies, doubled past
    /// [`BLOCK_MAX_ROW_GROUPS`] groups unless [`Self::max_rows`] is stated,
    /// and short of [`Self::min_rows`]; a stated size is gathered exactly.
    /// Kept apart from the default because a stated size and an unstated one
    /// are different requests to a block already gathered at another. In a
    /// block the request does not track ([`Self::tracked_columns`] answering
    /// `None`, as it always does under [`StatisticsSelection::METADATA`]) it sizes
    /// nothing and is ignored.
    pub group_size: Option<NonZeroU64>,
    /// The fewest rows a block's median group should hold, `None` when the
    /// caller stated none and [`ROW_GROUP_DEFAULT_MIN_ROWS`] applies; `0`
    /// coarsens nothing. Kept apart from the default for the reason
    /// [`Self::group_size`] is. Under a stated group size, which is gathered
    /// exactly, it sizes nothing and is ignored.
    pub min_rows: Option<u64>,
    /// The most rows a block's densest tenth of groups should hold — read at
    /// the 90th-percentile group ([`pgdt_model::max_rows_group`]) — and **there is no
    /// default**: a maximum applies only where a caller states one.
    ///
    /// Short of a stated group size, it is the only thing that makes a block
    /// finer than [`ROW_GROUP_DEFAULT_SIZE_BYTES`], so where it is stated it **outranks
    /// both the minimum and [`BLOCK_MAX_ROW_GROUPS`]**, which it turns off
    /// ([`Self::group_cap`]) — a person sensitive to what a query reads asked
    /// for the groups, and neither a default nor a bound on memory quietly
    /// overrules them. Under a stated group size it sizes nothing and is
    /// ignored, as [`Self::min_rows`] is.
    pub max_rows: Option<u64>,
}

impl StatisticsRequest {
    /// Every table and column at the data level, at the default group size
    /// and minimum and under no maximum — the default.
    pub const DATA: Self = Self {
        selection: StatisticsSelection::DATA,
        group_size: None,
        min_rows: None,
        max_rows: None,
    };

    /// Every table at the metadata level: no block censused, nothing gathered.
    pub const METADATA: Self = Self {
        selection: StatisticsSelection::METADATA,
        group_size: None,
        min_rows: None,
        max_rows: None,
    };

    /// Whether this request may track a column at all — false where no table
    /// or column is at the data level.
    pub fn gathers(&self) -> bool {
        self.selection.default == StatisticsLevel::Data
            || self.selection.overrides.iter().any(|(_, level)| *level == StatisticsLevel::Data)
    }

    /// The group size this request gathers at, before any merge.
    pub fn group_size(&self) -> u64 {
        self.group_size.map_or(ROW_GROUP_DEFAULT_SIZE_BYTES, NonZeroU64::get)
    }

    /// The most groups a block this request gathers may hold:
    /// [`BLOCK_MAX_ROW_GROUPS`] under an unstated size and an unstated
    /// maximum, `None` under either: a stated size is gathered exactly, and a
    /// stated maximum bounds the merging in the cap's place.
    pub fn group_cap(&self) -> Option<usize> {
        (self.group_size.is_none() && self.max_rows.is_none()).then_some(BLOCK_MAX_ROW_GROUPS)
    }

    /// The density minimum a block this request gathers is sized by at its
    /// end: the stated or default minimum under an unstated size, `None` under
    /// a stated one.
    pub fn min_rows(&self) -> Option<u64> {
        self.group_size.is_none().then(|| self.min_rows.unwrap_or(ROW_GROUP_DEFAULT_MIN_ROWS))
    }

    /// The density maximum a block this request gathers is sized by at its
    /// end: the stated maximum under an unstated size, and `None` under a
    /// stated size or where no maximum was stated — there being no default.
    pub fn max_rows(&self) -> Option<u64> {
        self.group_size.is_none().then_some(self.max_rows).flatten()
    }

    /// What a block this request gathers from its first row records it was
    /// sized under.
    pub fn sizing(&self) -> GroupSizing {
        match self.min_rows() {
            Some(min_rows) => GroupSizing::Density { min_rows, max_rows: self.max_rows() },
            None => GroupSizing::Stated,
        }
    }

    /// Gathering `columns`, of which it tracks `requested`, from a block's
    /// first row as this request sizes it.
    pub(crate) fn gathering(&self, columns: Vec<bool>, requested: Vec<bool>) -> StatisticsBackfill {
        StatisticsBackfill {
            columns,
            requested,
            group_size: self.group_size(),
            group_cap: self.group_cap(),
            min_rows: self.min_rows(),
            max_rows: self.max_rows(),
            sizing: self.sizing(),
        }
    }

    /// Which of `header`'s columns this request tracks, positionally — each
    /// column at the data level ([`StatisticsSelection::level`]) — and `None`
    /// when it tracks nothing in the block, which is then at the metadata
    /// level: neither censused nor gathered. A block whose header names no
    /// columns is tracked with no column, its groups still counted, where its
    /// table is at the data level.
    ///
    /// **This is also whether the block is censused**: a table is at the data
    /// level where any of its columns is (`docs/design/decisions.md`, "D35").
    pub fn tracked_columns(&self, header: &CopyHeader) -> Option<Vec<bool>> {
        let data =
            |column: Option<&str>| self.selection.level(header, column) == StatisticsLevel::Data;
        if header.columns.is_empty() {
            return data(None).then(Vec::new);
        }
        let tracked: Vec<bool> = header.columns.iter().map(|c| data(Some(c))).collect();
        tracked.contains(&true).then_some(tracked)
    }
}

/// What a crate above calls on [`StatisticsRequest`] that is not its API:
/// public only because a crate boundary forced it, and reached through
/// `internal`.
pub trait StatisticsRequestExt {
    fn backfill(
        &self,
        block: &CopyBlock,
        bounded: &[bool],
        allowance: Option<u64>,
    ) -> Option<StatisticsBackfill>;
}

impl StatisticsRequestExt for StatisticsRequest {
    /// What re-reading `block`, already mapped, must gather for it to hold
    /// what this request asks of it — `None` when it holds that already, or
    /// when the request tracks nothing in it.
    ///
    /// **A block lacks the requested statistics** where it holds none — every
    /// block mapped at the metadata level that this request puts at the data
    /// level among them, the re-read censusing it too — where a
    /// column the request tracks was not gathered or was gathered without the
    /// bounds `bounded` says gathering keeps for it, positionally to the
    /// block's header (`crate::stream::bounded_columns`;
    /// `docs/design/decisions.md`, "D79"), where the request
    /// **states** a group size other than the one the block was gathered at,
    /// or where it states no size, **states** a bound — minimum or maximum —
    /// and the sizing that makes, the other bound at its default, is not the
    /// block's record ([`pgdt_model::BlockStatistics::sizing`]). A
    /// resized block is re-read from its first row as the request sizes it; an
    /// unstated size and bounds lack nothing a gathered block holds, and
    /// re-read a block lacking a column at the size it holds, exactly, keeping
    /// its record (`docs/design/decisions.md`, "D34").
    ///
    /// **A block that declined is re-read only under a larger allowance**
    /// ([`CopyBlock::statistics_declined`]): `allowance` is this pass's, and
    /// `None` — an embedder that stated none — declines nothing and so retries
    /// everything. Without the record a `parse` at the same allocation would
    /// re-read and re-decline the same block every run
    /// (`docs/design/decisions.md`, "D85").
    ///
    /// **A block still at the size it gathered from also lacks a stated
    /// maximum its groups break** ([`pgdt_model::BlockStatistics::breaks_max_rows`]): no
    /// merge can make a block finer, so the maximum is reachable only by
    /// re-reading it at [`pgdt_model::BlockStatistics::predicted_group_size`]. Once that
    /// re-read has happened the block is finer than the size a gather starts
    /// from, so it is never re-read for that maximum again however its rows
    /// cluster.
    ///
    /// **So a run reads a block at most twice for a maximum, and a block it
    /// mapped itself once.** A block gathered under another sizing is resized
    /// first, which this answers from its first row at the default size, and
    /// only the groups that read leaves are asked whether they break the
    /// maximum. Rejected: predicting the finer size from the groups another
    /// sizing left, which saves that first read by guessing where the read
    /// would have measured.
    fn backfill(
        &self,
        block: &CopyBlock,
        bounded: &[bool],
        allowance: Option<u64>,
    ) -> Option<StatisticsBackfill> {
        let requested = self.tracked_columns(&block.header)?;
        if let (Some(declined), Some(allowance)) = (block.statistics_declined, allowance)
            && allowance <= declined
        {
            return None;
        }
        let Some(held) = block.statistics.as_deref() else {
            return Some(self.gathering(requested.clone(), requested));
        };
        let stated_bound = self.min_rows.is_some() || self.max_rows.is_some();
        let resized = match self.group_size {
            Some(size) => size.get() != held.group_size,
            None => stated_bound && self.sizing() != held.sizing,
        };
        let unmet_max = !resized
            && held.group_size == self.group_size()
            && self.max_rows().is_some_and(|max_rows| held.breaks_max_rows(max_rows));
        let missing = requested.iter().enumerate().any(|(i, &wanted)| {
            wanted
                && held.columns.get(i).and_then(Option::as_ref).is_none_or(|column| {
                    column.bounds.is_none() && bounded.get(i).copied().unwrap_or(false)
                })
        });
        if !resized && !unmet_max && !missing {
            return None;
        }
        let columns = requested
            .iter()
            .enumerate()
            .map(|(i, &wanted)| wanted || held.columns.get(i).is_some_and(Option::is_some))
            .collect();
        if resized {
            return Some(self.gathering(columns, requested));
        }
        if unmet_max {
            let max_rows = self.max_rows().expect("`unmet_max` read it");
            return Some(StatisticsBackfill {
                group_size: held.predicted_group_size(max_rows),
                ..self.gathering(columns, requested)
            });
        }
        Some(StatisticsBackfill {
            columns,
            requested,
            group_size: held.group_size,
            group_cap: None,
            min_rows: None,
            max_rows: None,
            sizing: held.sizing,
        })
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
    /// Those of `columns` the request tracks. A field its type's `*_in`
    /// refuses is met as the pass's [`pgdt_model::PostgresInvalidValues`] says in
    /// these alone; in a column gathered only because the block held it, it
    /// is gone past and recorded as an ignoring pass records it
    /// ([`pgdt_model::IgnoredRefusals`]), so a refusing parse fails on exactly the columns
    /// it tracks, whichever runs gathered the block — but a strict one, which
    /// checks every column's fields first ([`pgdt_model::PostgresInvalidValues::Strict`]).
    pub requested: Vec<bool>,
    /// The group size gathered at: for a block re-read from its first row,
    /// the request's stated size, else [`ROW_GROUP_DEFAULT_SIZE_BYTES`]; for
    /// a block whose groups break a stated maximum, the finer size
    /// [`pgdt_model::BlockStatistics::predicted_group_size`] predicts; and for one lacking
    /// only a column, the size it already held.
    pub group_size: u64,
    /// The most groups the block may hold, adjacent ones merging pairwise
    /// past it — [`StatisticsRequest::group_cap`] for a block re-read from its
    /// first row — and `None` for a size gathered exactly: a stated one, or
    /// the one the block already held.
    pub group_cap: Option<usize>,
    /// The fewest rows the finished block's median group may hold, its groups
    /// merging pairwise short of it — [`StatisticsRequest::min_rows`] for a
    /// block re-read from its first row — and `None` for a size gathered
    /// exactly.
    pub min_rows: Option<u64>,
    /// The most rows the finished block's 90th-percentile group may hold, no
    /// merge passing it — [`StatisticsRequest::max_rows`] for a block re-read
    /// from its first row — and `None` for a size gathered exactly.
    pub max_rows: Option<u64>,
    /// What the gathered block records it was sized under: the request's for
    /// a block re-read from its first row, the block's own record for one
    /// re-read at the size it held.
    pub sizing: GroupSizing,
}

/// How much a mapping pass records of a table or a column
/// (`docs/design/decisions.md`, "D35").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatisticsLevel {
    /// A block's location and row count, and nothing drawn from its rows: no
    /// census, no statistics, and no field split but a strict parse's. A query
    /// of such a table maps its blocks again for itself ([`crate::stream::table_stream`]), and a
    /// replay over a caller's map refuses one.
    Metadata,
    /// The census, for a table any of whose columns is at this level, and
    /// statistics, for each column that is — the default.
    #[default]
    Data,
}

/// Each table's and column's [`StatisticsLevel`]: a default for every one,
/// then overrides, **the most specific entry naming a column deciding its
/// level** — a qualified column entry, then a bare one, then a qualified
/// table, then a bare one, then the default; among equally specific entries
/// naming one column, the last.
/// The type's `Default` is the data level everywhere
/// (`docs/design/decisions.md`, "D77").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatisticsSelection {
    /// The level of every table and column no override names.
    pub default: StatisticsLevel,
    /// Tables and columns at a level of their own.
    pub overrides: Vec<(StatisticsTarget, StatisticsLevel)>,
}

impl StatisticsSelection {
    /// Every table and column at the data level.
    pub const DATA: Self = Self { default: StatisticsLevel::Data, overrides: Vec::new() };

    /// Every table at the metadata level.
    pub const METADATA: Self = Self { default: StatisticsLevel::Metadata, overrides: Vec::new() };

    /// The level of `column` of `header`'s table, or with `None` of the table
    /// itself, which no column entry reaches.
    pub fn level(&self, header: &CopyHeader, column: Option<&str>) -> StatisticsLevel {
        let mut level = (0, self.default);
        for (target, stated) in &self.overrides {
            let rank = match target {
                StatisticsTarget::Column { table, column: named }
                    if column == Some(named.as_str()) && header.matches(table) =>
                {
                    3 + u8::from(table.contains('.'))
                }
                StatisticsTarget::Table(table) if header.matches(table) => {
                    1 + u8::from(table.contains('.'))
                }
                _ => continue,
            };
            if rank >= level.0 {
                level = (rank, *stated);
            }
        }
        level.1
    }
}

/// What one override names. A table is named as [`CopyHeader::matches`]
/// reads it, qualified or bare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatisticsTarget {
    /// Every column of the table.
    Table(String),
    /// One column of the table.
    Column { table: String, column: String },
}

/// The growth an observer holds uncharged before it updates the pass's
/// [`StatisticsAccount`], which it does, once past it, before it observes
/// another row — so one wide row can carry it further. A
/// judgement setting only how often a wide observer updates.
pub(crate) const STATISTICS_ACCOUNT_CHARGE_STEP: u64 = 64 << 10;

/// Every statistic a mapping pass holds alive, in bytes of heap, by term
/// (`docs/design/decisions.md`, "D81").
///
/// **An account, not a bound**: each term is the sizes the allocator was asked
/// for, summed as the structures change — a finished block once, as it is
/// retained or loaded; an observer still gathering whenever its uncharged
/// growth passes [`STATISTICS_ACCOUNT_CHARGE_STEP`], at every column it closes a group on and
/// through every piece it folds in; and a vector or an interning map ahead of
/// the allocation it grows into. Shared by every observer of one pass, the
/// leader's pieces on the blocking pool included, so it is one lock over its
/// terms: an update, its peak and the instrument's check are read whole, and
/// no reader sees another thread's update half applied.
///
/// **What it does not see** is bounded by an observer rather than by the dump:
/// each open observer's growth until it passes [`STATISTICS_ACCOUNT_CHARGE_STEP`], a row's own
/// decode scratch while a column observes it, a merge's scratch for the pair
/// of groups it is merging, a finished block's rows per group while its size is
/// chosen, what `finish` builds beside what it replaces — the block's column
/// list, and a clipped group's bounds — the observer's own allocation, its
/// record of the fields an ignoring parse went past, and a strict parse's
/// table of checks.
///
/// **It is also the bound a decline reads** (`docs/design/decisions.md`,
/// "D85"): where [`Self::allowance`] is stated and the terms pass it, the
/// observer whose update saw that declines, bar the close `finish` makes, so
/// what the account sums is what the margin is left against.
#[derive(Debug, Default)]
pub(crate) struct StatisticsAccount {
    state: Mutex<AccountState>,
    /// The bytes every statistic alive may hold between them
    /// ([`pgdt_dump::ScanOptions::statistics_allowance_bytes`]); `None` bounds
    /// nothing and declines nothing.
    allowance: Option<u64>,
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
    /// An account bounded by `allowance` — `None` bounding nothing, which is
    /// [`Self::default`] and every caller that states no allowance.
    pub(crate) fn bounded_by(allowance: Option<u64>) -> Self {
        Self { state: Mutex::default(), allowance }
    }

    /// The bytes every statistic alive may hold between them, where one was
    /// stated.
    pub(crate) fn allowance(&self) -> Option<u64> {
        self.allowance
    }

    /// Apply every change in `changes` as one update: the peak and the
    /// instrument's check read the account once all of them are in, so a
    /// charge moving from one term to another never reads as both or neither.
    ///
    /// Answers whether the terms now stand **over** [`Self::allowance`], read
    /// inside the same lock as the update that moved them.
    pub(crate) fn apply(&self, changes: &[(Term, i64)]) -> bool {
        self.update(changes, 0, 0)
    }

    /// [`Self::apply`], with `announced` moving what the terms carry for
    /// allocations not yet made and `observers` the count of charges alive,
    /// in the same update.
    fn update(&self, changes: &[(Term, i64)], announced: i64, observers: i64) -> bool {
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
        let tolerance = open * STATISTICS_ACCOUNT_CHARGE_STEP;
        instrument::statistics_account_updated(total, state.announced, tolerance);
        self.allowance.is_some_and(|allowance| total > allowance)
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

    /// The allowance this charge's account bounds every statistic alive by,
    /// where one was stated — what a declining observer records.
    pub(crate) fn allowance(&self) -> Option<u64> {
        self.account.allowance()
    }

    /// Charge `structure` bytes to this charge's term and `interned` to
    /// [`Term::Interned`], in place of what was charged before, and answer
    /// whether the account now stands over its allowance
    /// ([`StatisticsAccount::apply`]).
    pub(crate) fn set(&mut self, structure: u64, interned: u64) -> bool {
        let changes = [
            (self.term, structure as i64 - self.structure as i64),
            (Term::Interned, interned as i64 - self.interned as i64),
        ];
        (self.structure, self.interned) = (structure, interned);
        self.account.apply(&changes)
    }

    /// **Charge `grows` — structure and interned bytes — ahead of the
    /// allocation `allocate` makes, and release `frees` once it has freed
    /// them**: a vector or a table grown while the smaller one is live, or
    /// realloc'd, is charged before any other update can read the heap
    /// holding it. Until the second update, the instrument's check reads the
    /// allocation as made where it judges the account short and as not yet
    /// made where it judges it over (`pgdt_core::instrument`).
    pub(crate) fn ahead<R>(
        &mut self,
        grows: (u64, u64),
        frees: (u64, u64),
        allocate: impl FnOnce() -> R,
    ) -> R {
        self.ahead_freeing(grows, || (allocate(), frees))
    }

    /// [`Self::ahead`], for work that learns only as it runs what it frees:
    /// `allocate` answers it, and it is released in the same update as the
    /// allocation is judged made, so the account never reads freed bytes as
    /// held.
    pub(crate) fn ahead_freeing<R>(
        &mut self,
        grows: (u64, u64),
        allocate: impl FnOnce() -> (R, (u64, u64)),
    ) -> R {
        let announced = (grows.0 + grows.1) as i64;
        self.structure += grows.0;
        self.interned += grows.1;
        let changes = [(self.term, grows.0 as i64), (Term::Interned, grows.1 as i64)];
        self.account.update(&changes, announced, 0);
        let (made, frees) = allocate();
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
    /// dropped, less any a back-fill has replaced.
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
