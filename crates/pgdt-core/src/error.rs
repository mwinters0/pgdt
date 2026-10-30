use std::path::{Path, PathBuf};

use thiserror::Error as ThisError;

/// A dependency's error, carried opaquely: the library's failures are its own
/// vocabulary, so no variant names a dependency's type, and a caller wanting
/// the cause downcasts it (`docs/design/decisions.md`, "D74").
pub type Cause = Box<dyn std::error::Error + Send + Sync + 'static>;

/// What the library refuses. A sentence naming a remedy names it in the
/// library's own terms, never a front end's flag, which each front end adds
/// when it reports one (`docs/design/decisions.md`, "D107").
///
/// **A variant carrying a cause displays it and never chains it**: no variant
/// has a [`source`](std::error::Error::source), so a reporter walking the
/// chain prints each message once, and one printing `Display` alone — as
/// DataFusion's `External` does — still prints it whole. A typed cause is
/// reached through the variant's field ([`Error::codec_cause`])
/// (`docs/design/decisions.md`, "D129").
#[derive(Debug, ThisError)]
pub enum Error {
    /// An I/O failure on no file a user named: a source's own refusal worded
    /// as one, or a read through something that does not know its path. One
    /// on a named file is [`Error::File`].
    #[error("io error: {0}")]
    Io(std::io::Error),
    /// An I/O failure on a file the user named — the dump, the cache, or a
    /// file written beside the cache — naming its path and what was being
    /// done to it, `doing` a phrase ending in the word the path follows:
    /// `open the dump at`. Raised at every open, `stat` and plain read of a
    /// local source, every read a decode makes of a local compressed one or
    /// of a cache's gzip windows — carried through the codec's crate in an
    /// `io::Error` ([`Error::into_io`]) — every open, read and write of a cache
    /// and of the file a save writes beside it, and where a gzip first pass
    /// cannot make, write or read back the file it holds its windows in,
    /// naming its directory ([`Error::file`]).
    ///
    /// The `io::Error` is reached as the `cause` field; it is not called
    /// `source`, which `thiserror` reads as an error cause.
    #[error("cannot {doing} {}: {cause}", path.display())]
    File { doing: &'static str, path: PathBuf, cause: std::io::Error },
    /// A background task panicked or was cancelled; the cause is tokio's
    /// `JoinError` ([`Error::join`]).
    #[error("background task failed: {0}")]
    Join(Cause),
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
    /// Arrow refused something; the cause is its `ArrowError`
    /// ([`Error::arrow`]).
    #[error("arrow error: {0}")]
    Arrow(Cause),
    /// A `--source` argument names something this build does not read: a URL
    /// scheme it speaks nothing for, a `file:` URL naming another machine, a
    /// URL carrying a credential it would have to send, or a place whose kind
    /// the opener it reached does not serve, or a file
    /// `pgdt_io::CompressedFileSource::open` is handed that is not one it
    /// opens. Raised by `pgdt_io::Origin`'s constructors and by the per-place
    /// openers below `pgdt_io::open`, before anything is fetched, and by
    /// `CompressedFileSource::open` after its leading bytes.
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
    /// From a codec's crate: an index build, a seek, or a unit decode failed
    /// against a compressed source — for `.xz`, `xz_seek`
    /// (`docs/design/decisions.md`, "The compressed source and the cache").
    /// The codec's own error carries the position; this variant names the
    /// codec and wraps and displays the cause ([`Error::codec`]).
    #[error("{codec} error: {cause}")]
    Codec {
        /// The codec's name, as a message says it: `xz`.
        codec: &'static str,
        cause: Cause,
    },
    /// The cache did not encode; the cause is bincode's `EncodeError`
    /// ([`Error::cache_encode`]).
    #[error("failed to encode structure cache: {0}")]
    CacheEncode(Cause),
    /// An operation needing a cache file asked of a cache mode that is
    /// disabled (`pgdt_dump::cache::CacheMode::require_enabled`).
    #[error("`{operation}` requires a cache file, and the cache is disabled — name one")]
    CacheDisabled { operation: &'static str },
    /// A scan was asked to build forward from a cache it cannot use — one
    /// that is not a pgdt cache, is damaged, is another build's, or describes
    /// another file — so it refuses rather than starting cold and overwriting
    /// it (`docs/design/decisions.md`, "D20"). Raised by an open through the
    /// cache (`pgdt_dump::cache::CacheMode::open`), which every scan entry
    /// point — `pgdt_mapping::map_file`, `pgdt_query::Table::settle` and
    /// `pgdt_dump::preamble_only` — reads, before any byte of the dump is
    /// read past its magic wherever the origin's probe answered (one that
    /// failed and then answered the open has an `.xz` walked first), through
    /// `pgdt_dump::cache::CacheMode::refusal`, which answers none where the
    /// mode may overwrite the cache instead; and by
    /// `pgdt_dump::cache::CacheMode::open_source`, over the source its caller
    /// built — and by `pgdt info`'s cache-only read, over a cache it cannot
    /// use.
    #[error("{}", cache_unusable(path, unusable))]
    CacheUnusable { path: PathBuf, unusable: Unusable },
    /// The source changed while this run was reading it, so every byte the
    /// run has already read is suspect and a map or row set built from them
    /// could mix two versions of the file. Raised by a remote source whose
    /// server refuses a request's precondition, and by
    /// [`pgdt_dump::cache::SourceWatch`] — at a cache save, once when the run
    /// finishes, and in place of a failure met while reading, which the
    /// change often is (`pgdt_dump::cache::SourceWatch::attribute`) — and an
    /// abort rather than a diagnostic on every provider,
    /// `pgdt_dump::cache::StrictIdentity::NONE` being the only opt-out.
    ///
    /// **Nothing is saved and nothing is removed**: the check says when the
    /// change was *detected*, never when it happened, so this run's partial
    /// map and statistics are dropped, and the cache already on disk — which
    /// describes the file as it was — is left exactly as it is
    /// (`docs/design/decisions.md`, "D20").
    #[error(
        "the dump changed while it was being read — {differences} — so nothing was saved and no cache was removed; re-run against a file nothing is rewriting, or read it anyway binding no identity signal"
    )]
    SourceChangedWhileRead { differences: String },
    /// A run that binds the in-flight identity — every
    /// `pgdt_dump::cache::StrictIdentity` but `NONE` — over a source nothing can
    /// check during it: a remote server stating neither a strong entity tag
    /// nor a `Last-Modified`, so no read can be pinned and the source's size
    /// and modification time are the probe's. Raised by
    /// `pgdt_dump::cache::SourceWatch::require_checkable` where a read begins,
    /// or where the DataFusion provider registers the dump, before anything is
    /// read, since the
    /// promise [`Error::SourceChangedWhileRead`] makes would otherwise be
    /// stated and not kept (`docs/design/decisions.md`, "D21"). `why` is the
    /// source's own sentence ([`pgdt_io::ByteRangeSource::in_flight_unchecked`]).
    #[error(
        "nothing can tell whether the dump changes while it is being read — {why} — so the run was refused before reading it; read it anyway binding no identity signal"
    )]
    SourceUncheckable { why: String },
    /// A weak identity signal the caller asked to *bind* does not hold, or
    /// cannot be had at all. Raised by a load through `pgdt_dump::cache::CacheMode` under
    /// `pgdt_dump::cache::StrictIdentity::time` or `location`, where an advisory
    /// diagnostic would otherwise be. Under `time` a missing modification
    /// signal fails too, silence being what strict identity exists to refuse,
    /// and so do equal entity tags either of which is weak, which confirm
    /// nothing; under `location` two sources fetched from nowhere agree.
    ///
    /// `term` is which selector was not met — `time` or `location` — and
    /// `unmet` names what was compared, not only the verdict, so the refusal
    /// can be checked against the file without a second run:
    /// `pgdt_dump::cache::WeakIdentity` and `pgdt_dump::cache::OriginMatch` carry the
    /// evidence out of their comparisons for it.
    #[error(
        "the cache at {} cannot be trusted with `{term}` bound: {unmet} — map the file again, or leave `{term}` unbound to treat that signal as advisory",
        path.display()
    )]
    StrictIdentityUnmet { path: PathBuf, term: &'static str, unmet: String },
    /// A block re-read — for the statistics it lacks, or for a typed query's
    /// array census — did not end where the map records it ending, so the source was rewritten at the same stored
    /// size — which the cache's identity check cannot see
    /// (`docs/design/decisions.md`, "D21") — or is cut short of that end, and
    /// what was read would describe other bytes than the map does. Raised
    /// before it is used (`pgdt_mapping`'s `reread_rows`), never worked around
    /// (`docs/design/decisions.md`, "D20").
    ///
    /// `path` is the cache the map was loaded from, named as
    /// [`Error::CacheUnusable`] names it, so a caller whose cache is not
    /// beside the dump knows which file to remove. `pgdt_mapping::map_file`'s
    /// back-fill has one under an enabled cache; the per-block entry point is handed a map
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
        /// Why the column has no order — one of `pgdt_query::predicate`'s three
        /// static sentences, or, for a nested column whose *shape* compares
        /// here, one naming the position beneath it that does not.
        reason: String,
    },
    /// **Every** operator refused, `=` and `!=` included — a different fault
    /// from the one above, which ends by offering the text comparison that is
    /// unavailable here. Raised where the file *states* that the server's
    /// equality is not a comparison of the text it holds
    /// (`pgdt_codec::compare::UnanswerableReason`), so answering bytewise would be
    /// a wrong answer rather than a weaker one — and for any nested column
    /// under DataFusion's semantics.
    #[error(
        "`{op}` on column `{column}` in the COPY block at offset {header_offset}: {reason}; no operator can be answered for this column, `=` and `!=` included"
    )]
    UncomparablePredicateColumn {
        header_offset: u64,
        column: String,
        op: &'static str,
        /// Which of `pgdt_codec::compare::UnanswerableReason`'s cases it is, or
        /// the nested-under-DataFusion refusal, worded beside the comparison it
        /// is about.
        reason: String,
    },
    #[error(
        "filter value `{value}` for `{column} {op} ...` does not parse as the column's declared type `{declared_type}`, which is written {accepted}{}",
        unlisted_labels(*.unlisted)
    )]
    PredicateValueDecode {
        column: String,
        op: &'static str,
        value: String,
        declared_type: String,
        /// The form the column's comparison actually reads, so the sentence is
        /// about this build's grammar rather than about the type: a `boolean`
        /// is refused `true` because it is written `t` or `f`, not because
        /// `boolean` has no such value. Supplied by
        /// `pgdt_codec::internal::accepted_form`, which sits in the codec's
        /// `key` beside the literal reader whose grammar it describes. A `String` because two arms answer with the column's
        /// own comparison payload — an enum's declared labels, a
        /// `numeric(p,s)`'s scale.
        accepted: String,
        /// How many of an enum's declared labels `accepted` leaves unlisted,
        /// zero where it lists the whole form: the rest are in the type's
        /// declaration, which a front end names its own way of printing
        /// (`docs/design/decisions.md`, "D107"). From
        /// `pgdt_codec::internal::unlisted_labels`.
        unlisted: usize,
    },
    #[error("projected column `{column}` not found in COPY block at offset {header_offset}")]
    UnknownProjectionColumn { header_offset: u64, column: String },
    #[error("projection names column `{column}` more than once")]
    DuplicateProjectionColumn { column: String },
    #[error(
        "resume token belongs to a different query — a token resumes only the query that issued it"
    )]
    ResumeQueryMismatch,
    #[error(
        "table `{name}` is ambiguous: matches {}; qualify the name or name its database",
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
    /// A dump read through its map was handed one that stops short of the
    /// source's end, or none, so a table's blocks past that point would be
    /// missing from its rows and its schema without anything saying so. Raised
    /// by `pgdt_query::Dump`'s constructors before a byte is read; building
    /// the rest of the map is a mapping pass's, the dump never scans.
    /// `size` is `None` where the source's end is not known yet.
    #[error(
        "the map covers {scanned_through} of {}; a dump read through its map needs one that reaches the end of the file",
        map_total(*.size)
    )]
    MapIncomplete { scanned_through: u64, size: Option<u64> },
    /// A typed plan over a dump's complete map was handed a table that map
    /// holds at the metadata level, which records no census of its rows, so
    /// no schema can be settled for it (`docs/design/decisions.md`, "D35").
    /// Raised by `pgdt_query::Table`'s `plan`, `schema`, `summary` and
    /// `divergences` before a byte is read; a query that maps for itself
    /// under the typed schema mode reads the rows again instead, one settled
    /// under the strings mode and planned typed raising it.
    #[error(
        "table `{table}` is mapped at the metadata level, which records nothing drawn from its rows, and a typed query over a map it does not build needs the data level's census — map it again at the data level, or read it in the strings schema mode"
    )]
    TableAtMetadataLevel { table: String },
    #[error(
        "metadata for database {} was not scanned — map it first, or read it in the strings schema mode",
        database.as_deref().unwrap_or("(unnamed)")
    )]
    MetadataNotScanned { database: Option<String> },
    /// A field a query decodes that does not parse as its column's mapped
    /// type. The line is named by its byte offset in the file alone: a query
    /// reading from a kept row group or a parallel partition starts mid-block,
    /// with no count of the rows before it to number the line by.
    #[error(
        "{table}.{column}: the line at offset {line_offset} holds `{value}`, which does not parse as its mapped type `{declared_type}` — read this column verbatim in the strings schema mode"
    )]
    FieldDecode {
        table: String,
        column: String,
        line_offset: u64,
        declared_type: String,
        value: String,
    },
    /// A field its declared type's `*_in` refuses, met by a pass decoding it
    /// — a parse gathering statistics, or a back-fill re-reading a block for
    /// them — under [`pgdt_model::PostgresInvalidValues::Default`], which fails at
    /// the first, as a restore under `ON_ERROR_STOP`
    /// fails the table's `COPY` (`docs/design/roadmap.md`, "A literal is
    /// guaranteed in `*_out`'s form and never read past `*_in`'s"). The line
    /// is named as the restore's own error names it — `line` is `COPY`'s
    /// count from the block's first data line (`copyfrom.c`'s
    /// `CopyFromErrorCallback`), a text row being one line — and by its byte
    /// offset in the file beside it, which is what a dump this size is
    /// seeked by.
    #[error(
        "COPY {table}, line {line}, column {column}: `{value}` is refused by PostgreSQL as a `{declared_type}` value — restoring this dump fails this table's COPY there, and so does this read; the line is at offset {line_offset}"
    )]
    FieldRefused {
        table: String,
        column: String,
        declared_type: String,
        line: u64,
        line_offset: u64,
        value: String,
    },
    /// A `COPY` row ending otherwise than its block's first row, which a
    /// restore refuses (I91) — wherever a pass enters the block at its header,
    /// the field's `--postgres-invalid-values` aside, the row being framing
    /// rather than a value. `line` is `COPY`'s count, as
    /// [`Self::FieldRefused`]'s is.
    #[error(
        "COPY {table}, line {line}: {refusal} — its block's first row ends otherwise, and restoring this dump fails this table's COPY there, and so does this read; the line is at offset {line_offset}"
    )]
    RowEndingRefused { table: String, line: u64, line_offset: u64, refusal: RowEndingRefusal },
    /// A NULL in a column its declaration makes `NOT NULL` — on the column, at
    /// the table, through a parent or through a domain — which a restore
    /// refuses (I76): [`Self::FieldRefused`]'s counterpart, met wherever such
    /// a field is read but under [`pgdt_model::PostgresInvalidValues::Ignore`].
    /// `line` is `COPY`'s count where a pass numbers it, as
    /// [`Self::FieldRefused`]'s is, and `None` for a query, which names the
    /// line by its offset alone, as [`Self::FieldDecode`] does.
    #[error("{}", null_refused(table, column, *line, *line_offset))]
    NullRefused { table: String, column: String, line: Option<u64>, line_offset: u64 },
    /// [`Self::FieldRefused`], read from the cache rather than the dump: a
    /// parse under [`pgdt_model::PostgresInvalidValues::Default`] whose cache
    /// records a field an earlier parse ignoring such fields went past
    /// ([`pgdt_model::CopyBlock::ignored_refusals`]) fails with the first it
    /// records in a column its request tracks, before reading anything — the
    /// verdict being the dump's, not that of whichever run gathered the cache.
    /// `refused` is that refusal as a read of the dump would have raised it.
    #[error(
        "{refused}. An earlier parse that went past the values PostgreSQL refuses recorded it in the cache {}, and nothing was re-read",
        cache.display()
    )]
    FieldRefusedRecorded { refused: Box<Error>, cache: Box<std::path::Path> },
    /// A column the query materializes holds values PostgreSQL accepts for
    /// its declared type and the query's front end cannot hold — past its
    /// Arrow type or, where the front end counts it, the calendar DataFusion
    /// displays — and the query is
    /// under [`pgdt_codec::UnrepresentableMode::Refuse`]: refused at planning,
    /// before a row is read, wherever the map counts one in the table, not
    /// only in the groups a filter keeps. `values` is the map's count in the
    /// tiers the query's front end cannot hold (`docs/design/decisions.md`,
    /// "D99"). The remedy is a mode, which the
    /// sentence names in the library's words.
    #[error(
        "{table}.{column} holds {values} `{declared_type}` value(s) this query cannot represent, and it refuses a column holding one — read them in the null mode, as NULL, or in the text mode, the column as its text, or leave the column unmaterialized, read only by a filter the library answers"
    )]
    Unrepresentable { table: String, column: String, declared_type: String, values: u64 },
    /// A filter term that reads its column's declared type, in a query under
    /// [`pgdt_codec::SchemaMode::Strings`], which resolves none: `IS [NOT]
    /// UNREPRESENTABLE`, every value being text its column holds and no NULL
    /// made of one, so there is nothing for the test to tell apart
    /// (`docs/design/decisions.md`, "D101"); and an ordering operator in
    /// PostgreSQL's semantics, which orders as the declared type does. One
    /// cause and one remedy, the typed schema mode. Raised at planning,
    /// before a row is read, and only once every term's column is in the
    /// block — a misspelled one is [`Self::UnknownPredicateColumn`].
    #[error(
        "`{column} {op}` reads the column's declared type — the values it accepts, or its order — and this query reads every column as its text, resolving no declared type; ask it of a query in the typed schema mode"
    )]
    TestNeedsDeclaredType { column: String, op: &'static str },
    /// [`Self::FieldDecode`]'s mirror, met on the way **out**: an Arrow array
    /// holds a value no PostgreSQL text form spells, so `render_field`
    /// refuses rather than writing something the file could not have held
    /// (`docs/design/decisions.md`, "D45").
    ///
    /// It names the Arrow value rather than a table, column and row offset
    /// because nothing a scan fills can reach it — every typed column a query
    /// fills comes from a `decode_*`, whose range is what its `render_*` can
    /// write back. Only an array a caller built itself carries one, and such
    /// an array has no dump position to name. Two cases reach it: `interval`,
    /// Arrow's `Interval(MonthDayNano)` counting nanoseconds where
    /// PostgreSQL's field counts microseconds, and an `int2vector` holding a
    /// NULL element, which its text form has no encoding for.
    #[error("this Arrow value has no `{declared_type}` text form: {reason}")]
    FieldRender { declared_type: &'static str, reason: String },
}

/// [`Error::NullRefused`]'s sentence: worded as [`Error::FieldRefused`]'s
/// where the line is numbered, and as [`Error::FieldDecode`]'s where it is
/// named by its offset alone.
fn null_refused(table: &str, column: &str, line: Option<u64>, line_offset: u64) -> String {
    const REFUSED: &str = "a NULL, which PostgreSQL refuses in a column declared `NOT NULL` — \
                           restoring this dump fails this table's COPY there, and so does this read";
    match line {
        Some(line) => format!(
            "COPY {table}, line {line}, column {column}: {REFUSED}; the line is at offset {line_offset}"
        ),
        None => format!("{table}.{column}: the line at offset {line_offset} holds {REFUSED}"),
    }
}

/// [`Error::MapIncomplete`]'s total: the source's size, or that its end is
/// not known yet.
fn map_total(size: Option<u64>) -> String {
    match size {
        Some(size) => format!("the source's {size} byte(s)"),
        None => "a source whose end has not been read yet".to_owned(),
    }
}

/// [`Error::PredicateValueDecode`]'s closing clause: where the rest of an
/// enum's labels are, when the accepted form stopped short of them.
fn unlisted_labels(unlisted: usize) -> String {
    match unlisted {
        0 => String::new(),
        n => format!(", and {n} more, which the type's declaration lists"),
    }
}

/// [`Error::CachedBlockChanged`]'s sentence: the cache named where there is
/// one, with the remedy on it, and the map alone where there is not.
fn cached_block_changed(path: Option<&Path>, header_offset: u64) -> String {
    match path {
        Some(path) => format!(
            "the COPY block the cache at {} records at offset {header_offset} no longer ends where that cache says, so the file changed since it was scanned — remove that cache and map the file again",
            path.display()
        ),
        None => format!(
            "the COPY block the map records at offset {header_offset} no longer ends where that map says, so the file changed since it was scanned"
        ),
    }
}

/// An `io::Error` as the library's: [`Error::Io`], unless its payload is
/// one of ours ([`Error::into_io`]), which is handed back as itself.
impl From<std::io::Error> for Error {
    fn from(cause: std::io::Error) -> Self {
        if cause.get_ref().is_some_and(|payload| payload.is::<Error>()) {
            let payload = cause.into_inner().expect("a payload was just read");
            return *payload.downcast::<Error>().expect("its type was just read");
        }
        Error::Io(cause)
    }
}

impl Error {
    /// [`Error::File`]: `cause`, met doing `doing` to the file at `path`.
    pub fn file(doing: &'static str, path: impl Into<PathBuf>, cause: std::io::Error) -> Self {
        Error::File { doing, path: path.into(), cause }
    }

    /// This error as an `io::Error`, for a read behind a trait that answers
    /// one: [`Error::Io`]'s own, or one whose payload is this error, of its
    /// cause's kind for [`Error::File`]. `From<io::Error>` hands the payload
    /// back, and through a codec's error, which displays the `io::Error`, its
    /// message is this error's — so a file named here is named whoever carries
    /// it.
    pub fn into_io(self) -> std::io::Error {
        let kind = match self {
            Error::Io(cause) => return cause,
            Error::File { ref cause, .. } => cause.kind(),
            _ => std::io::ErrorKind::Other,
        };
        std::io::Error::new(kind, self)
    }

    /// [`Error::Arrow`] carrying `cause`, an `ArrowError`.
    pub fn arrow(cause: impl std::error::Error + Send + Sync + 'static) -> Self {
        Error::Arrow(Box::new(cause))
    }

    /// [`Error::Codec`] naming `codec` and carrying `cause`, its crate's
    /// error.
    pub fn codec(
        codec: &'static str,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Error::Codec { codec, cause: Box::new(cause) }
    }

    /// [`Error::CacheEncode`] carrying `cause`, bincode's `EncodeError`.
    pub fn cache_encode(cause: impl std::error::Error + Send + Sync + 'static) -> Self {
        Error::CacheEncode(Box::new(cause))
    }

    /// [`Error::Join`] carrying `cause`, tokio's `JoinError`.
    pub fn join(cause: impl std::error::Error + Send + Sync + 'static) -> Self {
        Error::Join(Box::new(cause))
    }

    /// The cause an [`Error::Codec`] carries, as the type it is — what a
    /// caller telling one codec failure from another matches on, an
    /// `xz_seek::Error` for `.xz`.
    pub fn codec_cause<E: std::error::Error + 'static>(&self) -> Option<&E> {
        match self {
            Error::Codec { cause, .. } => cause.downcast_ref(),
            _ => None,
        }
    }
}

