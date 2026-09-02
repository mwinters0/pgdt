use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use anyhow::{Context, Result};
use arrow::array::RecordBatch;
use arrow::datatypes::DataType;
use clap::{Parser, Subcommand};
use futures::StreamExt;
use pgdump_query::cache::{CacheMode, CacheStatus};
use pgdump_query::pgtype::RANGE_STRUCT_FIELDS;
use pgdump_query::resolve::{ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
use pgdump_query::{
    ArrayShape, ByteRangeSource, CompareKind, ComparisonPlan, DataBlock, Diagnostic,
    DiagnosticKind, DumpIndex, DumpMetadata, LocalFileSource, NestedPlan, Predicate, PredicateOp,
    QueryOptions, ScanOptions, Severity, Span, SpanBody, TypeKind, preamble_only, render_field,
};

mod where_expr;

#[derive(Parser)]
#[command(
    name = "pgdq",
    about = "Query pg_dump plain-format files without loading them into memory"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// CLI spelling of [`SchemaMode`] — see "Output model" in
/// `docs/design/architecture.md`.
#[derive(Clone, Copy, Default, clap::ValueEnum)]
enum CliSchemaMode {
    #[default]
    Typed,
    Strings,
}

impl From<CliSchemaMode> for SchemaMode {
    fn from(mode: CliSchemaMode) -> Self {
        match mode {
            CliSchemaMode::Typed => SchemaMode::Typed,
            CliSchemaMode::Strings => SchemaMode::Strings,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Scan a dump file and build the structure cache — the only command
    /// that reads the dump for its structure (`pgdq info` reports from the
    /// cache this leaves). Resumes from a matching cache rather than
    /// restarting, and banks its progress at `COPY` block boundaries as it
    /// goes — including on Ctrl-C, which saves what has been scanned and
    /// exits 130 — so an interrupted scan is not wasted work. Remove the
    /// cache file to force a scan from byte 0.
    Parse {
        /// The dump file to scan.
        #[arg(long)]
        source: PathBuf,
        /// Cache file path, or `none` to disable the cache. Since `parse`'s
        /// whole purpose is to write the cache, `none` is rejected.
        #[arg(long)]
        dqcache: Option<PathBuf>,
        /// Scan only the dump's preamble — the dump-level header alone, no
        /// per-block listing or row counts — instead of the whole file. Cost
        /// is independent of dump size regardless of how much `COPY` data
        /// follows (`docs/design/architecture.md`, "Bounded preamble-only
        /// reads"). The cache this leaves is a partial one that `pgdq info`
        /// reads like any other.
        #[arg(long)]
        preamble_only: bool,
    },
    /// Report what a dump's cache holds. **`info` never scans** — it reads the
    /// cache `pgdq parse` wrote and errors if there is not one, rather than
    /// starting an hours-long scan on your behalf
    /// (`docs/design/architecture.md`, "CLI surface"). A cache from an
    /// unfinished scan is reported for as far as it got, with the coverage
    /// stated at the top.
    Info {
        /// The dump file the cache belongs to: its size is checked against
        /// the cache's, so a changed file is caught. Omit it to answer from
        /// `--dqcache` alone — cache-only mode, which then requires
        /// `--dqcache` and cannot check anything.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Cache file path, defaulting to the colocated `<source>.dqcache`.
        /// Required when `--source` is omitted — that is the cache-only entry
        /// point, and there is nothing else to answer from.
        #[arg(long, required_unless_present = "source")]
        dqcache: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
        /// List every span the map holds (`docs/design/architecture.md`,
        /// "`DumpIndex`: one owner per fact") — DDL objects
        /// and framing included, not just `COPY` blocks — instead of the
        /// per-table listing.
        #[arg(long)]
        map: bool,
        /// Print the internal index as JSON instead of the human-readable
        /// listing: the whole `DumpIndex`, its coverage, its diagnostics, and
        /// the per-`COPY`-block type resolution `--verbose` renders as text.
        /// No schema stability is promised — this is a raw dump of our
        /// internal representation, not a supported interchange format
        /// (`docs/design/architecture.md`, "CLI surface"). Incompatible with
        /// `--verbose`/`--map`, which format detail this already carries in
        /// full.
        #[arg(long)]
        json: bool,
    },
    /// Stream a table's rows, optionally projected to named columns and
    /// filtered by a boolean expression over single-column predicates.
    Query {
        /// The dump file to scan. `query` can never answer from a cache
        /// alone — row data is never cached — so this is always required.
        #[arg(long)]
        source: PathBuf,
        /// Table name, qualified (`schema.table`) or bare. Taken exactly as
        /// given, like `--column` and unlike a `--filter` term.
        #[arg(long)]
        table: String,
        /// Cache file path, or `none` to ignore any existing cache and
        /// perform a fresh scan without persisting it.
        #[arg(long)]
        dqcache: Option<PathBuf>,
        /// Single-column filter: `column=value`, `column!=value`,
        /// `column<value`, `column<=value`, `column>value`,
        /// `column>=value`, `column IS DISTINCT FROM value`,
        /// `column IS NOT DISTINCT FROM value`, `column IS NULL`, or
        /// `column IS NOT NULL`. Repeatable — every term must match, so the
        /// terms are ANDed. `OR`, negation and grouping are `--where`, which
        /// takes an expression over these same terms; giving both flags ANDs
        /// them (`docs/design/architecture.md`, "Predicates").
        ///
        /// Every operator but the two NULL tests compares **typed**: the
        /// filter's value is read with the column's own decoder, so a value
        /// that is not of that type is refused by name rather than matching
        /// nothing. The four ordering operators are additionally refused on a
        /// column whose type did not resolve or that is nested; `=`/`!=`
        /// compare such a column as text.
        ///
        /// Spaces around the operator are not data: `name = alpha` asks for
        /// `alpha`. Quote either side — `'` and `"` both work — to say
        /// otherwise: `name = " x"` keeps the leading space, and a quote
        /// inside a quoted part is doubled (`name = 'it''s'`). Quotes work on
        /// the column side too, which is how a column named `a=b` is asked
        /// for: `"a=b"=x`.
        ///
        /// A term is never read as an expression — but nor may it hold what
        /// `--where` would read as one. An unquoted `AND`, `OR`, `NOT` or
        /// paren is refused rather than taken literally, so no string means
        /// one thing here and another under `--where`; quote the part that
        /// holds it, or use `--where`.
        #[arg(long)]
        filter: Vec<String>,
        /// Boolean expression over `--filter`'s terms: `AND`, `OR`, `NOT` and
        /// parens, with `NOT` binding tighter than `AND` and `AND` tighter
        /// than `OR`. The keywords are case-insensitive and are recognised
        /// only outside quotes, so `--where 'tag=and'` is still an equality
        /// against `and`.
        ///
        /// Anything that is not a paren or a keyword is a term, read by
        /// exactly the grammar `--filter` reads — so `--where 'note=a and b'`
        /// is `note=a` AND the term `b`, which has no operator and is
        /// refused. `--filter` refuses that same string too, for holding a
        /// reserved spelling: a string both flags take means the same thing
        /// under both.
        ///
        /// A value that holds a paren or an unquoted keyword needs quoting —
        /// `--where "v='(1,a)'"` — since a bare `(` groups.
        ///
        /// Given with `--filter`, the expression and every term are ANDed.
        #[arg(long = "where", value_name = "EXPR")]
        where_expr: Option<String>,
        /// Materialize only this column, repeatable — the output carries the
        /// columns in the order the flags give them, which need not be the
        /// file's. A name the table does not carry is an error, and so is a
        /// repeated one. Omit it entirely for every column
        /// (`docs/design/architecture.md`, "Projection").
        ///
        /// A column that is not projected is never decoded, so projecting a
        /// column away is also the way past a value that fails to decode
        /// while keeping every other column typed.
        ///
        /// A `--filter` term may name a column this does not: the
        /// projection decides what is built, never what may be tested.
        ///
        /// The name is taken exactly as given — the shell has already
        /// delimited it, so there is no quoting to strip and a column whose
        /// name really does carry quote marks stays askable.
        #[arg(long = "column", value_name = "NAME")]
        column: Vec<String>,
        /// Materialize no columns at all — the `COUNT(*)` shape. Each row
        /// prints as an empty line and no header line is printed, so
        /// `--no-columns | wc -l` is a row count. Incompatible with
        /// `--column`.
        #[arg(long, conflicts_with = "column")]
        no_columns: bool,
        /// Select which database to query when `table` is ambiguous across
        /// a multi-`\connect` dump (`docs/design/architecture.md`,
        /// "One target per query").
        #[arg(long)]
        database: Option<String>,
        /// `typed` (default) resolves column types against the dump's DDL;
        /// `strings` skips that lookup entirely, matching the untyped
        /// byte-for-byte output — the way out of `Error::MetadataNotScanned`
        /// for a database an incremental scan hasn't read the DDL for yet.
        #[arg(long, value_enum, default_value_t)]
        schema_mode: CliSchemaMode,
    },
}

/// Turn the two projection flags into [`QueryOptions::projection`]. The three
/// states are distinct and none of them is spelled the same way:
/// `--no-columns` is the empty projection (`COUNT(*)`), one or more
/// `--column` is that list in that order, and neither flag is `None` — every
/// column (`docs/design/architecture.md`, "Projection").
///
/// The two flags cannot both be given: clap's `conflicts_with` refuses that
/// before this is reached, so `--no-columns` wins here only in a case that
/// cannot occur. Nothing rejects a repeated `--column` name at this layer —
/// the library refuses it as `Error::DuplicateProjectionColumn` before a byte
/// of the file is read, which is the same answer with the same wording
/// whether the caller is the CLI or an embedder.
fn projection(columns: Vec<String>, no_columns: bool) -> Option<Vec<String>> {
    if no_columns {
        Some(Vec::new())
    } else if columns.is_empty() {
        None
    } else {
        Some(columns)
    }
}

/// The comparison spellings, in the order they are tried **at one position**
/// — longest first, so `>=` is never read as `>` followed by a stray `=`,
/// the way `!=` has always been checked before `=`.
const FILTER_OPS: [(&str, PredicateOp); 6] = [
    ("!=", PredicateOp::Ne),
    (">=", PredicateOp::Ge),
    ("<=", PredicateOp::Le),
    ("=", PredicateOp::Eq),
    (">", PredicateOp::Gt),
    ("<", PredicateOp::Lt),
];

/// `word` at `i`, case-insensitively, and where it ends.
fn word_at(bytes: &[u8], i: usize, word: &str) -> Option<usize> {
    let end = i + word.len();
    (bytes.len() >= end && bytes[i..end].eq_ignore_ascii_case(word.as_bytes())).then_some(end)
}

/// Past the run of ASCII whitespace starting at `i` — `None` where there is
/// none, since every gap in the worded operators below must be a real one.
fn skip_spaces(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    (j > i).then_some(j)
}

/// `IS DISTINCT FROM` / `IS NOT DISTINCT FROM` starting at `i`, and how many
/// bytes it runs for — the two worded infix operators, offered to the same
/// positional scan the punctuation spellings go through so that **the
/// earliest operator still wins**. `note=a is distinct from b` is therefore
/// the equality it was before this existed, and `a is distinct from b=c` is
/// the distinctness test, exactly as `name=a>b` and `a>b=c` already split.
///
/// **Whitespace is required on both sides of the phrase**, which is what
/// keeps the addition from re-reading any term that parsed before: a column
/// named `is distinct from` is still askable as `is distinct from=x`, since
/// the phrase there is followed by `=` rather than by a space. What does
/// change meaning is a term whose *column* is spelled with the phrase in it
/// surrounded by spaces — `a is distinct from b=c` — and that is loud, not
/// silent: the column it now names is `a`.
///
/// Any run of whitespace separates the words, as in SQL, and the case is
/// free.
fn distinct_from_at(bytes: &[u8], i: usize) -> Option<(usize, PredicateOp)> {
    if i == 0 || !bytes[i - 1].is_ascii_whitespace() {
        return None;
    }
    let mut j = skip_spaces(bytes, word_at(bytes, i, "is")?)?;
    let op = match word_at(bytes, j, "not") {
        Some(after) => {
            j = skip_spaces(bytes, after)?;
            PredicateOp::IsNotDistinctFrom
        }
        None => PredicateOp::IsDistinctFrom,
    };
    j = skip_spaces(bytes, word_at(bytes, j, "distinct")?)?;
    let end = word_at(bytes, j, "from")?;
    // A value has to follow, and be separated from `FROM`: without this,
    // `v is distinct from` alone would split into an empty value rather than
    // falling through to the usage message it deserves.
    if !bytes.get(end).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    Some((end - i, op))
}

/// Split `spec` at its operator.
///
/// **The earliest position wins, and the longest spelling at that position.**
/// Scanning by position rather than by operator is what keeps a value that
/// contains an operator byte from stealing the split — `name=a>b` is `name`
/// equal to `a>b`, not `name=a` greater than `b`.
///
/// **The scan skips quoted regions**, so a column named `a=b` is askable as
/// `"a=b"=x`. A quote that never closes is its own outcome rather than "no
/// operator": the operator it swallowed is real, and reinterpreting the term
/// without it is the silent-wrong-answer shape this grammar exists to remove.
///
/// The two worded operators ([`distinct_from_at`]) are candidates at the same
/// positions, so they obey the same earliest-wins rule rather than being a
/// pass of their own — a pass would make `note=a is distinct from b` a
/// distinctness test on a column called `note=a`.
fn split_filter_op(spec: &str) -> FilterSplit<'_> {
    let bytes = spec.as_bytes();
    // The scan walks *bytes*, and compares bytes: every character it looks
    // for is ASCII and no byte of a multi-byte UTF-8 character is, so a match
    // is always at a character boundary and the `spec[..i]` slices below are
    // safe. Matching an operator through `str` instead would panic on the
    // interior byte of a multi-byte character — which is not hypothetical,
    // since trimming is Unicode's and a non-breaking space is what brings one
    // into a term.
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            // A doubled quote is an escaped one and keeps the region open.
            Some(q) if b == q => {
                if bytes.get(i + 1) == Some(&q) {
                    i += 2;
                } else {
                    quote = None;
                    i += 1;
                }
            }
            Some(_) => i += 1,
            None if b == b'\'' || b == b'"' => {
                quote = Some(b);
                i += 1;
            }
            None => {
                if let Some((symbol, op)) = FILTER_OPS
                    .into_iter()
                    .find(|(symbol, _)| bytes[i..].starts_with(symbol.as_bytes()))
                {
                    return FilterSplit::Op(&spec[..i], op, &spec[i + symbol.len()..]);
                }
                if let Some((len, op)) = distinct_from_at(bytes, i) {
                    return FilterSplit::Op(&spec[..i], op, &spec[i + len..]);
                }
                i += 1;
            }
        }
    }
    match quote {
        // Whatever opened it sits left of any operator, since the scan
        // returns at the first operator it reaches outside a quote.
        Some(q) => FilterSplit::UnbalancedQuote(q as char),
        None => FilterSplit::NoOperator,
    }
}

