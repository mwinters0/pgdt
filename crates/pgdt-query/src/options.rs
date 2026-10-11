//! What one table query asks for — its projection and its filter beside the
//! batching knobs — and how far its mapping scan walks
//! (`docs/design/decisions.md`, "Batches, streams and the leader").

use pgdt_codec::compare::ComparisonSemantics;
use pgdt_codec::{SchemaMode, UnrepresentableMode};
use pgdt_io::Parallelism;
use pgdt_model::{PostgresInvalidValues, StatisticsView, UnrepresentableTier};

use crate::predicate::Expr;

/// What one table query asks for, and how its batches are cut.
///
/// Both halves of a query live here — the projection and the filter terms
/// beside the batching knobs — rather than the query half arriving as
/// positional arguments
/// (`docs/design/decisions.md`, "Batches, streams and the leader").
#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// Which columns to materialize, by name, in the order given. `None`
    /// projects every column of the block; `Some(vec![])` projects none,
    /// which is the `COUNT(*)` shape — every batch then carries a row count
    /// and no arrays. Names are matched against the block's own column list
    /// exactly as [`crate::predicate::Predicate::column`] is, so a name the
    /// block does not carry is `Error::UnknownProjectionColumn` and a
    /// repeated name is `Error::DuplicateProjectionColumn`
    /// (`docs/design/decisions.md`, "D28").
    pub projection: Option<Vec<String>>,
    /// Post-parse row filter (`docs/design/decisions.md`, "D54"),
    /// as a boolean **expression** over single-column terms: a row is kept
    /// only if the root evaluates `Truth::True`. The default is the empty
    /// conjunction, which yields every row, so "no filter" needs no separate
    /// spelling.
    ///
    /// A term may name a column the projection does not: the projection
    /// decides what is *built*, never what may be tested.
    pub filter: Expr,
    /// Which order `filter`'s comparisons answer in: PostgreSQL's, by
    /// default, or DataFusion's order of the value each column emits, which a
    /// comparison this build cannot make that way refuses
    /// ([`ComparisonSemantics`]). Statistics prune a query in either, read
    /// only where they were gathered in its order.
    pub semantics: ComparisonSemantics,
    /// Rows per batch. A batch is flushed once it reaches this many rows.
    pub max_rows: usize,
    /// Optional cap on a batch's total field-byte count — counting only the
    /// fields a projection actually builds, since those are the bytes the
    /// batch holds. Whichever of this or `max_rows` is hit first flushes the
    /// batch.
    pub max_bytes: Option<usize>,
    /// Cap on the source byte span an in-flight batch covers — the distance
    /// from the start of its first selected row to the end of its latest, and
    /// the one trigger that bounds what a batch **pins** on a chunk-shaped
    /// source: the zero-copy `Utf8View` path holds a clone of every chunk it
    /// views until the batch flushes (`docs/design/decisions.md`, "D46").
    /// Defaults to 64 MiB, which no ordinary query reaches; `None` leaves a
    /// batch's span unbounded. What it bounds is the span rounded out to the
    /// retained unit, so on a block-shaped source, whose unit can exceed the
    /// cap, this is a batch-size knob and not the bound.
    ///
    /// **It is a ceiling, not the span a partitioned replay uses.**
    /// [`crate::table_stream_partitions`] derives the span it charges from the
    /// read-buffer budget and the requested worker count, so `jobs` readers
    /// are bought at the cost of batch size rather than declined
    /// (`docs/design/decisions.md`, "D84"). The derivation only moves down,
    /// and never narrows past **one announced read chunk**
    /// ([`pgdt_dump::ScanOptions::chunk_size_bytes`], which is what a
    /// chunk-shaped source retains) — a span stated below that floor is left
    /// where it was, the floor bounding the derivation rather than the
    /// result; a serial [`crate::table_stream`] does not derive at all.
    pub max_source_span: Option<usize>,
    /// Whether to resolve column types against the dump's DDL
    /// (`docs/design/decisions.md`, "D46"). Every `RecordBatch` this build
    /// produces carries the same schema as its query's
    /// [`pgdt_codec::ResolvedSchema`] — a column this build has no
    /// mapping for stays `Utf8View`, as `SchemaMode::Strings` maps every
    /// column.
    pub schema_mode: SchemaMode,
    /// How a value PostgreSQL accepts for a column's declared type and the
    /// column's Arrow type cannot hold is read ([`UnrepresentableMode`]):
    /// NULL, by default, its column read as its text, or refused. Which values those are is the
    /// semantics' to say — Arrow's format spec under PostgreSQL's, and past
    /// the engine's calendar too under DataFusion's
    /// (`docs/design/decisions.md`, "D98"). Moot under
    /// [`SchemaMode::Strings`], which reads every value as its text.
    pub unrepresentable: UnrepresentableMode,
    /// What a read does with a field its type's `*_in` refuses: fails on it,
    /// by default, or reads it as the decoders do, which only a float past
    /// its type's range, a `varchar(n)` or `char(n)` value past its length and
    /// a NULL in a `NOT NULL` column, or beneath it where a `NOT NULL` domain
    /// refuses one, are ([`PostgresInvalidValues`]). The
    /// typed read and every filter reading the field take it alike; a
    /// filter's literal is read as before.
    pub postgres_invalid_values: PostgresInvalidValues,
    /// Selects which database's table to query when the name alone is
    /// ambiguous — matched against `DatabaseMetadata::name`
    /// (`docs/design/decisions.md`, "D49"). `None` is the common case: a
    /// single-database dump, or an ambiguity a qualified name resolves.
    pub database: Option<String>,
    /// How far a query's mapping scan walks before it starts returning rows
    /// — see [`ScanExtent`].
    pub scan_extent: ScanExtent,
    /// How much concurrency this query may use, and what it may hold while
    /// it does — [`Parallelism::Serial`] by default, and stated here as well
    /// as on [`ScanOptions`](pgdt_dump::ScanOptions) because a query runs two passes with different
    /// shapes: a mapping scan, and a replay split into partitions
    /// (`docs/design/decisions.md`, "D1"). The replay loop announces it to
    /// the source ([`pgdt_io::ByteRangeSource::hint_parallelism`]), so the two
    /// passes are bounded separately, and `jobs`, capped by what the bytes
    /// afford, is how many sub-streams [`crate::table_stream_partitions`]
    /// hands back. Nothing here spawns — the caller runs them.
    pub parallelism: Parallelism,
    /// Whether the replay skips the row groups whose statistics — gathered by
    /// a mapping pass ([`pgdt_mapping::map_file`]) — prove no row satisfies
    /// `filter`, and stops reading a block sorted on a column `filter` bounds
    /// at its first row past the bound. On by default; the rows are the same
    /// either way, and `false` reads every row of every block, which is also
    /// how a value it would not read still raises its decode failure
    /// (`docs/design/decisions.md`, "D54"). What was skipped is a
    /// [`crate::stream::PlanNoteKind::StatisticsPruned`]; a stop is found only
    /// as rows are read, so what it left unread is reported after the fact
    /// ([`crate::stream::TableStream::early_stops`]).
    pub use_statistics: bool,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            projection: None,
            filter: Expr::default(),
            semantics: ComparisonSemantics::default(),
            max_rows: 8192,
            max_bytes: None,
            max_source_span: Some(64 << 20),
            schema_mode: SchemaMode::default(),
            unrepresentable: UnrepresentableMode::default(),
            postgres_invalid_values: PostgresInvalidValues::default(),
            database: None,
            scan_extent: ScanExtent::default(),
            parallelism: Parallelism::default(),
            use_statistics: true,
        }
    }
}

