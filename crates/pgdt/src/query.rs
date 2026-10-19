//! `pgdt query`.

use std::path::PathBuf;

use anyhow::Result;
use arrow::array::RecordBatch;
use clap::Args;
use futures::StreamExt;
use pgdt_codec::pgtype::NestedPlan;
use pgdt_codec::{SchemaMode, UnrepresentableMode};
use pgdt_dump::cache::Held;
use pgdt_io::Origin;
use pgdt_model::{Finding, PostgresInvalidValues, Severity};
use pgdt_query::{QueryOptions, ScanExtent, Table, TableSelector, render_field_into};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::args::{
    IdentityArgs, ModeParser, OverwriteArgs, ReadFlags, cache_mode, parse_chunk_size,
    parse_max_line_bytes, scan_options,
};
use crate::parallel::{ParallelArgs, QueryPasses, Resolved};
use crate::source::{about_the_source, naming_the_source};
use filter::{parse_filter_flag, parse_where};

mod filter;

/// `pgdt query`'s flags.
#[derive(Args)]
pub(crate) struct QueryArgs {
    /// The dump to scan — a path or a URL, as `parse` takes it. `query`
    /// can never answer from a cache alone — row data is never cached —
    /// so this is always required.
    #[arg(long)]
    source: String,
    /// Table name, qualified (`schema.table`) or bare. Taken exactly as
    /// given, like `--column` and unlike a `--filter` term.
    #[arg(long)]
    table: String,
    /// Cache file path, or `none` to ignore any existing cache and
    /// perform a fresh scan without persisting it.
    #[arg(long)]
    dtcache: Option<PathBuf>,
    /// Single-column filter: `column=value`, `column!=value`,
    /// `column<value`, `column<=value`, `column>value`,
    /// `column>=value`, `column IS DISTINCT FROM value`,
    /// `column IS NOT DISTINCT FROM value`, `column IN (value, …)`,
    /// `column IS [NOT] NULL`, or `column IS [NOT] UNREPRESENTABLE`.
    /// Repeatable — every term
    /// must match, so the terms are ANDed. `OR`, negation and grouping
    /// are `--where`, which takes an expression over these same terms;
    /// giving both flags ANDs them (`docs/design/decisions.md`,
    /// "Predicates").
    ///
    /// `IS UNREPRESENTABLE` matches a value PostgreSQL accepts for the
    /// column's declared type and the column's type cannot hold — what
    /// `--unrepresentable null` reads as NULL, which it so tells apart
    /// from the NULLs the dump holds — in any `--unrepresentable` mode,
    /// and is refused under `--schema-mode strings`, which reads no
    /// declared type.
    ///
    /// Every operator but those four tests compares **typed**: the
    /// filter's value is read with the column's own decoder, so a value
    /// that is not of that type is refused by name rather than matching
    /// nothing. The four ordering operators are additionally refused on a
    /// column whose type did not resolve, where `=`/`!=` compare as text;
    /// a nested column compares by its structure under both.
    ///
    /// Spaces around the operator are not data: `name = alpha` asks for
    /// `alpha`. Quote either side — `'` and `"` both work — to say
    /// otherwise: `name = " x"` keeps the leading space, and a quote
    /// inside a quoted part is doubled (`name = 'it''s'`). Quotes work on
    /// the column side too, which is how a column named `a=b` is asked
    /// for: `"a=b"=x`.
    ///
    /// A term is never read as an expression — but nor may it hold what
    /// `--where` would read as one. An unquoted `AND`, `OR` or `NOT` with
    /// whitespace, a paren or an end on both sides, `IS NOT` excepted, or a
    /// paren outside an `IN` list, is refused rather than taken literally, so no string means
    /// one thing here and another under `--where`; quote the part that
    /// holds it, or use `--where`.
    #[arg(long)]
    filter: Vec<String>,
    /// Boolean expression over `--filter`'s terms: `AND`, `OR`, `NOT` and
    /// parens, with `NOT` binding tighter than `AND` and `AND` tighter
    /// than `OR`. The keywords are case-insensitive and are recognised
    /// only as whole words outside quotes, so `--where 'tag=and'` is still
    /// an equality against `and`.
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
    /// (`docs/design/decisions.md`, "D28").
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
    /// a multi-`\connect` dump (`docs/design/decisions.md`,
    /// "D49").
    #[arg(long)]
    database: Option<String>,
    /// `typed` (default) resolves column types against the dump's DDL;
    /// `strings` skips that lookup entirely, matching the untyped
    /// byte-for-byte output — the way out of `Error::MetadataNotScanned`
    /// for a database an incremental scan hasn't read the DDL for yet.
    #[arg(long, default_value_t, value_parser = ModeParser::every(&SchemaMode::ALL))]
    schema_mode: SchemaMode,
    /// What to do with a value PostgreSQL accepts for a column's declared
    /// type and the column's type cannot hold — `infinity` in a `date`,
    /// `NaN` in a `numeric(p,s)`, `24:00:00` in a `time`: `null`, the
    /// default, reads each as NULL, for filtering too, and says on stderr
    /// how many each column printed holds, the filter term
    /// `column IS UNREPRESENTABLE` telling them from the NULLs the dump
    /// holds; `text` reads each column
    /// holding one as its text, every other column keeping its type, and
    /// filters it in its declared type's order; `refuse` refuses, before
    /// a row is read, a query printing a column that holds one, whatever
    /// its filter keeps, and compares one a filter reads in PostgreSQL's
    /// order. `--schema-mode strings` reads every value as its text.
    #[arg(
        long,
        value_name = "MODE",
        default_value_t,
        value_parser = ModeParser::every(&UnrepresentableMode::ALL)
    )]
    unrepresentable: UnrepresentableMode,
    /// What to do with a value PostgreSQL refuses for its column's type —
    /// `70000` in a `smallint`, a `double precision` written past its
    /// range: `default` refuses a query reading one, as a restore refuses
    /// the dump there; `ignore` reads a `real` or `double precision`
    /// written past its range as the largest value of its sign, and one
    /// written below its smallest as zero, a `varchar(n)` or `char(n)`
    /// value past its length as written, and a NULL in a `NOT NULL`
    /// column as NULL, for printing and filtering alike, and still
    /// refuses every other. A filter's own value
    /// PostgreSQL refuses is refused either way.
    #[arg(
        long,
        value_name = "MODE",
        default_value_t,
        value_parser = ModeParser::accepting(
            &PostgresInvalidValues::ALL,
            PostgresInvalidValues::queryable
        )
    )]
    postgres_invalid_values: PostgresInvalidValues,
    /// Bytes requested per read from the dump — the same knob `parse`
    /// carries, and with the same measured answer behind its default.
    #[arg(long, value_name = "BYTES", value_parser = parse_chunk_size)]
    chunk_size: Option<usize>,
    /// The longest line the scan accepts, in bytes — the same limit
    /// `parse` carries, applied to both of a query's passes over the file.
    #[arg(long, value_name = "BYTES", value_parser = parse_max_line_bytes)]
    max_line_bytes: Option<usize>,
    /// Whether to use the statistics `parse` recorded: `all`, the default,
    /// skips each stretch of the table's data whose statistics prove no
    /// row in it satisfies the filter, and says on stderr how much was
    /// skipped, and stops reading data sorted on a column the filter
    /// bounds at its first row past the bound, saying once the rows are
    /// printed how much that left unread; `none` reads every row. The
    /// rows printed are the same either way.
    #[arg(long, value_name = "USE", value_enum, default_value_t)]
    statistics: QueryStatistics,
    #[command(flatten)]
    identity: IdentityArgs,
    #[command(flatten)]
    overwrite: OverwriteArgs,
    #[command(flatten)]
    parallel: ParallelArgs,
}