/// What [`split_filter_op`] found. `NoOperator` is the `IS NULL` forms' cue,
/// not a fault: they are the fallback, tried only on a term with no operator
/// outside quotes.
enum FilterSplit<'a> {
    Op(&'a str, PredicateOp, &'a str),
    NoOperator,
    UnbalancedQuote(char),
}

/// The text a quoted part holds: the outer pair stripped and every doubled
/// interior quote collapsed to one, SQL's own escape.
///
/// `None` — the part does not open with a quote, so it is data exactly as
/// written. `Some(Err(quote))` — it opens with one and what follows is not a
/// well-formed quoted string. An unterminated quote and text after the
/// closing one are deliberately the *same* fault: the alternative is falling
/// back to the unquoted reading, which hands a user who mistyped one quote a
/// value nobody meant and an empty result that reads as an answer.
fn dequote(part: &str) -> Option<Result<String, char>> {
    let quote = part.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut out = String::new();
    let mut rest = &part[quote.len_utf8()..];
    loop {
        let Some(at) = rest.find(quote) else { return Some(Err(quote)) };
        out.push_str(&rest[..at]);
        rest = &rest[at + quote.len_utf8()..];
        if let Some(after) = rest.strip_prefix(quote) {
            out.push(quote);
            rest = after;
        } else if rest.is_empty() {
            return Some(Ok(out));
        } else {
            return Some(Err(quote));
        }
    }
}

/// One side of a filter term as the [`Predicate`] should carry it: whitespace
/// outside the quotes trimmed off, and a quoted part taken exactly as
/// written. `what` names the side for the error message and nothing else.
///
/// Trimming is `str::trim`, the same definition the `IS NULL` forms use, so
/// the parser holds one notion of whitespace and a non-breaking space pasted
/// out of a web page is caught by it.
fn filter_part(part: &str, what: &str, spec: &str) -> Result<String> {
    let part = part.trim();
    match dequote(part) {
        None => Ok(part.to_string()),
        Some(Ok(text)) => Ok(text),
        Some(Err(quote)) => Err(unbalanced_quote(what, quote, spec)),
    }
}

/// The one message every malformed quote earns, wherever it was found.
fn unbalanced_quote(what: &str, quote: char, spec: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "--filter `{spec}`: unbalanced `{quote}` quote in the {what} — a quoted {what} closes with the matching `{quote}` at its very end, and any `{quote}` inside it is doubled"
    )
}

/// One `--filter` argument: the term grammar below, and before it the refusal
/// that keeps a string from meaning one thing under each flag
/// ([`where_expr::refuse_where_structure`], which is where that reasoning
/// lives).
///
/// It runs first, so a term that is both structural and malformed earns the
/// structural message: `--filter 'and is null'` is told that `AND` is a
/// reserved spelling rather than that its column was not understood.
fn parse_filter_flag(spec: &str) -> Result<Predicate> {
    where_expr::refuse_where_structure(spec)?;
    parse_filter(spec)
}

/// Parse one filter term into a [`Predicate`] — one term of the conjunction
/// a repeated `--filter` builds, and equally the **leaf** of a `--where`
/// expression ([`where_expr`]), which is one grammar rather than two:
/// `column<op>value` for any of the six comparison spellings,
/// `column IS [NOT] DISTINCT FROM value`, or `column IS NULL` /
/// `column IS NOT NULL` (the worded forms matched case-insensitively — see
/// `docs/design/architecture.md`, "A filter term is parsed for two
/// audiences").
///
/// **The `IS` forms are the fallback, not the first test.** An operator
/// outside quotes is looked for first, and the suffix is only stripped from a
/// term that has none. Testing the suffix first made `note=this is null` an
/// `IS NULL` on a column called `note=this`; under this order it is an
/// equality against `this is null`, which is what it says.
fn parse_filter(spec: &str) -> Result<Predicate> {
    match split_filter_op(spec) {
        FilterSplit::Op(column, op, value) => Ok(Predicate {
            column: filter_part(column, "column name", spec)?,
            op,
            value: Some(filter_part(value, "value", spec)?),
        }),
        FilterSplit::UnbalancedQuote(quote) => Err(unbalanced_quote("column name", quote, spec)),
        FilterSplit::NoOperator => {
            let trimmed = spec.trim();
            for (suffix, op) in
                [("is not null", PredicateOp::IsNotNull), ("is null", PredicateOp::IsNull)]
            {
                if let Some(column) = strip_ci_suffix(trimmed, suffix) {
                    return Ok(Predicate {
                        column: filter_part(column, "column name", spec)?,
                        op,
                        value: None,
                    });
                }
            }
            anyhow::bail!(
                "--filter must be `column=value` (or `!=`, `<`, `<=`, `>`, `>=`), `column IS DISTINCT FROM value`, `column IS NOT DISTINCT FROM value`, `column IS NULL`, or `column IS NOT NULL`, got `{spec}`"
            )
        }
    }
}

/// The sentence a name that was not found earns when it opens and closes with
/// a matching quote: `--column` and `--table` take their names exactly as
/// given, so the quote marks were part of what was looked for.
///
/// **Only a `--filter` term has quoting to strip**, and that is not an
/// inconsistency: a term is one string that must be split into three parts,
/// so quotes carry boundary information there, while the shell has already
/// delimited a `--column` argument. Stripping them here would instead make a
/// column genuinely named with quote marks unaskable
/// (`docs/design/architecture.md`, "A filter term is parsed for two
/// audiences").
fn quoted_name_note(flag: &str, name: &str) -> Option<String> {
    let quote = name.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    (name.len() > quote.len_utf8() && name.ends_with(quote)).then(|| {
        format!(
            "`{flag}` takes the name exactly as given, so `{name}` was matched literally, quote marks included — only a `--filter` term has quoting to strip"
        )
    })
}

