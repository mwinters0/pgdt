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
