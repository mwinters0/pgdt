//! What a column's statistics are stored against: its columns resolved as a
//! gathering observer resolved them, its declaration and whether they are
//! believed under it, and which statistics a column of a type keeps — read by
//! gathering and by pruning and summaries above it.

use arrow_schema::DataType;

use pgdt_codec::compare::{CompareKind, ComparisonPlan, ComparisonSemantics};
use pgdt_codec::decode::typmod_unscaled_digits;
use pgdt_codec::pgtype::NestedPlan;
use pgdt_codec::{ResolvedSchema, SchemaMode, resolve_columns};
use pgdt_core::CopyHeader;
use pgdt_model::{ColumnDef, ColumnStatistics, CopyBlock, DumpMetadata};

/// The kinds each of `header`'s columns' stored sets of bounds are ordered
/// by, positionally, resolved as `gather::observer_tracking` resolves them — so for
/// a column whose statistics record the declared type and collation
/// `metadata` states now, the kinds its sets were gathered under
/// (`pgdt_query`'s pruning; `docs/design/decisions.md`, "D79").
pub fn stored_bounds_kinds(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> Vec<[Option<CompareKind>; 2]> {
    let resolved = stored_resolution(header, metadata, database);
    resolved.columns().iter().map(|column| column.bounds_kinds()).collect()
}

/// Which of `block`'s columns gathering keeps bounds and row order for,
/// positionally to its header, its columns resolved against `metadata` for
/// `database`, the one its span records, as a mapping pass resolves them — what
/// [`StatisticsRequest::backfill`](crate::statistics::StatisticsRequest::backfill)
/// reads a block's held statistics against, a column held without the bounds
/// this says it gets lacking them (`docs/design/decisions.md`, "D79").
pub(crate) fn bounded_columns(
    block: &CopyBlock,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> Vec<bool> {
    stored_bounds_kinds(&block.header, metadata, database)
        .iter()
        .map(|kinds| kinds[0].is_some())
        .collect()
}

/// `header`'s columns resolved as `gather::observer_tracking` resolves them, under
/// the DDL `metadata` states for it now: for a column whose statistics record
/// that declared type and collation, the resolution they were gathered under.
pub fn stored_resolution(
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
/// are each field's own text, as `ColumnGatherer::new` and
/// `DictionaryGatherer::observe` decide it: every dictionary kept but a
/// `character` column's, whose entries give up their trailing blanks.
pub fn dictionary_holds_field_text(stored: &ResolvedSchema, i: usize) -> bool {
    let column = &stored.columns()[i];
    let comparison = column.comparison();
    column.plan().is_scalar()
        && comparison.dictionary_answers_in(ComparisonSemantics::Postgres)
        && !matches!(
            comparison,
            ComparisonPlan::Compared { kind: CompareKind::PaddedText { .. }, .. }
        )
}

/// Whether gathering keeps sums for a column the typed read emits as
/// `data_type` ([`pgdt_model::ColumnStatistics::sums`]) — and so
/// whether a column a query emits so is the one they were summed as.
pub fn keeps_sums(data_type: &DataType) -> bool {
    Summand::of(data_type, &NestedPlan::Scalar).is_some()
}

/// The declaration `metadata` holds for column `name` of the table
/// `qualified` in `database`, wherever the table takes it from
/// ([`pgdt_model::DatabaseMetadata::declared_column`]) — what a column's
/// statistics record their declared type and collation from, and what a
/// query compares those against (`pgdt_query`'s pruning).
pub fn declared_column<'m>(
    metadata: Option<&'m DumpMetadata>,
    database: Option<&str>,
    qualified: &str,
    name: &str,
) -> Option<&'m ColumnDef> {
    metadata
        .and_then(|m| m.databases.iter().find(|db| db.name.as_deref() == database))
        .and_then(|db| db.declared_column(qualified, name))
}

/// Whether `column`'s bounds, row order and dictionary were gathered under the
/// declared type and `COLLATE` clause its declaration `def` still gives it:
/// the only DDL they are believed under (`docs/design/decisions.md`, "D78").
pub fn believed_under(column: &ColumnStatistics, def: Option<&ColumnDef>) -> bool {
    column.declared_type.as_deref() == def.map(|d| d.declared_type.as_str())
        && column.collation.as_deref() == def.and_then(|d| d.collation.as_deref())
}

/// How a column's values are summed: parsed as the typed read parses them
/// (`pgdt_query`'s batch), and widened to the 128 bits every sum is kept in
/// (`docs/design/decisions.md`, "D91").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Summand {
    Int16,
    Int32,
    Int64,
    UInt32,
    /// A `numeric(p,s)`'s value put through its typmod, unscaled at `s`
    /// places.
    Decimal128 {
        precision: u16,
        scale: i8,
    },
}

impl Summand {
    /// How a column the typed read emits as `data_type`, filled as `plan`
    /// says, is summed, or `None` for a type that keeps no sum.
    pub(crate) fn of(data_type: &DataType, plan: &NestedPlan) -> Option<Self> {
        match data_type {
            DataType::Int16 => Some(Self::Int16),
            DataType::Int32 => Some(Self::Int32),
            DataType::Int64 => Some(Self::Int64),
            DataType::UInt32 => Some(Self::UInt32),
            DataType::Decimal128(arrow, scale) => Some(Self::Decimal128 {
                precision: match plan {
                    NestedPlan::Decimal { precision } => *precision,
                    _ => u16::from(*arrow),
                },
                scale: *scale,
            }),
            _ => None,
        }
    }

    /// `text` as the integer the typed read appends, or `None` where the read
    /// would refuse it.
    pub(crate) fn value(self, text: &str) -> Option<i128> {
        match self {
            Self::Int16 => text.parse::<i16>().ok().map(i128::from),
            Self::Int32 => text.parse::<i32>().ok().map(i128::from),
            Self::Int64 => text.parse::<i64>().ok().map(i128::from),
            Self::UInt32 => text.parse::<u32>().ok().map(i128::from),
            Self::Decimal128 { precision, scale } => {
                typmod_unscaled_digits(text, precision, i16::from(scale)).ok()?.parse().ok()
            }
        }
    }
}