/// `pgdt query`: stream a table's rows to stdout in file order.
pub(crate) async fn run(args: QueryArgs) -> Result<()> {
    let QueryArgs {
        source: file,
        table,
        dtcache,
        filter,
        where_expr,
        column,
        no_columns,
        database,
        schema_mode,
        unrepresentable,
        postgres_invalid_values,
        chunk_size,
        max_line_bytes,
        statistics,
        identity,
        overwrite,
        parallel,
    } = args;
    let read = ReadFlags { chunk_size, max_line_bytes };
    let origin = Origin::resolve(&file)?;
    let mode = cache_mode(&origin, dtcache.as_deref())
        .with_strict_identity(identity.resolve())
        .with_overwrite_unusable(overwrite.overwrite_unusable_cache);
    // Every term is parsed before the file is opened, so a
    // malformed one is reported without a scan; the library then
    // resolves each against the block's own schema.
    let terms =
        filter.iter().map(String::as_str).map(parse_filter_flag).collect::<Result<Vec<_>>>()?;
    let filter = match where_expr {
        // Byte for byte the tree a repeated `--filter` always built,
        // including the empty conjunction that keeps every row.
        None => pgdt_query::Expr::And(terms),
        Some(spec) if terms.is_empty() => parse_where(&spec)?,
        // Both flags: one conjunction whose children are the
        // expression and each term, the terms not nested a level down.
        Some(spec) => {
            pgdt_query::Expr::And(std::iter::once(parse_where(&spec)?).chain(terms).collect())
        }
    };
    // As `info` and `parse`: the dump is opened through its cache once,
    // the cache decoded once and handed to the pass, and one that does
    // not describe this file is refused having read no more than the
    // file's leading bytes (`docs/design/decisions.md`, "D20").
    // Announced in two lines, the first ahead of the open, as `parse`
    // does.
    let stated = parallel.discover();
    stated.announce();
    let opened = mode.open(&origin).await.map_err(|e| naming_the_source(e, &origin))?;
    let cached_statistics = match &opened.cache {
        Held::Loaded(loaded) => loaded.index.statistics_heap_bytes(),
        Held::Missing | Held::Disabled | Held::Replacing(_) => 0,
    };
    let passes = stated.resolve_query(opened.source.as_ref(), cached_statistics);
    passes.replay.announce();
    let mapping_span = passes.mapping_span();
    let QueryPasses { mapping, replay, .. } = passes;
    // The refusals a query can raise that want more than the
    // library's own words: those about the source gain its name, and
    // a column name not found gains a note. Which wants the name is
    // asked of [`about_the_source`] rather than listed again here — a
    // second list is how this command went slices behind `parse`'s.
    let report = |err: pgdt_core::Error| {
        if about_the_source(&err) {
            naming_the_source(err, &origin)
        } else {
            name_taken_verbatim(err)
        }
    };
    let query_options = QueryOptions {
        schema_mode,
        unrepresentable,
        postgres_invalid_values,
        filter,
        projection: projection(column, no_columns),
        parallelism: replay.parallelism(),
        use_statistics: statistics == QueryStatistics::All,
        ..QueryOptions::default()
    };
    // What the request alone refuses, before the mapping pass reads a
    // byte past the open.
    query_options.validate().map_err(&report)?;
    let selector = table_selector(&table, database);
    // The mapping pass, stopping once the table is settled: only its
    // lines are said in `mapping_span`. Nothing stops it but the
    // process ending, so its token never fires.
    let settled = Table::settle(
        opened,
        &selector,
        scan_options(read, &mapping),
        ScanExtent::UntilTargetSettled,
        schema_mode,
        &CancellationToken::new(),
    )
    .instrument(mapping_span)
    .await
    .map_err(&report)?;
    let Some(settled) = settled else {
        return no_rows(&table, &origin);
    };
    // Partitioned, not serial: `Parallelism::Serial` — `--jobs 1` — is
    // one partition, so the serial path is reached through the same
    // plan rather than branched to (D51).
    let plan = settled.plan(read.options(), query_options).map_err(&report)?;
    // Known from the plan alone, before any block is read — unlike
    // `announce_comparisons` below, which waits on the first batch.
    announce_plan_notes(plan.plan_notes(), &replay);
    // The plan's one schema says how each nested column is written back
    // as a literal: a `List<Struct{…}>` as an array of ranges or a
    // multirange.
    let plans = plan.resolved_schema().plans;
    // The whole table in file order, the earliest failing piece's error
    // after the rows before it (`docs/design/decisions.md`, "D52").
    let mut stream = plan.in_file_order(BATCH_ROWS);
    let mut header_printed = false;
    let mut announced = false;
    let mut any_batch = false;
    let mut rows = 0u64;
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(&report)?;
        if !announced {
            announce_comparisons(&stream);
            announced = true;
        }
        any_batch = true;
        // A zero-column projection prints no header: it would be an
        // empty line, and `--no-columns | wc -l` would come back one
        // too many.
        if !header_printed && batch.num_columns() > 0 {
            let names: Vec<String> =
                batch.schema().fields().iter().map(|f| f.name().clone()).collect();
            println!("{}", names.join("\t"));
            header_printed = true;
        }
        print_batch(&batch, &plans)?;
        rows += batch.num_rows() as u64;
    }
    // A query that matched a block but selected no rows still
    // resolved a schema, so the announcement is owed either way.
    if !announced {
        announce_comparisons(&stream);
    }
    announce_early_stops(&stream);
    if !any_batch {
        return no_rows(&table, &origin);
    }
    eprintln!("{rows} row(s)");
    Ok(())
}

