use std::path::{Path, PathBuf};

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
    /// A `--source` argument names something this build does not read: a URL
    /// scheme it speaks nothing for, a `file:` URL naming another machine, a
    /// URL carrying a credential it would have to send, or a place whose kind
    /// the opener it reached does not serve. Raised by `crate::Origin`'s
    /// constructors and by `crate::open_local` / `crate::open_remote`, before
    /// anything is fetched.
    ///
    /// **It refuses by name**: `origin` is the argument as the user wrote it
    /// and `why` says which rule it met, so a mistyped scheme is a sentence
    /// rather than a fall-through to "no such file". The field is not called
    /// `source`, which `thiserror` reads as an error cause.
    #[error("{origin} cannot be read: {why}")]
    SourceNotReadable { origin: String, why: String },
    /// A request to a remote source failed: a refusal the server made, or a
    /// transport failure the client's own retries could not ride out. Nobody
    /// asked for it, so it is an error rather than a cancellation — treating
    /// one as a cancellation would make `parse` report partial success on a
    /// dump it could not read.
    ///
    /// **No progress is lost that was not already at risk.** The save throttle
    /// means the cache on disk holds the scan to its last save and a re-run
    /// resumes from it, so a network failure costs one throttle interval
    /// rather than the scan (`docs/design/decisions.md`, "D62"). What this
    /// variant owes is the naming: the URL, and the
    /// underlying failure rather than the backend's wording alone.
    #[error("{url}: {message}")]
    Remote { url: String, message: String },
    /// From `xz_seek`: a walk, a seek, or a block decode failed against an
    /// `.xz`-compressed [`crate::XzSource`]
    /// (`docs/design/decisions.md`, "The compressed source and the cache").
    /// The crate's own `Error::compressed_offset`/`uncompressed_range` carry
    /// the position; this variant only wraps and displays it.
    #[error("xz error: {0}")]
    Xz(#[from] xz_seek::Error),
    #[error("failed to encode structure cache: {0}")]
    CacheEncode(#[from] bincode::error::EncodeError),
    #[error("cache is disabled (`--dtcache none`), but `{operation}` requires a cache file")]
    CacheDisabled { operation: &'static str },
    #[error("cache mode mismatch: {0}")]
    CacheModeMismatch(&'static str),
    /// A scan was asked to build forward from a cache it cannot use — one
    /// that is not a pgdt cache, is damaged, is another build's, or describes
    /// another file — so it refuses rather than starting cold and overwriting
    /// it (`docs/design/decisions.md`, "D20"). Raised by the four scan entry
    /// points — `crate::map_file`, `crate::table_stream`,
    /// `crate::table_stream_partitions` and `crate::index::preamble_only` —
    /// before any byte of the dump is read, and by a caller pre-empting them,
    /// all through `crate::cache::CacheMode::refusal`, which answers none
    /// where the mode may overwrite the cache instead.
    #[error("{}", cache_unusable(path, unusable))]
    CacheUnusable { path: PathBuf, unusable: Unusable },
    /// The source changed while this run was reading it, so every byte the
    /// run has already read is suspect and a map or row set built from them
    /// could mix two versions of the file. Raised by
    /// [`crate::cache::SourceWatch`] — at a cache save, once when the run
    /// finishes, and in place of a failure met while reading, which the
    /// change often is (`crate::cache::SourceWatch::attribute`) — and an
    /// abort rather than a diagnostic on every provider,
    /// `crate::cache::StrictIdentity::NONE` being the only opt-out.
    ///
    /// **Nothing is saved and nothing is removed**: the check says when the
    /// change was *detected*, never when it happened, so this run's partial
    /// map and statistics are dropped, and the cache already on disk — which
    /// describes the file as it was — is left exactly as it is
    /// (`docs/design/decisions.md`, "D20").
    #[error(
        "the dump changed while it was being read — {differences} — so nothing was saved and no cache was removed; re-run against a file nothing is rewriting, or pass `--strict-identity=none` to read it anyway"
    )]
    SourceChangedWhileRead { differences: String },
    /// A weak identity signal the caller asked to *bind* does not hold, or
    /// cannot be had at all. Raised by `crate::cache::CacheMode::load` under
    /// `crate::cache::StrictIdentity::time` or `location`, where an advisory
    /// diagnostic would otherwise be. Under `time` a missing modification
    /// signal fails too, silence being what strict identity exists to refuse;
    /// under `location` two sources fetched from nowhere agree.
    ///
    /// `term` is which selector was not met — `time` or `location` — and
    /// `unmet` names what was compared, not only the verdict, so the refusal
    /// can be checked against the file without a second run:
    /// `crate::cache::WeakIdentity` and `crate::cache::OriginMatch` carry the
    /// evidence out of their comparisons for it.
    #[error(
        "the cache at {} cannot be trusted under `--strict-identity={term}`: {unmet} — drop `{term}` from the selection to treat that signal as advisory, or parse again",
        path.display()
    )]
    StrictIdentityUnmet { path: PathBuf, term: &'static str, unmet: String },
    /// A block re-read for the statistics it lacks did not end where the
    /// map records it ending, so the source was rewritten at the same stored
    /// size — which the cache's identity check cannot see
    /// (`docs/design/decisions.md`, "D21") — or, handed to the per-block
    /// entry point, is cut short of that end, and statistics gathered from it
    /// would describe other bytes than the map does. Raised before they are
    /// stored (`crate::stream::gather_block_statistics`), never worked around
    /// (`docs/design/decisions.md`, "D20").
    ///
    /// `path` is the cache the map was loaded from, named as
    /// [`Error::CacheUnusable`] names it, so a caller whose cache is not
    /// beside the dump knows which file to remove. `crate::map_file`'s
    /// back-fill always has one; the per-block entry point is handed a map
    /// with no cache attached, and leaves it `None`.
    #[error("{}", cached_block_changed(.path.as_deref(), *.header_offset))]
    CachedBlockChanged { path: Option<PathBuf>, header_offset: u64 },
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
    /// Two of one table's `COPY` blocks name different sets of columns, so no
    /// one schema holds both without stating values the dump never held. The
    /// order they name them in may differ; the set may not.
    #[error(
        "table `{table}` is written as COPY blocks naming different columns — the block at offset {header_offset} and the one at offset {other_offset}; one table's blocks must name the same columns"
    )]
    TableColumnsDisagree { table: String, header_offset: u64, other_offset: u64 },
    /// A replay over a map the caller holds was handed one that stops short
    /// of the source's end, so a table's blocks past that point would be
    /// missing from its rows and its schema without anything saying so. Raised
    /// by `crate::TablePartitions::plan` before a byte is read; building the
    /// rest of the map is the caller's to arrange, the replay never scans.
    #[error(
        "the map covers {scanned_through} of the source's {size} byte(s); a replay over a caller's map needs one that reaches the end of the file"
    )]
    MapIncomplete { scanned_through: u64, size: u64 },
    #[error(
        "metadata for database {} was not scanned — run `pgdt parse` first, or use --schema-mode strings",
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
    /// an array has no dump position to name. Two cases reach it: `interval`,
    /// Arrow's `Interval(MonthDayNano)` counting nanoseconds where
    /// PostgreSQL's field counts microseconds, and an `int2vector` holding a
    /// NULL element, which its text form has no encoding for.
    #[error("this Arrow value has no `{declared_type}` text form: {reason}")]
    FieldRender { declared_type: &'static str, reason: String },
}