/// Add [`quoted_name_note`] to the one library refusal that can carry it. The
/// failure is loud either way; what the note adds is *why* a name the user is
/// sure exists was not found.
fn name_taken_verbatim(err: pgdump_query::Error) -> anyhow::Error {
    if let pgdump_query::Error::UnknownProjectionColumn { column, .. } = &err
        && let Some(note) = quoted_name_note("--column", column)
    {
        return anyhow::anyhow!("{err}; {note}");
    }
    err.into()
}

/// Say, once per query and on stderr, which of this query's comparisons do
/// not answer what PostgreSQL's own operator would
/// (`docs/design/architecture.md`, "Predicates", the comparison register).
///
/// It is per *term*, not per column: most divergences are divergences of
/// order alone, so a `text` column filtered with both `<` and `=` warns about
/// the first and not the second.
///
/// **Announced by the CLI rather than carried by a library channel.** The
/// signal is per-column *and* conditional on a predicate — L4 — while
/// `DumpIndex.diagnostics` is L1 and `ResolvedSchema.notes` is L2, so writing
/// it into either would invert the layering. An embedder reads
/// `TableStream::comparison_notes` for the same facts; what it *should* be
/// handed is filed in `docs/design/roadmap-P6-embeddable-engine-inbox.md`.
fn announce_comparisons(stream: &pgdump_query::TableStream<'_>) {
    for note in stream.comparison_notes() {
        eprintln!("warning: {}", note.message());
    }
}

/// Case-insensitive suffix strip, for matching `IS NULL`/`IS NOT NULL` at
/// the end of a `--filter` argument regardless of how the user cased it.
fn strip_ci_suffix<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let split = s.len().checked_sub(suffix.len())?;
    let (head, tail) = s.split_at(split);
    tail.eq_ignore_ascii_case(suffix).then_some(head)
}

/// The one line `pgdq parse` prints about *this invocation* rather than about
/// the file: where the scan picked up. `None` for a scan that started at byte
/// 0, which is the case that needs no explanation.
///
/// A run that found the cache already complete scanned nothing at all, and
/// says so rather than reporting a resume point equal to the file's size —
/// the two are different facts to a user checking whether an interrupted scan
/// finished.
fn resume_notice(resumed_from: u64, size: u64) -> Option<String> {
    match resumed_from {
        0 => None,
        n if n >= size => {
            Some(format!("nothing to scan: the cache already covers all {size} byte(s)"))
        }
        n => Some(format!("resumed a previous scan at byte {n} of {size}")),
    }
}

/// Catch `SIGINT` and `SIGTERM` for the duration of a scan, so an interrupted
/// `pgdq parse` saves what it has instead of throwing it away
/// (`docs/design/architecture.md`, "`parse` resumes, and saves as it goes").
///
/// The guard is **cooperative**: the signal sets a flag the mapping loop reads
/// once per chunk, and the loop persists the index it owns before returning.
/// *Rejected:* `tokio::select!` in the CLI over `ctrl_c` and the scan future.
/// It reads as the obvious form and it is the one that silently discards the
/// work — `map_file` owns the `DumpIndex` for the whole scan, so cancelling
/// that future drops the map rather than saving it.
///
/// A **second** signal, of either kind, exits immediately: a save that wedges
/// must not be able to hold the process, and a Ctrl-C that appears to do
/// nothing is worse than no handler at all.
///
/// Returns the cell the exit code is read from: `0` until a signal lands,
/// then that signal's number.
fn install_interrupt_guard(cancel: Arc<AtomicBool>) -> Result<Arc<AtomicI32>> {
    use tokio::signal::unix::{SignalKind, signal};

    let signalled = Arc::new(AtomicI32::new(0));
    for kind in [SignalKind::interrupt(), SignalKind::terminate()] {
        let number = kind.as_raw_value();
        let mut stream =
            signal(kind).with_context(|| format!("installing handler for {number}"))?;
        let (cancel, signalled) = (Arc::clone(&cancel), Arc::clone(&signalled));
        tokio::spawn(async move {
            while stream.recv().await.is_some() {
                // A non-zero previous value means the other handler, or this
                // one, has already asked the scan to stop.
                let already = signalled.swap(number, Ordering::SeqCst);
                cancel.store(true, Ordering::SeqCst);
                if already != 0 {
                    std::process::exit(128 + number);
                }
            }
        });
    }
    Ok(signalled)
}

/// Print one batch's rows tab-separated, `\N` for NULL — mirroring COPY
/// TEXT's own NULL marker. Each field is rendered back to PostgreSQL text via
/// [`render_field`], so output is byte-identical whether `--schema-mode` is
/// `typed` or `strings` (`docs/design/architecture.md`,
/// "CLI surface").
///
/// `plans` is the stream's own [`pgdump_query::ResolvedSchema::plans`], which
/// is what says whether a `List<Struct{…}>` column is written as an array of
/// ranges or as a multirange. A column with no entry falls back to
/// `NestedPlan::Scalar`, which is right for every non-nested type.
///
/// The `Result` is `render_field`'s refusal of a value with no PostgreSQL
/// text form, which **no batch this binary prints can hold**: every typed
/// column here is filled by a decoder whose range its renderer can write back.
/// It is propagated rather than unwrapped because an unreachable panic in the
/// output path is a worse answer than an error message.
fn print_batch(batch: &RecordBatch, plans: &[NestedPlan]) -> Result<()> {
    for row in 0..batch.num_rows() {
        let fields: Vec<String> = batch
            .columns()
            .iter()
            .enumerate()
            .map(|(col, c)| {
                let plan = plans.get(col).unwrap_or(&NestedPlan::Scalar);
                Ok(render_field(c.as_ref(), row, plan)?.unwrap_or_else(|| "\\N".to_string()))
            })
            .collect::<Result<_>>()?;
        println!("{}", fields.join("\t"));
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Parse { source: file, dqcache, preamble_only: preamble_only_flag } => {
            // `parse` is the only scanner (`docs/design/architecture.md`,
            // "CLI surface"). Reject `--dqcache none` up front, before paying
            // for a scan we won't be allowed to persist.
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let path = mode
                .require_enabled("parse")
                .context("`--dqcache none` cannot be combined with `parse`")?
                .to_path_buf();
            let source = LocalFileSource::open(&file)?;
            if preamble_only_flag {
                let (metadata, diagnostics) =
                    preamble_only(&source, &ScanOptions::default(), &mode).await?;
                print_metadata(&metadata, false);
                print_diagnostics(&diagnostics);
                println!();
                println!("wrote cache to {}", path.display());
                return Ok(());
            }
            let size = source.size().await?;
            let cancel = Arc::new(AtomicBool::new(false));
            let signalled = install_interrupt_guard(Arc::clone(&cancel))?;
            let scan_options = ScanOptions { cancel: Some(cancel), ..ScanOptions::default() };
            let run = pgdump_query::map_file(&source, &scan_options, &mode).await?;
            if run.interrupted {
                // No listing: the user asked the scan to stop, not for a
                // report on what it had reached, and `pgdq info` is the
                // command that reports. Both lines go to stderr, so a caller
                // redirecting stdout gets an empty report rather than a
                // truncated one.
                eprintln!(
                    "interrupted at byte {} of {size} — the cache at {} holds the scan so far",
                    run.index.scanned_through,
                    path.display()
                );
                eprintln!("re-run `pgdq parse --source {}` to continue", file.display());
                // Exit by signal (130/143), so a script can tell an interrupt
                // from a failure. `SIGINT` is the fallback for a flag nothing
                // in this binary sets any other way.
                let number = signalled.load(Ordering::SeqCst);
                std::process::exit(128 + if number == 0 { 2 } else { number });
            }
            // The listing describes the file's state after this run, not this
            // invocation's diff — so the one line that *is* about the
            // invocation goes above it, where a user checking on an
            // interrupted scan looks first.
            if let Some(notice) = resume_notice(run.resumed_from, size) {
                println!("{notice}");
                println!();
            }
            // `map_file` reached EOF, so its censuses cover the whole file.
            print_index(&run.index, false, false, true);
            println!();
            println!("wrote cache to {}", path.display());
        }
        Command::Info { source: file, dqcache, verbose, map, json } => {
            if json && (verbose || map) {
                anyhow::bail!(
                    "--json already carries everything --verbose/--map would add — drop one of them"
                );
            }
            let Some(file) = file else {
                // Cache-only mode (`docs/design/architecture.md`,
                // "The cache"): no live dump file at all, so
                // clap already required `--dqcache` for us.
                let path = dqcache.expect("clap requires --dqcache when --source is omitted");
                return info_offline(&path, verbose, map, json).await;
            };
            // `info` never scans, so `--dqcache none` — "ignore the cache" —
            // would leave nothing at all to answer from. The message names the
            // way out, the way `Error::FieldDecode` names `--schema-mode
            // strings`: someone reaching for `none` is usually reaching for it
            // because the dump's own directory is read-only, and what they
            // want is a cache written somewhere else.
            //
            // Formed here rather than in `Error::CacheDisabled` because it
            // interpolates the user's own `--source` path, which the library
            // error does not have and should not take a `PathBuf` to get.
            // `FieldDecode` names a *static* flag string, which is why that
            // one could live in the error.
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let path = mode
                .require_enabled("info")
                .with_context(|| {
                    format!(
                        "`--dqcache none` cannot be combined with `info`, which never scans — run \
                         `pgdq parse --source {} --dqcache <path>` to build a cache somewhere \
                         writable, then pass that same `--dqcache <path>` here",
                        file.display()
                    )
                })?
                .to_path_buf();
            let source = LocalFileSource::open(&file)?;
            let status = pgdump_query::cache::load(&path, &source).await?;
            let (mut index, mtime_changed, total_size) = match status {
                CacheStatus::Valid { index, mtime_changed, total_size }
                | CacheStatus::Incomplete { index, mtime_changed, total_size } => {
                    (index, mtime_changed, total_size)
                }
                unusable => anyhow::bail!(unusable_cache_message(&unusable, &path, Some(&file))),
            };
            if mtime_changed {
                index.diagnostics.push(Diagnostic::cache_mtime_changed());
            }
            report(&index, total_size, verbose, map, json);
        }
        Command::Query {
            source: file,
            table,
            dqcache,
            filter,
            where_expr,
            column,
            no_columns,
            database,
            schema_mode,
        } => {
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            // Every term is parsed before the file is opened, so a
            // malformed one is reported without a scan; the library then
            // resolves each against the block's own schema.
            let terms = filter
                .iter()
                .map(String::as_str)
                .map(parse_filter_flag)
                .collect::<Result<Vec<_>>>()?;
            let filter = match where_expr {
                // Byte for byte the tree a repeated `--filter` always built,
                // including the empty conjunction that keeps every row.
                None => pgdump_query::Expr::all(terms),
                Some(spec) if terms.is_empty() => where_expr::parse_where(&spec)?,
                // Both flags: one conjunction of the expression and the
                // terms, flattened rather than nested, since nothing in the
                // library prefers either shape.
                Some(spec) => pgdump_query::Expr::And(
                    std::iter::once(where_expr::parse_where(&spec)?)
                        .chain(terms.into_iter().map(pgdump_query::Expr::Term))
                        .collect(),
                ),
            };
            let source = LocalFileSource::open(&file)?;
            let mut header_printed = false;
            let mut any_batch = false;
            let mut rows = 0u64;
            let query_options = QueryOptions {
                database,
                schema_mode: schema_mode.into(),
                filter,
                projection: projection(column, no_columns),
                ..QueryOptions::default()
            };
            // Pull mode, not `read_table`: rendering a nested column back to
            // its literal needs the stream's `NestedPlan`s, and push mode
            // only hands the resolved schema back once the whole stream has
            // been drained (`docs/design/architecture.md`, "Arrow assembly
            // and the zero-copy path"). The scan itself is the same one —
            // `read_table` drains this stream internally.
            let mut stream = pgdump_query::table_stream(
                &source,
                &table,
                ScanOptions::default(),
                query_options,
                None,
                mode,
            );
            let mut announced = false;
            while let Some(batch) = stream.next().await.transpose().map_err(name_taken_verbatim)? {
                if !announced {
                    announce_comparisons(&stream);
                    announced = true;
                }
                any_batch = true;
                // A zero-column projection prints no header. The header would
                // be an empty line, and the row count `--no-columns | wc -l`
                // is asked for would come back one too many
                // (`docs/design/architecture.md`, "Projection").
                if !header_printed && batch.num_columns() > 0 {
                    let names: Vec<String> =
                        batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                    println!("{}", names.join("\t"));
                    header_printed = true;
                }
                // Re-read per batch: a table's blocks each carry their own
                // schema (a header-less block names its columns from its
                // first row), so the plans belong to the block the batch came
                // from, not to the query.
                print_batch(&batch, &stream.resolved_schema().plans)?;
                rows += batch.num_rows() as u64;
            }
            // A query that matched a block but selected no rows still
            // resolved a schema, so the announcement is owed either way; it
            // is made at the first batch when there is one so it precedes the
            // rows rather than trailing them.
            if !announced {
                announce_comparisons(&stream);
            }
            if any_batch {
                eprintln!("{rows} row(s)");
            } else {
                eprintln!("no rows found for {table} in {}", file.display());
                if let Some(note) = quoted_name_note("--table", &table) {
                    eprintln!("note: {note}");
                }
            }
        }
    }
    Ok(())
}

