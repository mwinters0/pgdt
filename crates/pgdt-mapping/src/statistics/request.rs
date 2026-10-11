//! What a mapping pass is asked to record (`docs/design/decisions.md`,
//! "D77"): each table's and column's level, the group size and the density
//! bounds a block's groups are held between.

use std::num::NonZeroU64;

use pgdt_core::CopyHeader;
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
/// (`gather::gatherer::density_merges`; `docs/design/decisions.md`, "D82").
/// `ROW_GROUP_DEFAULT_SIZE_BYTES` over a row a kibibyte wide; a judgement,
/// not a reading.
pub const ROW_GROUP_DEFAULT_MIN_ROWS: u64 = 1 << 10;

/// What a mapping pass is asked to record: each table's and column's level,
/// at what group size and between what density bounds
/// (`docs/design/decisions.md`, "D77"). An argument of
/// [`crate::map_file`] alone — a query never gathers.
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
    /// the 90th-percentile group ([`pgdt_model::internal::max_rows_group`]) — and **there is no
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

    /// What re-reading `block`, already mapped, must gather for it to hold
    /// what this request asks of it — `None` when it holds that already, or
    /// when the request tracks nothing in it.
    ///
    /// deficiency: KD115 — `bounded` must be `bounded_columns` of this same
    /// block and the metadata the re-read will resolve it against, and nothing
    /// checks that it is: a caller passing another block's mask, or one
    /// resolved against other metadata, is answered for columns the re-read
    /// will not bound as the mask says. `map_file`'s back-fill is the only
    /// caller today and computes both together. **(b) owned by P36**: closing
    /// it means this taking the metadata and computing the mask itself.
    ///
    /// **A block lacks the requested statistics** where it holds none — every
    /// block mapped at the metadata level that this request puts at the data
    /// level among them, the re-read censusing it too — where a
    /// column the request tracks was not gathered or was gathered without the
    /// bounds `bounded` says gathering keeps for it, positionally to the
    /// block's header ([`crate::bounded_columns`];
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
    pub fn backfill(
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
    /// of such a table maps its blocks again for itself (`pgdt_query::table_stream`), and a
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