/// [`Error::CachedBlockChanged`]'s sentence: the cache named where there is
/// one, with the remedy on it, and the map alone where there is not.
fn cached_block_changed(path: Option<&Path>, header_offset: u64) -> String {
    match path {
        Some(path) => format!(
            "the COPY block the cache at {} records at offset {header_offset} no longer ends where that cache says, so the file changed since it was scanned — remove that cache and parse again",
            path.display()
        ),
        None => format!(
            "the COPY block the map records at offset {header_offset} no longer ends where that map says, so the file changed since it was scanned"
        ),
    }
}

/// Why a cache at a path cannot be used — carried whole by `crate::cache::CacheStatus`,
/// `CacheLoad`, `CacheClaim` and [`Error::CacheUnusable`], so a caller
/// that reports it and one that refuses on it name the same reason
/// (`docs/design/decisions.md`, "D22").
///
/// **Every reason but one is recognisably ours**, and those are what
/// `crate::cache::CacheMode::with_overwrite_unusable` may replace; a file that is not a
/// pgdt cache is refused whatever is asked ([`Unusable::overwritable`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    /// The file does not open with the cache header's magic: something else is at this
    /// path. **Never overwritten.**
    NotACache,
    /// This build's header, over a rest that does not decode — a file cut
    /// short or damaged.
    Unreadable,
    /// Another build's cache: `found` is the format version its header
    /// states, where this build reads `crate::cache::CACHE_FORMAT_VERSION`; nothing
    /// migrates (`docs/design/roadmap.md`, "Pre-1.0").
    UnsupportedVersion { found: u32 },
    /// A readable cache whose recorded *stored* size disagrees with the live
    /// source's, so every byte offset in it could be wrong — the staleness
    /// check reads `crate::ByteRangeSource::stored_size`, not the addressable
    /// length (`docs/design/decisions.md`, "D21"). Both stored sizes are
    /// carried: they are the evidence a refusal states.
    SourceChanged { cached_stored_size: u64, live_stored_size: u64 },
    /// A cache whose compression layer is not the live source's — one says
    /// `.xz` and the other not, or the two seek tables differ — at the same
    /// stored size, so it was written for another file
    /// (`docs/design/decisions.md`, "D18"). Recognition reaches it first
    /// (`crate::Recognized::Mismatch`) wherever the cache's claim was handed
    /// to it; `crate::cache::load` answers it for a source opened without one.
    CompressionContradicted,
}