/// The sentence `pgdq info` prints for a cache it cannot use. All four causes
/// end in `pgdq parse`, and they are still four different sentences: the
/// remedy is the same, the fact the user needs to know is not — "you have
/// never parsed this file" and "your file changed since you parsed it" send a
/// reader to different places.
///
/// **One match, two renderings**, the same discipline [`resolution_words`]
/// applies. `source` is `None` in cache-only mode, which has no dump file to
/// name and so states the fault and stops; the two paths otherwise describe
/// the same faults, and a second match is how they come to describe them
/// differently. Cache-only mode cannot reach
/// [`CacheStatus::SourceChanged`] at all — there is no live file to compare
/// against, which is exactly what its `CacheOffline` diagnostic warns about.
///
/// Takes the whole [`CacheStatus`] rather than a narrowed type so the match
/// stays exhaustive: a usable status reaching here is a caller bug, and it says
/// so rather than printing a plausible error.
fn unusable_cache_message(status: &CacheStatus, path: &Path, source: Option<&Path>) -> String {
    // Each arm supplies its own connective and tail, because "no cache at X"
    // and "X is not a pgdq cache" do not join to the same sentence.
    let remedy = |lead: &str, tail: &str| match source {
        Some(s) => format!(" — {lead}run `pgdq parse --source {}`{tail}", s.display()),
        None => String::new(),
    };
    match status {
        CacheStatus::Missing => {
            format!("no cache at {}{}", path.display(), remedy("", " first"))
        }
        CacheStatus::Unreadable => {
            format!("{} is not a pgdq cache{}", path.display(), remedy("check the path, or ", ""))
        }
        CacheStatus::UnsupportedVersion => format!(
            "the cache at {} was written by a different pgdq build and cannot be read{}",
            path.display(),
            remedy("", "")
        ),
        CacheStatus::SourceChanged { cached_size, live_size } => {
            let source = source.expect("cache-only mode has no live source to compare against");
            format!(
                "{} has changed since it was parsed ({live_size} bytes now, {cached_size} when \
                 the cache at {} was written), so every offset in the cache could be wrong{}",
                source.display(),
                path.display(),
                remedy("", "")
            )
        }
        CacheStatus::Valid { .. } | CacheStatus::Incomplete { .. } => {
            unreachable!("a usable cache is reported, not refused")
        }
    }
}

/// `pgdq info` with no `--source`: answer strictly from the cache at `path`
/// (`docs/design/architecture.md`, "The cache"). An `Incomplete` cache is
/// reported like any other, with its coverage stated — cache-only mode has no
/// scan to extend it with, but "as far as the scan got" is still an answer,
/// and refusing it was what this phase removed.
async fn info_offline(path: &Path, verbose: bool, map: bool, json: bool) -> Result<()> {
    let mode = CacheMode::Offline(path.to_path_buf());
    let (index, total_size) = match mode.load_offline().await? {
        CacheStatus::Valid { index, total_size, .. }
        | CacheStatus::Incomplete { index, total_size, .. } => (index, total_size),
        unusable => anyhow::bail!(unusable_cache_message(&unusable, path, None)),
    };
    report(&index, total_size, verbose, map, json);
    Ok(())
}

/// One column's resolution outcome, in both spellings: a stable token for
/// `--json` and the sentence `info --verbose` prints
/// (`docs/design/architecture.md`, "CLI surface").
///
/// **One match, two renderings.** Splitting them into two functions is how the
/// machine-readable export and the text listing drift into describing
/// different vocabularies; a single exhaustive match makes a new
/// [`ColumnResolution`] variant a compile error that has to answer both.
fn resolution_words(r: &ColumnResolution) -> (&'static str, &'static str) {
    match r {
        ColumnResolution::Mapped => ("mapped", "mapped"),
        ColumnResolution::UnknownType => {
            ("unknown_type", "unknown type — no mapping for this build")
        }
        ColumnResolution::NotDeclared => {
            ("not_declared", "not declared — no DDL explained this column")
        }
        ColumnResolution::MetadataNotScanned => (
            "metadata_not_scanned",
            "metadata not scanned — the scan never reached this database's DDL; finish the parse",
        ),
        ColumnResolution::OpaqueElementType => (
            "opaque_element_type",
            "opaque element type — the array's element type is information-free in the dump",
        ),
        ColumnResolution::NestedArrayElement => (
            "nested_array_element",
            "nested array element — the array's element type is itself an array",
        ),
        ColumnResolution::VaryingArrayShape => (
            "varying_array_shape",
            "varying array shape — dimensionality differs between rows, or a value carries an explicit lower bound",
        ),
        ColumnResolution::OpaqueBaseType => {
            ("opaque_base_type", "opaque base type — information-free in the dump")
        }
        ColumnResolution::EmptyEnum => ("empty_enum", "empty enum"),
    }
}

/// The sentence half of [`resolution_words`] — `info --verbose`'s per-column
/// line.
fn resolution_label(r: &ColumnResolution) -> &'static str {
    resolution_words(r).1
}

/// What one column became in Arrow — the other half of `info --verbose`'s
/// per-column line (`docs/design/architecture.md`, "CLI surface").
///
/// Arrow's own `Display` is terse and reversible (`List(Utf8View)`,
/// `Struct("x": Int32, "y": Utf8View)`), and a composite's field names are the
/// user's own, so it carries real information and is what prints — with one
/// substitution. The five-field range struct is identical for every range
/// column in every dump and renders as 137 characters saying so, so it
/// collapses to `Range<T>`, `T` being the bound type: the only part that
/// varies. The manual states the struct's real layout once, which is what
/// makes the elision lossless.
///
/// **The substitution is detected from the [`NestedPlan`], never from the
/// field names** — a user composite is free to declare five fields with
/// exactly those names, and `pgtype::RANGE_STRUCT_FIELDS` reserves dispatch to
/// the plan. A built-in multirange and an array of the matching range render
/// *identically* (`List(Range<Int32>)`), which is correct rather than a
/// collision to fix: they are the same Arrow type, the plans differ, and the
/// declared PostgreSQL type sits on the same line.
///
/// The type and the plan come from one producer and cannot disagree; this
/// being display code, a disagreeing pair falls back to plain `Display`
/// rather than panicking the way the builder does.
fn arrow_type_label(data_type: &DataType, plan: &NestedPlan) -> String {
    match (plan, data_type) {
        (NestedPlan::Array(element), DataType::List(field)) => {
            format!("List({})", arrow_type_label(field.data_type(), element))
        }
        (NestedPlan::Record(field_plans), DataType::Struct(fields))
            if field_plans.len() == fields.len() =>
        {
            let rendered: Vec<String> = fields
                .iter()
                .zip(field_plans)
                .map(|(f, p)| format!("{:?}: {}", f.name(), arrow_type_label(f.data_type(), p)))
                .collect();
            format!("Struct({})", rendered.join(", "))
        }
        (NestedPlan::Range(bound), _) => range_label(data_type, bound),
        (NestedPlan::Multirange(bound), DataType::List(field)) => {
            format!("List({})", range_label(field.data_type(), bound))
        }
        _ => data_type.to_string(),
    }
}