/// The rows a batch of `pgdt query`'s stream holds: what it prints a batch
/// at a time, and nothing a user states.
const BATCH_ROWS: usize = 8192;

/// What `--table` and `--database` select: a name qualified
/// (`schema.table`) or bare (any schema), split at its first `.`, in the
/// database `--database` names, or any (`docs/design/decisions.md`, "D49").
fn table_selector(table: &str, database: Option<String>) -> TableSelector {
    let selector = match table.split_once('.') {
        Some((schema, table)) => TableSelector::named(table).in_schema(schema),
        None => TableSelector::named(table),
    };
    match database {
        Some(database) => selector.in_database(database),
        None => selector,
    }
}

/// Say that `table` yielded nothing in `origin` — no such table, or one
/// holding no row the filter keeps — with the note a quoted name earns.
fn no_rows(table: &str, origin: &Origin) -> Result<()> {
    eprintln!("no rows found for {table} in {origin}");
    if let Some(note) = quoted_name_note("--table", table) {
        eprintln!("note: {note}");
    }
    Ok(())
}

/// The sentence a name that was not found earns when it opens and closes with
/// a matching quote: `--column` and `--table` take their names exactly as
/// given, so the quote marks were part of what was looked for.
///
/// **Only a `--filter` term has quoting to strip**, a term being one string
/// to split into three parts; the shell has already delimited a `--column`
/// argument, and stripping quotes here would make a column genuinely named
/// with quote marks unaskable (`docs/design/decisions.md`, "D60").
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

