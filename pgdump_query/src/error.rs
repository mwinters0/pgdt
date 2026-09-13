use std::path::PathBuf;

use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("COPY block at offset {header_offset} has no terminating `\\.` line")]
    UnterminatedCopyBlock { header_offset: u64 },
    #[error("large-object data region at offset {start_offset} has no terminating `COMMIT;` line")]
    UnterminatedLargeObjectRegion { start_offset: u64 },
    #[error("line at offset {offset} exceeds the {limit}-byte line limit")]
    LineTooLong { offset: u64, limit: usize },
    #[error("scan cancelled at byte {scanned_through}")]
    ScanCancelled { scanned_through: u64 },
    #[error("value is not valid UTF-8 (valid up to byte {valid_up_to})")]
    InvalidUtf8 { valid_up_to: usize },
    #[error(
        "row at offset {row_offset} (COPY block at {header_offset}) has {found} column(s), expected {expected}"
    )]
    ColumnCountMismatch { header_offset: u64, row_offset: u64, expected: usize, found: usize },
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),
    /// From `xz_seek`: a walk, a seek, or a block decode failed against an
    /// `.xz`-compressed [`crate::XzSource`]
    /// (`docs/design/decisions.md`, "The compressed source and the cache").
    /// The crate's own `Error::compressed_offset`/`uncompressed_range` carry
    /// the position; this variant only wraps and displays it.
    #[error("xz error: {0}")]
    Xz(#[from] xz_seek::Error),
    #[error("failed to encode structure cache: {0}")]
    CacheEncode(#[from] bincode::error::EncodeError),
    #[error("cache is disabled (`--dqcache none`), but `{operation}` requires a cache file")]
    CacheDisabled { operation: &'static str },
    #[error("cache mode mismatch: {0}")]
    CacheModeMismatch(&'static str),
    /// A scan was asked to build forward from a cache that does not describe
    /// the source it was handed, so it refuses rather than scanning and
    /// overwriting it (`docs/design/decisions.md`, "D20"). Raised by
    /// the three scan entry points — `crate::map_file`,
    /// `crate::table_stream`, `crate::index::preamble_only` — before any byte
    /// of the dump is read, from
    /// [`crate::cache::CacheLoad::SourceChanged`]'s two sizes plus the path
    /// the mode resolved. It is the one unusable cache outcome that is an
    /// error rather than a cold start.
    #[error(
        "the cache at {} was written for a source of {cached_stored_size} byte(s), but this source is {live_stored_size} byte(s), so scanning would overwrite a cache for another file — remove it, or name a different cache path",
        path.display()
    )]
    CacheSourceMismatch { path: PathBuf, cached_stored_size: u64, live_stored_size: u64 },
    #[error("predicate column `{column}` not found in COPY block at offset {header_offset}")]
    UnknownPredicateColumn { header_offset: u64, column: String },
    #[error(
        "`{op}` on column `{column}` in the COPY block at offset {header_offset}: {reason}; use `=` or `!=` for a text comparison"
    )]
    UnorderedPredicateColumn {
        header_offset: u64,
        column: String,
        op: &'static str,
        /// Why the column has no order — one of `crate::predicate`'s three
        /// static sentences, or, for a nested column whose *shape* compares
        /// here, one naming the position beneath it that does not.
        reason: String,
    },
    /// **Every** operator refused, `=` and `!=` included — a different fault
    /// from the one above, which ends by offering the text comparison that is
    /// unavailable here. Raised where the file *states* that the server's
    /// equality is not a comparison of the text it holds
    /// (`crate::pgtype::UnanswerableReason`), so answering bytewise would be
    /// a wrong answer rather than a weaker one.
    #[error(
        "`{op}` on column `{column}` in the COPY block at offset {header_offset}: {reason}; no operator can be answered for this column, `=` and `!=` included"
    )]
    UncomparablePredicateColumn {
        header_offset: u64,
        column: String,
        op: &'static str,
        /// Which of `crate::pgtype::UnanswerableReason`'s cases it is, worded
        /// beside the comparison it is about.
        reason: String,
    },
    #[error(
        "filter value `{value}` for `{column} {op} ...` does not parse as the column's declared type `{declared_type}`, which is written {accepted}"
    )]
    PredicateValueDecode {
        column: String,
        op: &'static str,
        value: String,
        declared_type: String,
        /// The form the column's comparison actually reads, so the sentence
        /// is about this build's grammar rather than about the type: a
        /// `boolean` is refused `true` because it is written `t` or `f`, not
        /// because `boolean` has no such value. Supplied by
        /// `crate::predicate::accepted_form`, which sits beside the grammar
        /// it describes. A `String` because two arms answer with the column's
        /// own comparison payload — an enum's declared labels, a
        /// `numeric(p,s)`'s scale.
        accepted: String,
    },
    #[error("projected column `{column}` not found in COPY block at offset {header_offset}")]
    UnknownProjectionColumn { header_offset: u64, column: String },
    #[error("projection names column `{column}` more than once")]
    DuplicateProjectionColumn { column: String },
    #[error(
        "resume token belongs to a different query — the table, projection, filter and schema mode must all match the stream being resumed"
    )]
    ResumeQueryMismatch,
    #[error(
        "table `{name}` is ambiguous: matches {}; qualify the name or pass --database",
        candidates.join(", ")
    )]
    AmbiguousTable { name: String, candidates: Vec<String> },
    #[error(
        "metadata for database {} was not scanned — run `pgdq parse` first, or use --schema-mode strings",
        database.as_deref().unwrap_or("(unnamed)")
    )]
    MetadataNotScanned { database: Option<String> },
    #[error(
        "{table}.{column} at row offset {row_offset}: value `{value}` does not parse as its mapped type `{declared_type}` — use --schema-mode strings to read this column verbatim"
    )]
    FieldDecode {
        table: String,
        column: String,
        row_offset: u64,
        declared_type: String,
        value: String,
    },
    /// [`Self::FieldDecode`]'s mirror, met on the way **out**: an Arrow array
    /// holds a value no PostgreSQL text form spells, so `render_field`
    /// refuses rather than writing something the file could not have held
    /// (`docs/design/decisions.md`, "D44").
    ///
    /// It names the Arrow value rather than a table, column and row offset
    /// because nothing this crate scans can reach it — every typed column it
    /// fills comes from a `decode_*`, whose range is what its `render_*` can
    /// write back. Only an array a caller built itself carries one, and such
    /// an array has no dump position to name. `interval` is its one case:
    /// Arrow's `Interval(MonthDayNano)` counts nanoseconds where
    /// PostgreSQL's field counts microseconds.
    #[error("this Arrow value has no `{declared_type}` text form: {reason}")]
    FieldRender { declared_type: &'static str, reason: String },
}
