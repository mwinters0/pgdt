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
    #[error("failed to encode structure cache: {0}")]
    CacheEncode(#[from] bincode::error::EncodeError),
    #[error("cache is disabled (`--dqcache none`), but `{operation}` requires a cache file")]
    CacheDisabled { operation: &'static str },
    #[error("cache mode mismatch: {0}")]
    CacheModeMismatch(&'static str),
    #[error("predicate column `{column}` not found in COPY block at offset {header_offset}")]
    UnknownPredicateColumn { header_offset: u64, column: String },
    #[error(
        "`{op}` on column `{column}` in the COPY block at offset {header_offset}: {reason}; use `=` or `!=` for a text comparison"
    )]
    UnorderedPredicateColumn {
        header_offset: u64,
        column: String,
        op: &'static str,
        reason: &'static str,
    },
    #[error(
        "filter value `{value}` for `{column} {op} ...` does not parse as the column's declared type `{declared_type}`, which is written {accepted}"
    )]
    PredicateValueDecode {
        column: String,
        op: &'static str,
        value: String,
        declared_type: String,
        /// The form the column's comparison actually reads, named so the
        /// sentence is about this build's grammar rather than about the type:
        /// a `boolean` is refused `true` because it is written `t` or `f`,
        /// not because `boolean` has no such value. Supplied by
        /// `crate::predicate::accepted_form`, which sits beside the grammar
        /// it describes.
        ///
        /// A `String` rather than a `&'static str` because two arms answer
        /// with the column's own comparison payload — an enum's declared
        /// labels, a `numeric(p,s)`'s scale — and those are the arms where
        /// the payload *is* the answer. The clause is formatted at the raise
        /// site so the rendering stays beside the grammar; carrying a
        /// comparison type here instead would point this module at one that
        /// sits above it.
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
}