/// Add [`quoted_name_note`] to the one library refusal that can carry it —
/// the failure is loud either way; the note adds *why* the name was not
/// found.
fn name_taken_verbatim(err: pgdt_core::Error) -> anyhow::Error {
    if let pgdt_core::Error::UnknownProjectionColumn { column, .. } = &err
        && let Some(note) = quoted_name_note("--column", column)
    {
        return anyhow::anyhow!("{err}; {note}");
    }
    err.into()
}

/// What `query` does with the statistics `parse` stored: the CLI spelling of
/// [`QueryOptions::use_statistics`].
#[derive(Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
enum QueryStatistics {
    #[default]
    All,
    None,
}

/// Turn the two projection flags into [`QueryOptions::projection`]. The three
/// states are distinct and none of them is spelled the same way:
/// `--no-columns` is the empty projection (`COUNT(*)`), one or more
/// `--column` is that list in that order, and neither flag is `None` — every
/// column.
///
/// The two flags cannot both be given: clap's `conflicts_with` refuses that
/// before this is reached. A repeated `--column` name is refused by the
/// library as `Error::DuplicateProjectionColumn` before a byte is read, so
/// the CLI and an embedder get the same answer.
fn projection(columns: Vec<String>, no_columns: bool) -> Option<Vec<String>> {
    if no_columns {
        Some(Vec::new())
    } else if columns.is_empty() {
        None
    } else {
        Some(columns)
    }
}

/// Say, once per query and on stderr, which of this query's comparisons do
/// not answer what PostgreSQL's own operator would
/// (`docs/design/decisions.md`, "D59"). Per *term*, not per column: a `text`
/// column filtered with both `<` and `=` warns about the first, not the
/// second.
///
/// **Announced by the CLI rather than by a library channel** — the signal is
/// per-column *and* conditional on a predicate, so query's, while
/// `DumpIndex.diagnostics` is the model's and `ResolvedSchema.notes` the
/// codec's. An embedder
/// drains `TableStream::comparison_notes` into its own
/// `pgdt_model::DiagnosticSink` beside the other two; this command prints
/// each channel where its output already speaks, and adopts no sink.
fn announce_comparisons(stream: &pgdt_query::TableStream<'_>) {
    for note in stream.comparison_notes() {
        eprintln!("warning: {}", note.message());
    }
}