impl QueryOptions {
    /// **How this query's statistics are read where a column holds a value
    /// its type cannot**, as its reads and its filter take one: as NULL under
    /// [`UnrepresentableMode::Null`], in the tiers its semantics cannot hold
    /// ([`Self::unrepresentable_reach`]), and otherwise every value in
    /// PostgreSQL's order — under the refuse mode; under the untyped mode,
    /// whose columns holding one are text and compare in that order in
    /// PostgreSQL's semantics, and read no bounds in DataFusion's; and under
    /// [`SchemaMode::Strings`], whose text holds every value
    /// (`docs/design/decisions.md`, "D97", "D98").
    pub fn statistics_view(&self) -> StatisticsView {
        match (self.schema_mode, self.unrepresentable, self.unrepresentable_reach()) {
            (SchemaMode::Typed, UnrepresentableMode::Null, UnrepresentableTier::Format) => {
                StatisticsView::Representable
            }
            (SchemaMode::Typed, UnrepresentableMode::Null, UnrepresentableTier::Engine) => {
                StatisticsView::Displayable
            }
            _ => StatisticsView::Every,
        }
    }

    /// **The tiers of value this query's front end cannot hold**, named by the
    /// wider: every layer holds a value to Arrow's format spec, and DataFusion
    /// — whose comparison [`ComparisonSemantics::DataFusion`] is — cannot display
    /// a `date` or timestamp past [`pgdt_model::calendar_end`] either
    /// (`docs/design/decisions.md`, "D98").
    pub(crate) fn unrepresentable_reach(&self) -> UnrepresentableTier {
        match self.semantics {
            ComparisonSemantics::Postgres => UnrepresentableTier::Format,
            ComparisonSemantics::DataFusion => UnrepresentableTier::Engine,
        }
    }
}

/// How far [`crate::stream::table_stream`]'s mapping scan walks
/// (`docs/design/decisions.md`, "D48"). Rows are always replayed from blocks the map
/// already holds, so this controls how much of the file a query pays to map
/// before any row comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanExtent {
    /// Stop as soon as the queried table is settled: at least one matching
    /// block has closed, none of the matching blocks carried a partition-root
    /// marker (I2 — a marked block's name owns further blocks that are *not*
    /// adjacent, so only EOF enumerates them), and the map holds no
    /// `\connect`, after which a name can be defined again. This is what
    /// keeps a query against an early table in a huge dump from costing a
    /// full scan.
    ///
    /// What it gives up: a second, conflicting candidate past the stopping
    /// point is never seen, so `Error::AmbiguousTable` reports only what the
    /// scan reached. A file whose *first* segment has no `\connect` is the
    /// case with no early signal at all.
    ///
    /// Deficiency register: `deficiency: KD6` — so a second, conflicting
    /// table past the stopping point is never seen, `Error::AmbiguousTable` is
    /// not raised for it, and the query answers with the candidate it found.
    /// **(c) unowned**; promoted by a concatenated or `pg_dumpall`-style file
    /// reaching a user through a cold query. [`Self::Full`], or a query after
    /// `pgdt parse`, gives exact detection today, which is why the DataFusion
    /// provider, reading only a complete map, never shows it; closing it by
    /// default means giving up the early stop.
    #[default]
    UntilTargetSettled,
    /// Map the whole file before returning anything. Costs a full scan and
    /// gives exact ambiguity detection and a complete, reusable cache — the
    /// same map `pgdt parse` builds.
    Full,
}

#[cfg(test)]
mod tests;