impl Unusable {
    /// Whether `crate::cache::CacheMode::with_overwrite_unusable` may replace a cache
    /// found in this state: every one that is recognisably a pgdt cache, a
    /// damaged one of this build's header included (`docs/design/decisions.md`, "D20").
    pub fn overwritable(&self) -> bool {
        !matches!(self, Unusable::NotACache)
    }
}

/// What a user can do about a cache that is recognisably ours and cannot be
/// used, in the words every such refusal ends with — the library's
/// [`Error::CacheUnusable`] and the sentences `pgdt info` prints
/// (`docs/design/decisions.md`, "D20").
pub const OVERWRITE_WAYS_OUT: &str = " — remove it, name a different cache path, or pass \
     `--overwrite-unusable-cache` to have a scan replace it";

/// What a scan refusing an unusable cache says: which way it cannot be used,
/// then what the user can do. **A cache recognisably ours names three ways
/// out, and a file that is not one two**, the flag never replacing it
/// (`crate::cache::Unusable::overwritable`).
fn cache_unusable(path: &Path, unusable: &Unusable) -> String {
    let path = path.display();
    let found = match unusable {
        Unusable::NotACache => {
            return format!(
                "{path} is not a pgdt cache, and no scan writes over a file it did not write — \
                 check the path, or name a different cache path"
            );
        }
        Unusable::Unreadable => format!("the cache at {path} is cut short or damaged"),
        Unusable::UnsupportedVersion { found } => {
            format!(
                "the cache at {path} was written by a different pgdt build (cache format {found})"
            )
        }
        Unusable::SourceChanged { cached_stored_size, live_stored_size } => format!(
            "the cache at {path} was written for a source of {cached_stored_size} byte(s), but \
             this source is {live_stored_size} byte(s), so it describes another file"
        ),
        Unusable::CompressionContradicted => format!(
            "the cache at {path} records compression details this source contradicts, so it was \
             written for another file"
        ),
    };
    format!("{found}{}", OVERWRITE_WAYS_OUT)
}