/// Say, once per query and on stderr, when the memory budget in force cut the
/// plan short, and what statistics let it skip: the plan's notes
/// ([`pgdt_query::TablePartitions::plan_notes`]), settled before any
/// partition runs — unlike [`announce_comparisons`], this needs no batch
/// first.
///
/// **Severity and the origin clause are two questions, and the note answers
/// the second.** A note quoting a budget is followed by where that budget came
/// from ([`Resolved::plan_note_origin`]), which only this layer knows, and
/// that is asked of [`pgdt_query::PlanNote::budget_bytes`] rather than read
/// off the arms below: a `note:` may quote a budget, and on a plain source the
/// clause is the only thing saying the carved number is not the allowance that
/// was typed (`docs/design/decisions.md`, "D83", and `KD32`).
fn announce_plan_notes(notes: &[pgdt_query::PlanNote], parallel: &Resolved) {
    let origin = parallel.plan_note_origin();
    for note in notes {
        // The library's severity, as the DataFusion provider's sink hears it;
        // an `Info` here is a `note:`, the word the other query-time facts use.
        let severity = match note.severity() {
            Severity::Info => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        let origin = if note.budget_bytes().is_some() { origin.as_str() } else { "" };
        eprintln!("{severity}: {}{origin}", note.message());
    }
}

/// Say, once the query has drained and only where one fired, what the early
/// stop on sorted data left unread. **Found while rows are read**, so unlike
/// [`announce_plan_notes`] it is said after the fact, the stream in file
/// order summing every partition's ([`pgdt_query::TableStream::early_stops`]),
/// a block split across them counted once. A stop planned and never reached
/// prints nothing: the pruning note already says statistics were consulted.
fn announce_early_stops(stream: &pgdt_query::TableStream<'_>) {
    let unread: Vec<u64> =
        stream.early_stops().iter().filter_map(|stop| stop.unread_bytes).collect();
    if !unread.is_empty() {
        eprintln!(
            "note: reading stopped early in {} block(s) sorted past the filter's bound, so a \
             further {} byte(s) of rows are not read",
            unread.len(),
            unread.iter().sum::<u64>()
        );
    }
}

/// Print one batch's rows tab-separated, `\N` for NULL — mirroring COPY
/// TEXT's own NULL marker. Each field is rendered back to PostgreSQL text via
/// [`render_field_into`], so output is byte-identical whether `--schema-mode`
/// is `typed` or `strings`, but for a value its type cannot hold and an
/// `escape` `bytea` (`docs/design/decisions.md`, "D66").
///
/// **One buffer for the whole batch.** The line is assembled in a `String`
/// that is cleared per row and keeps its capacity across the batch, so a
/// scalar field is written where it will be printed from.
///
/// `plans` is the plan's own [`pgdt_codec::ResolvedSchema::plans`], which
/// is what says whether a `List<Struct{…}>` column is written as an array of
/// ranges or as a multirange. A column with no entry falls back to
/// `NestedPlan::Scalar`, which is right for every non-nested type.
///
/// The `Result` is `render_field_into`'s refusal of a value with no PostgreSQL
/// text form, which **no batch this binary prints can hold**: every typed
/// column here is filled by a decoder whose range its renderer can write
/// back. Propagated rather than unwrapped, an unreachable panic in the output
/// path being a worse answer than an error. A refused value can leave a
/// partial field in the buffer; nothing prints it, the error ending the query.
fn print_batch(batch: &RecordBatch, plans: &[NestedPlan]) -> Result<()> {
    let mut line = String::new();
    for row in 0..batch.num_rows() {
        line.clear();
        for (col, c) in batch.columns().iter().enumerate() {
            if col > 0 {
                line.push('\t');
            }
            let plan = plans.get(col).unwrap_or(&NestedPlan::Scalar);
            if !render_field_into(c.as_ref(), row, plan, &mut line)? {
                line.push_str("\\N");
            }
        }
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