/// The `Range<T>` substitution itself, shared by the `Range` and `Multirange`
/// plans — the latter is a `List` of exactly this struct.
fn range_label(data_type: &DataType, bound: &NestedPlan) -> String {
    match data_type {
        DataType::Struct(fields) if fields.len() == RANGE_STRUCT_FIELDS.len() => {
            format!("Range<{}>", arrow_type_label(fields[0].data_type(), bound))
        }
        _ => data_type.to_string(),
    }
}

/// An enum column's declared labels, in declaration order, or `None` for
/// every other column — read off the column's own [`ComparisonPlan`], which is
/// where resolution already put them (`docs/design/architecture.md`,
/// "CLI surface").
///
/// A domain over an enum answers here too, because
/// `pgtype::comparison_user_type` recurses through the domain chain; that is
/// the right answer, since such a column takes exactly those labels. An
/// *empty* enum is `ComparisonPlan::Refused` and so has nothing to list, which
/// matches the `empty enum` sentence the line above it already prints.
fn enum_labels(plan: &ComparisonPlan) -> Option<&[String]> {
    match plan {
        ComparisonPlan::Compared { kind: CompareKind::Enum(labels), .. } => Some(labels),
        _ => None,
    }
}

/// The labels as `info --verbose` prints them: each one single-quoted with any
/// interior quote doubled, comma-separated.
///
/// **Quoting is forced by the data, and this quoting by two precedents that
/// agree.** A label is arbitrary text — `has space`, `has,comma`,
/// `has'quote` are all legal and all in the fixtures — so a bare comma-joined
/// list cannot be read back apart. Single quotes with `''` doubling is both
/// what the dump's own `CREATE TYPE … AS ENUM (…)` writes and what a
/// `--filter` value accepts ([`dequote`]), so a printed label pastes straight
/// into `--filter "mood=<label>"` and reads the same as the file it came from.
///
/// *Rejected:* Rust's `{:?}`, which `arrow_type_label` uses for a composite's
/// field names. It is unambiguous too, but it spells a PostgreSQL literal in
/// Rust's escape vocabulary, and the double quote it produces is the one this
/// project's filter grammar treats as the *other* quote.
fn label_list(labels: &[String]) -> String {
    labels.iter().map(|l| format!("'{}'", l.replace('\'', "''"))).collect::<Vec<_>>().join(", ")
}

/// How a database is named in the listing. A `\connect`-less dump has no
/// name to print, and `(unnamed)` is what the listing calls that database —
/// one spelling, so the metadata header, the block listing and `--map` cannot
/// come to disagree about what an unnamed database is called.
fn database_label(database: &Option<String>) -> &str {
    match database {
        Some(name) => name,
        None => "(unnamed)",
    }
}

/// Prints a `database: <name>` line each time the database changes, and only
/// when a listing spans more than one — the common case (a plain or
/// single-`--create` dump) prints no header at all.
///
/// **The rows are already in file order and every `\connect` segment is
/// contiguous in the file**, so a header whenever the value changes is the
/// whole grouping rule: nothing has to be sorted or bucketed first. This is
/// also what makes an `AmbiguousTable` error's candidate names actionable —
/// they are names this listing already showed
/// (`docs/design/architecture.md`, "One target per query").
struct DatabaseHeadings<'a> {
    multi: bool,
    current: Option<&'a Option<String>>,
}

impl<'a> DatabaseHeadings<'a> {
    /// `databases` is every row's database, in listing order; only its
    /// cardinality is read here.
    fn new(databases: impl Iterator<Item = &'a Option<String>>) -> Self {
        let multi = databases.collect::<BTreeSet<_>>().len() > 1;
        Self { multi, current: None }
    }

    fn before(&mut self, database: &'a Option<String>) {
        if self.multi && self.current != Some(database) {
            self.current = Some(database);
            println!("database: {}", database_label(database));
        }
    }
}

/// Dump-level metadata header: server/`pg_dump` versions, extension and
/// user-defined-type counts (`docs/design/architecture.md`,
/// "CLI surface"). The `database: <name>` line is only shown when it's informative —
/// a single unnamed database (a plain, non-`--create` dump: the overwhelming
/// common case) is printed with no header line, since one would just be
/// noise.
///
/// Under `verbose`, the `user-defined types` count becomes the heading of a
/// listing of the types themselves, one line each, in the order the dump
/// declares them. The count is otherwise their only trace: nothing else in
/// `info` names a user-defined type, so a user cannot learn from it that
/// `public.mood` exists, let alone what it holds.
fn print_metadata(metadata: &DumpMetadata, verbose: bool) {
    let multi = metadata.databases.len() > 1;
    for db in &metadata.databases {
        let show_name = multi || db.name.is_some();
        let indent = if show_name { "  " } else { "" };
        if show_name {
            println!("database: {}", database_label(&db.name));
        }
        if let Some(v) = &db.server_version {
            println!("{indent}server version: {v}");
        }
        if let Some(v) = &db.pg_dump_version {
            println!("{indent}pg_dump version: {v}");
        }
        println!("{indent}extensions: {}", db.extensions.len());
        println!("{indent}user-defined types: {}", db.types.len());
        if verbose {
            // The name column is padded to the widest name this database
            // declares, so the kinds line up; the right edge stays ragged,
            // an enum's label list being as long as the type is.
            let width = db.types.iter().map(|t| t.name.chars().count()).max().unwrap_or(0);
            for def in &db.types {
                println!(
                    "{indent}    {:width$}  {}",
                    def.name,
                    type_kind_summary(&def.kind),
                    width = width
                );
            }
        }
    }
}

/// One user-defined type's kind, rendered with whatever payload that kind
/// carries — the enum's labels, the domain's base type and `COLLATE` clause,
/// the composite's fields, the range's subtype
/// (`docs/design/architecture.md`, "CLI surface").
///
/// **Every arm renders**, not the enum alone: a listing headed `user-defined
/// types` that showed only enums would be a lie about what the dump holds.
/// `Composite { fields: None }` says `(fields not parsed)` explicitly, because
/// that is the one arm whose absence changes how a column of the type
/// resolves; a `Range` naming no subtype says so for symmetry. Neither shape
/// is one `pg_dump` writes, so both are pinned by this module's unit test
/// rather than against a fixture.
///
/// Not to be confused with [`type_kind_label`], which is `--map`'s one-word
/// name for the same vocabulary — a span line has no room for a payload.
fn type_kind_summary(kind: &TypeKind) -> String {
    match kind {
        TypeKind::Enum { labels } if labels.is_empty() => "enum: (no labels)".to_string(),
        TypeKind::Enum { labels } => format!("enum: {}", label_list(labels)),
        TypeKind::Domain { base_type, collation: None } => format!("domain over {base_type}"),
        TypeKind::Domain { base_type, collation: Some(c) } => {
            format!("domain over {base_type} COLLATE {c}")
        }
        TypeKind::Composite { fields: None } => "composite: (fields not parsed)".to_string(),
        TypeKind::Composite { fields: Some(fields) } if fields.is_empty() => {
            "composite: (no fields)".to_string()
        }
        TypeKind::Composite { fields: Some(fields) } => {
            let rendered: Vec<String> = fields
                .iter()
                .map(|f| match &f.collation {
                    Some(c) => format!("{} {} COLLATE {c}", f.name, f.declared_type),
                    None => format!("{} {}", f.name, f.declared_type),
                })
                .collect();
            format!("composite: {}", rendered.join(", "))
        }
        // The `canonical` function is named where the DDL declares one,
        // because it is the whole reason a column of this type refuses every
        // filter operator — a user meeting that refusal comes here to see
        // what the file said.
        TypeKind::Range { subtype, canonical, .. } => {
            let over = match subtype {
                Some(subtype) => format!("range over {subtype}"),
                None => "range (subtype not parsed)".to_string(),
            };
            match canonical {
                Some(function) => format!("{over}, canonical {function}"),
                None => over,
            }
        }
        TypeKind::Base => "base type".to_string(),
        TypeKind::Shell => "shell type".to_string(),
    }
}

/// One `COPY` block's resolved schema, paired back with the block it came
/// from — the single resolution pass `--verbose`'s text and `--json`'s export
/// both render (`docs/design/architecture.md`, "CLI surface"). Two passes is
/// the failure mode here: the export would quietly become a second
/// implementation of what the listing says.
///
/// `complete` says whether `index` covers the file
/// ([`DumpIndex::is_complete`]), which is what decides whether a block's
/// array-shape census may be believed. A *mapped* block's census is always
/// total for that block, but a reported schema answers "what is this table",
/// and one table's data can occupy several blocks (I2) — so a map that
/// stopped short cannot speak for a block past its frontier, and every column
/// resolves optimistically until it can
/// (`docs/design/architecture.md`, "The array shape census").
///
/// A header-less block resolves to an empty schema: its column names come from
/// its first data row, which no index records. It is still listed, so the
/// export's shape does not vary per block.
fn block_resolutions(
    index: &DumpIndex,
    complete: bool,
) -> Vec<(&pgdump_query::CopyBlock, ResolvedSchema)> {
    index
        .blocks()
        .map(|block| {
            let census: &[ArrayShape] = if complete { &block.array_shapes } else { &[] };
            let resolved = resolve_columns(
                &block.header.qualified_name(),
                &block.header.columns,
                index.metadata.as_ref(),
                block.database.as_deref(),
                SchemaMode::Typed,
                census,
            );
            (block, resolved)
        })
        .collect()
}

/// `--json`'s shape: the whole [`DumpIndex`] flattened to one object, plus the
/// three things it does not itself carry — how much of the file it covers, the
/// diagnostics `#[serde(skip)]` drops for the cache's own reasons
/// (`docs/design/architecture.md`, "The cache"), and the per-block type
/// resolution, which is an L2 conclusion an L1 index has no field for. No
/// schema stability is promised for any of this — see the `--json` flag's help
/// text.
///
/// **Coverage is components, not a rendered percentage.** `scanned_through`
/// comes flattened out of the index and `total_size` sits beside it, so a
/// script computes whatever ratio it wants instead of parsing the text
/// listing's line back apart.
#[derive(serde::Serialize)]
struct IndexJson<'a> {
    #[serde(flatten)]
    index: &'a DumpIndex,
    total_size: u64,
    diagnostics: &'a [Diagnostic],
    resolution: Vec<BlockResolutionJson<'a>>,
}