/// Why a cache at a path cannot be used — carried whole by `pgdt_dump::cache::CacheStatus`,
/// `pgdt_dump::cache::Held` and [`Error::CacheUnusable`], so a caller
/// that reports it and one that refuses on it name the same reason
/// (`docs/design/decisions.md`, "D22").
///
/// **Every reason but one is recognisably ours**, and those are what
/// `pgdt_dump::cache::CacheMode::with_overwrite_unusable` may replace; a file that is not a
/// pgdt cache is refused whatever is asked ([`Unusable::overwritable`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    /// The file does not open with the cache header's magic: something else is at this
    /// path. **Never overwritten.**
    NotACache,
    /// The cache header's magic, over a rest this build cannot read as a
    /// cache whole — a file cut short or damaged.
    Unreadable,
    /// Another build's cache: `found` is the format version its header
    /// states, where this build reads `pgdt_model::CACHE_FORMAT_VERSION`; nothing
    /// migrates (`docs/design/roadmap.md`, "Pre-1.0").
    UnsupportedVersion { found: u32 },
    /// A readable cache whose recorded *stored* size disagrees with the live
    /// source's, so every byte offset in it could be wrong — the staleness
    /// check reads `pgdt_io::ByteRangeSource::stored_size`, not the addressable
    /// length (`docs/design/decisions.md`, "D21"). Both stored sizes are
    /// carried: they are the evidence a refusal states.
    SourceChanged { cached_stored_size: u64, live_stored_size: u64 },
    /// A cache whose compression layer is not the live source's — one says
    /// compressed and the other not, or the two indexes differ — at the same
    /// stored size, so it was written for another file
    /// (`docs/design/decisions.md`, "D18"). Recognition reaches it first
    /// (`pgdt_io::Recognized::Mismatch`) wherever the cache's claim was handed
    /// to it; a load against the live source answers it for a source opened without one.
    CompressionContradicted,
    /// A cache whose unrepresentable counts were taken under a calendar
    /// ending on another day than this build's — `pgdt_model::calendar_end`,
    /// each a `Date32` day count — so a count of what a DataFusion query
    /// cannot display may be wrong either way (`docs/design/decisions.md`,
    /// "D96"). Never read as current and never replaced unasked: a `parse`
    /// told it may overwrite the cache counts again.
    CalendarChanged { counted_under: i32, build: i32 },
}