/// One `COPY` block's resolution, keyed by the block rather than rolled up per
/// table. A table can span blocks (I2) and a header-less block names its
/// columns from its first row, so a per-table rollup needs a merge rule that
/// does not exist yet; leaving the grouping to the consumer is where it
/// honestly sits (`docs/design/architecture.md`, "CLI surface").
#[derive(serde::Serialize)]
struct BlockResolutionJson<'a> {
    database: Option<&'a str>,
    table: String,
    header_offset: u64,
    columns: Vec<ColumnResolutionJson<'a>>,
}

/// One column's resolution: what the DDL declared, what it became, and why.
/// `arrow_type` is the exact string `info --verbose` prints for the same
/// column, so the two renderings cannot disagree about the type either.
#[derive(serde::Serialize)]
struct ColumnResolutionJson<'a> {
    name: &'a str,
    declared: Option<&'a str>,
    outcome: &'static str,
    arrow_type: String,
    plan: &'a NestedPlan,
}

fn print_index_json(index: &DumpIndex, total_size: u64, complete: bool) {
    let resolutions = block_resolutions(index, complete);
    let resolution = resolutions
        .iter()
        .map(|(block, resolved)| BlockResolutionJson {
            database: block.database.as_deref(),
            table: block.header.qualified_name(),
            header_offset: block.header_offset,
            columns: resolved
                .notes
                .iter()
                .enumerate()
                .map(|(i, note)| ColumnResolutionJson {
                    name: &note.column,
                    declared: note.declared.as_deref(),
                    outcome: resolution_words(&note.resolution).0,
                    arrow_type: arrow_type_label(
                        resolved.schema.field(i).data_type(),
                        &resolved.plans[i],
                    ),
                    plan: &resolved.plans[i],
                })
                .collect(),
        })
        .collect();
    let wrapped = IndexJson { index, total_size, diagnostics: &index.diagnostics, resolution };
    println!("{}", serde_json::to_string_pretty(&wrapped).expect("DumpIndex is always valid JSON"));
}

/// How much of the file the index covers, stated **once, at the top**, with
/// nothing below it qualified (`docs/design/architecture.md`, "CLI surface").
///
/// A partial index lacks *records*, not confidence: a block enters the map
/// only at a `CopyEnd` watermark and every mapping pass censuses, so every
/// record it holds is complete in itself. There is no half-known block, only
/// blocks past the frontier that are not there at all — which is why this line
/// is the only qualification the listing carries.
///
/// The percentage floors, so it reads 100% only for a genuinely finished scan.
fn completion_line(scanned_through: u64, total_size: u64) -> String {
    // A zero-byte file is trivially covered in full, and has no ratio.
    let percent = (scanned_through.min(total_size) * 100).checked_div(total_size).unwrap_or(100);
    format!("Scan completion: {percent}% ({scanned_through} bytes)")
}

/// Every `pgdq info` rendering goes through here: the coverage line, then the
/// listing or the export.
fn report(index: &DumpIndex, total_size: u64, verbose: bool, map: bool, json: bool) {
    let complete = index.is_complete(total_size);
    if json {
        print_index_json(index, total_size, complete);
        return;
    }
    println!("{}", completion_line(index.scanned_through, total_size));
    println!();
    print_index(index, verbose, map, complete);
}

/// Print the whole listing, below whatever coverage line [`report`] already
/// stated. `complete` is passed straight through to [`block_resolutions`],
/// which is where it means something.
///
/// **Nothing here is qualified by how much of the file was scanned.** The
/// coverage line above says it once; a partial index's records are each
/// complete in themselves (see [`completion_line`]), so repeating the caveat
/// per block would suggest a variation that does not exist.
fn print_index(index: &DumpIndex, verbose: bool, map: bool, complete: bool) {
    if let Some(metadata) = &index.metadata {
        print_metadata(metadata, verbose);
        println!();
    }

    if print_diagnostics(&index.diagnostics) {
        println!();
    }

    let printed_roles = print_cross_references(index);
    let printed_kinds = print_object_kinds(index);
    if printed_roles || printed_kinds {
        println!();
    }

    if map {
        print_map(index);
        println!();
        println!("{} span(s)", index.spans.len());
        return;
    }

    // One resolution pass, shared with `--json` — see `block_resolutions`.
    let blocks = block_resolutions(index, complete);
    if blocks.is_empty() {
        println!("no COPY blocks found");
        return;
    }

    let mut total_columns = 0usize;
    let mut total_unmapped = 0usize;

    let mut headings = DatabaseHeadings::new(blocks.iter().map(|(b, _)| &b.database));

    for (block, resolved) in &blocks {
        headings.before(&block.database);
        println!("{} ({} rows)", block.header.qualified_name(), block.row_count);
        if block.header.columns.is_empty() {
            println!("    columns: (not listed in COPY header)");
        } else {
            let columns: Vec<String> = resolved
                .notes
                .iter()
                .map(|d| match &d.declared {
                    Some(ty) => format!("{} {ty}", d.column),
                    None => format!("{} (unknown)", d.column),
                })
                .collect();
            println!("    columns: {}", columns.join(", "));
            if verbose {
                // One line per column that has something to say. A column
                // that did not map says why; a column that mapped says what
                // it mapped *to*, unless that is `Utf8View` — the
                // no-information answer, and the only Arrow type a
                // non-`Mapped` resolution ever produces, so the two arms
                // never both fire.
                //
                // An enum column then carries its declared labels on a
                // continuation line beneath, uncapped: `Dictionary(Int32,
                // Utf8)` says nothing about *which* labels, and this is the
                // only place a user can read them without grepping the dump
                // for its `CREATE TYPE`. It is a continuation rather than a
                // suffix because one eight-label enum on the column's own
                // line would wrap and break the alignment of every row around
                // it.
                for (i, note) in resolved.notes.iter().enumerate() {
                    let data_type = resolved.schema.field(i).data_type();
                    if note.resolution != ColumnResolution::Mapped {
                        println!("    {}: {}", note.column, resolution_label(&note.resolution));
                    } else if *data_type != DataType::Utf8View {
                        println!(
                            "    {}: {}",
                            note.column,
                            arrow_type_label(data_type, &resolved.plans[i])
                        );
                    }
                    if let Some(labels) = enum_labels(&resolved.comparisons[i]) {
                        println!("        labels: {}", label_list(labels));
                    }
                }
            }
            total_columns += resolved.notes.len();
            total_unmapped += resolved.unmapped_count();
        }
        if verbose {
            println!("    header offset: {}", block.header_offset);
            println!("    data offset:   {}", block.data_offset);
            println!("    terminator:    {}", block.terminator_offset);
            println!("    end offset:    {}", block.end_offset);
        }
    }

    println!();
    // No byte count here: the coverage line above owns that, and stating it
    // twice invites the two to disagree.
    println!("{} COPY block(s), {} row(s)", blocks.len(), index.total_rows());
    if total_unmapped > 0 {
        println!(
            "{total_unmapped} of {total_columns} columns unmapped — run with --verbose for details"
        );
    }
}

/// `DumpIndex::diagnostics` (or, for `--preamble-only`, the diagnostics
/// `preamble_only` reports separately), printed unconditionally — this is
/// (`docs/design/architecture.md`, "The cache"). Cache-only mode's
/// "unverified, historical" banner rides this same path (`DiagnosticKind::CacheOffline`).
/// Returns whether anything was printed, matching `print_cross_references`'s
/// and `print_object_kinds`' convention.
fn print_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    if diagnostics.is_empty() {
        return false;
    }
    println!("diagnostics:");
    for d in diagnostics {
        println!("    [{}] {}", severity_label(d.severity), diagnostic_message(&d.kind));
    }
    true
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

fn diagnostic_message(kind: &DiagnosticKind) -> String {
    match kind {
        DiagnosticKind::TilingBroken { issues } => format!(
            "the file map has {} gap(s)/overlap(s) that don't tile the file — this is a pgdq bug, please report it",
            issues.len()
        ),
        DiagnosticKind::CacheMtimeChanged => "the dump file's mtime has changed since the cache was saved (size still matches, so the cache was kept)".to_string(),
        DiagnosticKind::TocCoverage { attributed, spans } => {
            format!("TOC coverage: {attributed}/{spans} span(s) attributed to a TOC entry")
        }
        DiagnosticKind::CacheOffline => {
            "answering from a cache with no source dump file to check it against — unverified, historical as of whenever the cache was last saved".to_string()
        }
    }
}

/// Referenced-role and referenced-tablespace summary
/// (`docs/design/architecture.md`, "TOC enrichment")
/// — an empty set prints nothing, so a dump referencing neither leaves no
/// trace here. Returns whether anything was printed, so the caller knows
/// whether to add a separating blank line.
fn print_cross_references(index: &DumpIndex) -> bool {
    let mut printed = false;
    if !index.roles.is_empty() {
        println!("roles: {}", index.roles.iter().cloned().collect::<Vec<_>>().join(", "));
        printed = true;
    }
    if !index.tablespaces.is_empty() {
        println!(
            "tablespaces: {}",
            index.tablespaces.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        printed = true;
    }
    printed
}

/// Per-`Type:` object-kind counts — one per archive entry, the same closed
/// ~63-value vocabulary the TOC-coverage diagnostic counts against
/// (`docs/design/architecture.md`, "TOC enrichment"). Counts `toc_owned`
/// spans, not every attributed one: this is an object *census*, and a
/// follow-on statement
/// (`ALTER ... OWNER TO`, etc.) inherits its governing entry's `toc` rather
/// than carrying `None` — counting `span.toc.is_some()` here would count that
/// object twice ("Span boundaries: statement-anchored, object-attributed,
/// greedy"). A span with no TOC comment at all (the header-less-input
/// fallback) contributes to no bucket here, since there is nothing typed to
/// count it under. Returns whether anything was printed.
fn print_object_kinds(index: &DumpIndex) -> bool {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for span in &index.spans {
        if span.toc_owned
            && let Some(toc) = &span.toc
        {
            *counts.entry(toc.kind.as_str()).or_default() += 1;
        }
    }
    if counts.is_empty() {
        return false;
    }
    println!("object kinds:");
    for (kind, count) in counts {
        println!("    {kind}: {count}");
    }
    true
}

/// `--map`: every span the full file map found, in file order — the raw
/// structure `DumpIndex::spans` keeps, not the per-table view `blocks()`
/// filters it down to (`docs/design/architecture.md`,
/// "`DumpIndex`: one owner per fact"). Grouped by [`DatabaseHeadings`], the
/// same convention the ordinary block listing uses.
fn print_map(index: &DumpIndex) {
    let mut headings = DatabaseHeadings::new(index.spans.iter().map(|s| &s.database));
    for span in &index.spans {
        headings.before(&span.database);
        println!("[{}, {}) {}", span.start, span.end, span_summary(span));
    }
}

/// One-line label for a span in `--map` output.
fn span_summary(span: &Span) -> String {
    match &span.body {
        SpanBody::Data(DataBlock::Copy(block)) => {
            format!("COPY {} ({} rows)", block.header.qualified_name(), block.row_count)
        }
        SpanBody::Data(DataBlock::InsertRun(run)) => {
            format!("INSERT run: {} ({} statements)", run.table, run.row_count)
        }
        SpanBody::Data(DataBlock::LargeObjects(_)) => "large objects".to_string(),
        SpanBody::Table { name, .. } => format!("TABLE {name}"),
        SpanBody::TypeDef { name, kind } => format!("TYPE {name} ({})", type_kind_label(kind)),
        SpanBody::Extension { name, .. } => format!("EXTENSION {name}"),
        SpanBody::Collation { collation } => {
            let determinism = if collation.deterministic { "" } else { " (deterministic = false)" };
            format!("COLLATION {}{determinism}", collation.name)
        }
        SpanBody::Connect { database } => format!("\\connect {database}"),
        SpanBody::VersionHeader { .. } => "version header".to_string(),
        SpanBody::AlterTypeAddValue { type_name, label } => {
            format!("ALTER TYPE {type_name} ADD VALUE {label:?}")
        }
        SpanBody::Framing => "framing".to_string(),
        SpanBody::Unparsed => match &span.toc {
            Some(toc) => format!("{} {}", toc.kind, toc.name),
            None => "unparsed".to_string(),
        },
        SpanBody::Unscanned => "unscanned".to_string(),
    }
}

/// Short label for a [`TypeKind`] — `--map`'s compact form of the same
/// six-emission-shape vocabulary `docs/manual/type-handling.md` explains for
/// readers. [`type_kind_summary`] is the `--verbose` type listing's fuller
/// rendering, payload included.
fn type_kind_label(kind: &TypeKind) -> &'static str {
    match kind {
        TypeKind::Enum { .. } => "enum",
        TypeKind::Domain { .. } => "domain",
        TypeKind::Composite { .. } => "composite",
        TypeKind::Range { .. } => "range",
        TypeKind::Base => "base",
        TypeKind::Shell => "shell",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{Field, Fields};
    use std::sync::Arc;

    /// One parsed term, or the message it was refused with.
    fn filter(spec: &str) -> Result<(String, PredicateOp, Option<String>), String> {
        match parse_filter(spec) {
            Ok(p) => Ok((p.column, p.op, p.value)),
            Err(e) => Err(e.to_string()),
        }
    }

    /// `column`, `op` and `value` of a term that must parse.
    fn ok(spec: &str) -> (String, PredicateOp, Option<String>) {
        filter(spec).unwrap_or_else(|e| panic!("`{spec}` should parse: {e}"))
    }

    /// The message a term that must not parse was refused with.
    fn err(spec: &str) -> String {
        match filter(spec) {
            Err(message) => message,
            Ok(parsed) => panic!("`{spec}` should be refused, parsed as {parsed:?}"),
        }
    }

    /// The bare spelling, unchanged: no whitespace anywhere means nothing to
    /// trim and no quote to strip, so the sysadmin-shaped half of the
    /// audience sees exactly what it always did.
    #[test]
    fn a_bare_term_parses_as_it_reads() {
        assert_eq!(ok("name=alpha"), ("name".into(), PredicateOp::Eq, Some("alpha".into())));
        assert_eq!(ok("v>=5"), ("v".into(), PredicateOp::Ge, Some("5".into())));
        assert_eq!(ok("v!=5"), ("v".into(), PredicateOp::Ne, Some("5".into())));
    }

    /// Whitespace outside quotes is not data, on **both** sides of the
    /// operator. Untrimmed, the value side failed loudly on a typed column
    /// and silently on a text one — an empty result that reads as an answer.
    #[test]
    fn whitespace_outside_quotes_is_trimmed_on_both_sides() {
        assert_eq!(ok(" name = alpha "), ("name".into(), PredicateOp::Eq, Some("alpha".into())));
        assert_eq!(ok("v >= 5"), ("v".into(), PredicateOp::Ge, Some("5".into())));
    }

    /// Whitespace is `str::trim`'s, not ASCII space's, so a non-breaking
    /// space pasted out of a web page is caught rather than searched for.
    #[test]
    fn trimming_is_unicode_whitespace() {
        assert_eq!(
            ok("name\u{a0}=\u{a0}alpha\u{a0}"),
            ("name".into(), PredicateOp::Eq, Some("alpha".into()))
        );
    }

    /// An all-whitespace value collapses to the empty string, which needs no
    /// special handling: it is a legitimate value to search for and the
    /// quoted spelling is there for anyone who meant the spaces.
    #[test]
    fn an_all_whitespace_value_collapses_to_empty() {
        assert_eq!(ok("name=   "), ("name".into(), PredicateOp::Eq, Some(String::new())));
    }

    /// A quoted value is taken exactly as written, which is what restores
    /// every value trimming would otherwise make unaskable — a space-padded
    /// `char(n)` value is expressible from the command line, not only through
    /// the API.
    #[test]
    fn a_quoted_value_is_taken_as_written() {
        assert_eq!(ok(r#"name = " x""#), ("name".into(), PredicateOp::Eq, Some(" x".into())));
        assert_eq!(ok("name = 'x '"), ("name".into(), PredicateOp::Eq, Some("x ".into())));
        assert_eq!(ok("name=''"), ("name".into(), PredicateOp::Eq, Some(String::new())));
    }

    /// Both quote characters open a value. Which one a user reaches for is
    /// decided by the shell rather than by taste — the term is normally
    /// already inside shell single quotes — so accepting one would punish
    /// whichever half of the audience picked the other.
    #[test]
    fn both_quote_characters_open_a_value() {
        let (_, _, double) = ok(r#"name="the answer""#);
        let (_, _, single) = ok("name='the answer'");
        assert_eq!(double, Some("the answer".into()));
        assert_eq!(single, double);
    }

    /// A quote inside a quoted part is doubled, as SQL does it. Two quote
    /// characters also give a lazier escape for free: a value holding one can
    /// be written in the other, with no doubling at all.
    #[test]
    fn an_interior_quote_is_doubled_or_written_in_the_other_quote() {
        assert_eq!(ok("note='it''s'").2, Some("it's".into()));
        assert_eq!(ok(r#"note="say ""hi""""#).2, Some(r#"say "hi""#.into()));
        assert_eq!(ok(r#"note="it's""#).2, Some("it's".into()));
    }

    /// A quote that does not open the part is ordinary data — nothing scans
    /// for quotes inside an unquoted value.
    #[test]
    fn a_quote_inside_an_unquoted_value_is_data() {
        assert_eq!(ok("note=don't").2, Some("don't".into()));
        assert_eq!(ok(r#"note=a"b"#).2, Some(r#"a"b"#.into()));
    }

    /// Quotes work on the column side too, and the operator split skips
    /// them — which is the whole point, since it is what makes a column named
    /// `a=b` askable at all.
    #[test]
    fn a_quoted_column_name_survives_the_split() {
        assert_eq!(ok(r#""my column"=x"#).0, "my column");
        assert_eq!(ok(r#""a=b"=x"#), ("a=b".into(), PredicateOp::Eq, Some("x".into())));
        assert_eq!(ok(r#" "a=b" = "y=z" "#).2, Some("y=z".into()));
    }

    /// The split rule is otherwise unchanged: earliest position, longest
    /// spelling, so a value carrying an operator byte still cannot steal it.
    #[test]
    fn the_earliest_operator_outside_quotes_still_wins() {
        assert_eq!(ok("name=alpha>x"), ("name".into(), PredicateOp::Eq, Some("alpha>x".into())));
    }

    /// The two worded infix operators, in every case and with any run of
    /// whitespace between their words — the spelling
    /// `PredicateOp::symbol` already names them by, so the grammar and every
    /// refusal message agree without a second table.
    #[test]
    fn the_distinct_from_forms_parse() {
        assert_eq!(
            ok("v is distinct from 1"),
            ("v".into(), PredicateOp::IsDistinctFrom, Some("1".into()))
        );
        assert_eq!(
            ok("v IS NOT DISTINCT FROM 1"),
            ("v".into(), PredicateOp::IsNotDistinctFrom, Some("1".into()))
        );
        assert_eq!(
            ok("v Is  Not   Distinct\tFrom  ' x'"),
            ("v".into(), PredicateOp::IsNotDistinctFrom, Some(" x".into()))
        );
    }

    /// **A worded operator is a candidate at a position, not a pass of its
    /// own**, so the earliest operator still wins in both directions: the
    /// punctuation one when it is to the left, the phrase when it is.
    #[test]
    fn the_earliest_operator_wins_against_a_worded_one_too() {
        assert_eq!(
            ok("note=a is distinct from b"),
            ("note".into(), PredicateOp::Eq, Some("a is distinct from b".into()))
        );
        assert_eq!(
            ok("a is distinct from b=c"),
            ("a".into(), PredicateOp::IsDistinctFrom, Some("b=c".into()))
        );
    }

    /// The phrase needs whitespace on both sides, which is what keeps every
    /// term that parsed before parsing the same way: a column named
    /// `is distinct from` is still askable unquoted, because what follows the
    /// phrase there is `=` rather than a space.
    #[test]
    fn a_worded_operator_needs_whitespace_around_it() {
        assert_eq!(
            ok("is distinct from=x"),
            ("is distinct from".into(), PredicateOp::Eq, Some("x".into()))
        );
        // Only the exact words, and only with a value after them: a prefix
        // match on `distinctly`, or a `FROM` with nothing behind it, falls
        // through to the usage message rather than splitting.
        for spec in ["v is distinctly from x", "v is distinct from"] {
            let message = err(spec);
            assert!(message.contains("--filter must be"), "`{spec}`: {message}");
        }
    }

    /// **The `IS` forms are the fallback.** Stripping the suffix from the
    /// whole term first made this an `IS NULL` on a column called
    /// `note=this`; an operator outside quotes is looked for first, so it is
    /// the equality it plainly reads as.
    #[test]
    fn a_value_ending_in_is_null_is_not_an_is_null_term() {
        assert_eq!(
            ok("note=this is null"),
            ("note".into(), PredicateOp::Eq, Some("this is null".into()))
        );
    }

    /// The `IS` forms still parse, still case-insensitively, and now take a
    /// quoted column name — which is what lets a column called `is null` be
    /// named at all.
    #[test]
    fn the_is_forms_parse_on_a_term_with_no_operator() {
        assert_eq!(ok("created_at IS NULL"), ("created_at".into(), PredicateOp::IsNull, None));
        assert_eq!(
            ok("created_at is not null"),
            ("created_at".into(), PredicateOp::IsNotNull, None)
        );
        assert_eq!(ok(r#" "my column" Is Null "#).0, "my column");
        assert_eq!(ok(r#""is null" = x"#), ("is null".into(), PredicateOp::Eq, Some("x".into())));
    }

    /// A malformed quote is refused, never reinterpreted — falling back to
    /// the unquoted reading would hand a user who mistyped one quote a value
    /// nobody meant. Unterminated and trailing-text are one fault with one
    /// message, wherever in the term they sit.
    #[test]
    fn a_malformed_quote_is_refused() {
        for spec in ["name='x", "name='x'y", "'name=x", r#"name = "x'"#, "'name' 'is null"] {
            let message = err(spec);
            assert!(message.contains("unbalanced"), "`{spec}`: {message}");
            assert!(message.contains(spec), "`{spec}`: {message}");
        }
    }

    /// A term with neither an operator nor an `IS` form is the usage fault it
    /// always was, and the message still quotes the term back.
    #[test]
    fn a_term_with_no_operator_at_all_is_a_usage_fault() {
        let message = err("nonsense");
        assert!(message.contains("--filter must be"), "{message}");
        assert!(message.contains("nonsense"), "{message}");
    }

    /// **The structural refusal runs before the term grammar**, so a term
    /// that is both structural and unparseable is told which of the two it
    /// is. `and is null` names a column `and` under the old reading and is a
    /// parse error under `--where`; it is refused here for the reserved
    /// spelling, and the remedy is the quoting the grammar already teaches.
    #[test]
    fn the_flag_refuses_structure_before_it_parses_a_term() {
        let structural = parse_filter_flag("and is null").expect_err("a reserved spelling");
        let message = format!("{structural:#}");
        assert!(message.contains("`AND`"), "{message}");
        assert!(!message.contains("--filter must be"), "{message}");
        assert_eq!(
            parse_filter_flag(r#""and" is null"#).expect("the quoted column is askable").column,
            "and"
        );
        // A term with no structure still reaches the term grammar, refusal
        // and all.
        assert!(
            format!("{:#}", parse_filter_flag("nonsense").expect_err("no operator"))
                .contains("--filter must be")
        );
        assert_eq!(
            parse_filter_flag("name=alpha").expect("a plain term").value.as_deref(),
            Some("alpha")
        );
    }

    /// The note a name that was not found earns when it looks quoted —
    /// `--column` and `--table` take their names verbatim, so the quote marks
    /// were part of what was looked for.
    #[test]
    fn a_quoted_looking_name_earns_the_verbatim_note() {
        let note = quoted_name_note("--column", "\"id\"").expect("a quoted-looking name");
        assert!(note.contains("--column"), "{note}");
        assert!(note.contains("matched literally"), "{note}");
        assert_eq!(quoted_name_note("--column", "id"), None);
        assert_eq!(quoted_name_note("--table", "\"id"), None, "one quote is not a pair");
        assert_eq!(quoted_name_note("--table", "\""), None, "one character is not a pair");
    }

    fn list_of(child: DataType) -> DataType {
        DataType::List(Arc::new(Field::new("item", child, true)))
    }

    fn range_struct(bound: DataType) -> DataType {
        DataType::Struct(Fields::from(vec![
            Field::new(RANGE_STRUCT_FIELDS[0], bound.clone(), true),
            Field::new(RANGE_STRUCT_FIELDS[1], bound, true),
            Field::new(RANGE_STRUCT_FIELDS[2], DataType::Boolean, false),
            Field::new(RANGE_STRUCT_FIELDS[3], DataType::Boolean, false),
            Field::new(RANGE_STRUCT_FIELDS[4], DataType::Boolean, false),
        ]))
    }

    /// A scalar column's Arrow type is arrow's own `Display`, unmodified —
    /// which is the half of the line that was never visible before, and the
    /// reason the line is printed for every mapped non-`Utf8View` column
    /// rather than only for nested ones.
    #[test]
    fn a_scalar_column_renders_as_arrows_own_display() {
        assert_eq!(
            arrow_type_label(&DataType::Decimal128(38, 10), &NestedPlan::Scalar),
            "Decimal128(38, 10)"
        );
    }

    #[test]
    fn arrays_and_composites_render_through_arrows_display() {
        assert_eq!(
            arrow_type_label(
                &list_of(DataType::Utf8View),
                &NestedPlan::Array(Box::new(NestedPlan::Scalar))
            ),
            "List(Utf8View)"
        );
        let point = DataType::Struct(Fields::from(vec![
            Field::new("x", DataType::Int32, true),
            Field::new("y", DataType::Utf8View, true),
        ]));
        assert_eq!(
            arrow_type_label(&point, &NestedPlan::Record(vec![NestedPlan::Scalar; 2])),
            r#"Struct("x": Int32, "y": Utf8View)"#
        );
    }

    /// The one substitution: the five-field range struct is identical in
    /// every dump, so only its bound type is worth printing.
    #[test]
    fn the_range_struct_collapses_to_its_bound_type() {
        assert_eq!(
            arrow_type_label(
                &range_struct(DataType::Int32),
                &NestedPlan::Range(Box::new(NestedPlan::Scalar))
            ),
            "Range<Int32>"
        );
    }

    /// A built-in multirange and an array of the matching range are the
    /// *same* Arrow type and different plans, so rendering identically is
    /// correct rather than a collision — the declared PostgreSQL type sits on
    /// the same line and tells them apart.
    #[test]
    fn a_multirange_and_an_array_of_the_matching_range_render_identically() {
        let multirange = arrow_type_label(
            &list_of(range_struct(DataType::Int32)),
            &NestedPlan::Multirange(Box::new(NestedPlan::Scalar)),
        );
        let array_of_range = arrow_type_label(
            &list_of(range_struct(DataType::Int32)),
            &NestedPlan::Array(Box::new(NestedPlan::Range(Box::new(NestedPlan::Scalar)))),
        );
        assert_eq!(multirange, "List(Range<Int32>)");
        assert_eq!(array_of_range, multirange);
    }

    /// Dispatch is the plan's, never the field names' — a user composite may
    /// declare five fields with exactly the range struct's names, and it must
    /// still print as the struct it is.
    #[test]
    fn a_composite_wearing_the_range_structs_field_names_is_not_collapsed() {
        let impostor = range_struct(DataType::Int32);
        let rendered =
            arrow_type_label(&impostor, &NestedPlan::Record(vec![NestedPlan::Scalar; 5]));
        assert!(rendered.starts_with(r#"Struct("lower": Int32"#), "{rendered}");
        assert!(!rendered.contains("Range<"), "{rendered}");
    }

    /// Every `TypeKind` arm renders, with whatever payload it carries. The
    /// fixtures reach all but two of these — a composite whose body held an
    /// unparseable fragment and a range whose parameter list named no
    /// `subtype` are shapes `pg_dump` does not write — so this is where those
    /// two say what they say.
    #[test]
    fn every_type_kind_renders_with_its_own_payload() {
        use pgdump_query::ColumnDef;

        let labels =
            |ls: &[&str]| TypeKind::Enum { labels: ls.iter().map(|l| l.to_string()).collect() };
        assert_eq!(type_kind_summary(&labels(&["sad", "has'quote"])), "enum: 'sad', 'has''quote'");
        assert_eq!(type_kind_summary(&labels(&[])), "enum: (no labels)");
        assert_eq!(type_kind_summary(&TypeKind::domain("integer")), "domain over integer");
        assert_eq!(
            type_kind_summary(&TypeKind::Domain {
                base_type: "text".to_string(),
                collation: Some(r#"pg_catalog."C""#.to_string()),
            }),
            r#"domain over text COLLATE pg_catalog."C""#
        );
        assert_eq!(
            type_kind_summary(&TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("x", "integer"),
                    ColumnDef {
                        name: "c".to_string(),
                        declared_type: "text".to_string(),
                        collation: Some(r#"pg_catalog."C""#.to_string()),
                    },
                ]),
            }),
            r#"composite: x integer, c text COLLATE pg_catalog."C""#
        );
        assert_eq!(
            type_kind_summary(&TypeKind::Composite { fields: Some(vec![]) }),
            "composite: (no fields)"
        );
        assert_eq!(
            type_kind_summary(&TypeKind::Composite { fields: None }),
            "composite: (fields not parsed)"
        );
        assert_eq!(
            type_kind_summary(&TypeKind::Range {
                subtype: Some("double precision".to_string()),
                multirange_type_name: Some("public.myrange_multi".to_string()),
                canonical: None,
            }),
            "range over double precision"
        );
        assert_eq!(
            type_kind_summary(&TypeKind::Range {
                subtype: None,
                multirange_type_name: None,
                canonical: None
            }),
            "range (subtype not parsed)"
        );
        // A declared `canonical` function is named, because it is why every
        // filter operator refuses a column of this type.
        assert_eq!(
            type_kind_summary(&TypeKind::Range {
                subtype: Some("integer".to_string()),
                multirange_type_name: None,
                canonical: Some("public.canonrange_canonical".to_string()),
            }),
            "range over integer, canonical public.canonrange_canonical"
        );
        assert_eq!(type_kind_summary(&TypeKind::Base), "base type");
        assert_eq!(type_kind_summary(&TypeKind::Shell), "shell type");
    }
}