impl Unusable {
    /// Whether `pgdt_dump::cache::CacheMode::with_overwrite_unusable` may replace a cache
    /// found in this state: every one that is recognisably a pgdt cache, a
    /// damaged one of this build's header included (`docs/design/decisions.md`, "D20").
    pub fn overwritable(&self) -> bool {
        !matches!(self, Unusable::NotACache)
    }
}

/// A `Date32` day count as the date it names, for a refusal to state.
fn calendar_day(days: i32) -> String {
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("the Unix epoch is a date");
    epoch.checked_add_signed(chrono::TimeDelta::days(i64::from(days))).map_or_else(
        || format!("day {days} from 1970"),
        |day| {
            use chrono::Datelike;
            format!("{}-{:02}-{:02}", day.year(), day.month(), day.day())
        },
    )
}

/// What a user can do about a cache that is recognisably ours and cannot be
/// used, in the words every such refusal ends with — [`Error::CacheUnusable`],
/// which `pgdt info` prints too (`docs/design/decisions.md`, "D20").
pub const OVERWRITE_WAYS_OUT: &str =
    " — remove it, name a different cache path, or have a scan replace it";

/// What a scan refusing an unusable cache says: which way it cannot be used,
/// then what the user can do. **A cache recognisably ours names three ways
/// out, and a file that is not one two**, the flag never replacing it
/// (`pgdt_dump::cache::Unusable::overwritable`).
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
        Unusable::CalendarChanged { counted_under, build } => format!(
            "the cache at {path} counted the values a query cannot display against a calendar \
             ending {}, where this build's ends {}, so a mapping pass must count them again",
            calendar_day(*counted_under),
            calendar_day(*build),
        ),
    };
    format!("{found}{}", OVERWRITE_WAYS_OUT)
}

/// Why a restore refuses a row ending other than its block's first row does,
/// in `CopyReadLineText`'s words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowEndingRefusal {
    /// A raw CR in a block whose rows end in LF, or one not before the LF in a
    /// block whose rows end in CR LF.
    LiteralCarriageReturn,
    /// A row ending in a bare LF in a block whose rows end in CR LF.
    LiteralNewline,
}

impl std::fmt::Display for RowEndingRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::LiteralCarriageReturn => "literal carriage return found in data",
            Self::LiteralNewline => "literal newline found in data",
        })
    }
}

#[cfg(test)]
mod tests;
