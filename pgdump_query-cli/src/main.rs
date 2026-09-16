use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use anyhow::{Context, Result};
use arrow::array::RecordBatch;
use arrow::datatypes::DataType;
use clap::{Args, Parser, Subcommand};
use futures::StreamExt;
use pgdump_query::cache::{CacheClaim, CacheEnvelope, CacheMode, CacheStatus, CompressionShape};
use pgdump_query::pgtype::RANGE_STRUCT_FIELDS;
use pgdump_query::resolve::{ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
use pgdump_query::{
    ArrayShape, ByteRangeSource, CompareKind, ComparisonPlan, DataBlock, Diagnostic,
    DiagnosticKind, DumpIndex, DumpMetadata, KnownCompression, NestedPlan, Parallelism, Predicate,
    PredicateOp, QueryOptions, Recognized, ScanOptions, Severity, Span, SpanBody,
    StatisticsRequest, StatisticsSelection, StatisticsTarget, TypeKind, open_local, preamble_only,
    render_field_into,
};

mod alloc;
mod info_statistics;
mod introspect;
mod where_expr;

#[derive(Parser)]
#[command(
    name = "pgdq",
    version = alloc::VERSION,
    about = "Query pg_dump plain-format files without loading them into memory"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// CLI spelling of [`SchemaMode`] (`docs/design/decisions.md`, "D66").
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

/// What `query` does with the statistics `parse` stored: the CLI spelling of
/// [`QueryOptions::use_statistics`].
#[derive(Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
enum QueryStatistics {
    #[default]
    All,
    None,
}

/// The two numbers a caller states its parallelism in, shared by the two
/// scanning commands (`docs/design/decisions.md`, "I/O, memory and parallelism").
///
/// **An omitted `--jobs` is the source's own recommendation**, not a constant
/// here: [`ByteRangeSource::default_workers`] answers it, which is why
/// [`Discovered::resolve`] takes the open source
/// (`docs/design/decisions.md`, "D64"). `--jobs 1` is the serial path as a
/// property of `Parallelism::workers` rather than of anything here.
#[derive(Args)]
struct ParallelArgs {
    /// How many workers pgdq may ask for. Left unstated, the file decides: a
    /// plain dump reads serially, and an `.xz` one takes the CPUs this process
    /// was given, or its own block count where that is smaller — lowered again
    /// to the number of readers the memory budget can pay for, where that is
    /// fewer, whether that budget was discovered or stated with
    /// `--parallel-memory`. A count stated here is never lowered that way.
    ///
    /// **It states what is asked for, not what is delivered.** Two input
    /// shapes admit no parallelism whatever this says: a `.xz` file with one
    /// block, and an `INSERT` run (`docs/manual/dump-inspection.md`). And on a
    /// plain file the read buffer pool's own depth binds before this does, so
    /// a `parse` above `--jobs 4` runs four workers and queues the rest.
    ///
    /// What it reaches is `query`'s row replay, cut into at most this many
    /// sub-streams read at once and merged back into file order; a `parse`'s
    /// structure scan, which splits the interior of every `COPY` block large
    /// enough to cut and folds the answers back into one block list; and how
    /// many decoded `.xz` blocks the source retains: four, or one per
    /// would-be reader where that is more.
    ///
    /// **On `query`, `--parallel-memory` has to cover a second cost before
    /// this number is delivered at all** — see that flag's own doc.
    #[arg(long, value_name = "N", value_parser = parse_jobs)]
    jobs: Option<usize>,
    /// What those workers may hold between them in read buffers, in bytes.
    /// Left unstated, pgdq reads the memory limit it is running under — the
    /// smallest cgroup limit that binds — less a 384 MiB reserve, and takes
    /// inside that what the file asks for: one reader's worth for each worker
    /// it would run, rather than the whole allowance. Where no limit is set
    /// that same number is held under half the memory the machine reports
    /// available. A plain dump asks for nothing of its own and gets 64 MiB, or
    /// the limit less the reserve where that is smaller.
    ///
    /// It is a real bound rather than a target: a `.xz` file that does not
    /// leave room inside it for one reader — one of its blocks, a read buffer
    /// and the decompressor's own working memory, beside the three further
    /// blocks the pool keeps however few workers run — is read through the
    /// streaming decoder instead of being decoded a block at a time, which is
    /// correct but slower on backward reads. Raise it to buy the block path
    /// back on a file written with large blocks (`xz -9 -T0`,
    /// `xz --block-size=`).
    ///
    /// **It is also what decides how many of `--jobs`' workers read at once**,
    /// on both commands: the largest count whose whole cost fits inside it. For
    /// an ordinary 24 MiB-block `.xz` the first reader is about 106 MiB and each
    /// one past the fourth about 58, so a 64 MiB budget affords none of them —
    /// which is why a discovered budget is sized off the count the source
    /// recommends rather than off a constant.
    ///
    /// **On `query` it divides by two terms rather than one**: what a worker
    /// costs to read, plus the 64 MiB a sub-stream's held batch may pin
    /// (`docs/design/decisions.md`, "I/O, memory and parallelism") —
    /// charged on a plain file, where a batch pins read buffers the first term
    /// never counted, and not on a block-decoding `.xz`, whose first term
    /// already counts a decoded block. At 64 MiB — which is what a plain file
    /// gets unless a limit or a flag says otherwise — the plain file's sum
    /// already exceeds the budget, so `query --jobs N` on one runs serially
    /// however large `N` is; raise it past roughly 145 MiB to get a second
    /// sub-stream at all.
    /// `parse` is unaffected by the second term: it builds no batches, so
    /// nothing on that path pins a span.
    #[arg(long, value_name = "BYTES", value_parser = parse_parallel_memory)]
    parallel_memory: Option<u64>,
}

impl ParallelArgs {
    /// [`Discovered::resolve`] under `/`, composing both halves in one call —
    /// what a test wants and production cannot use, a run having a line to
    /// print between them.
    #[cfg(test)]
    fn resolve(&self, source: &dyn ByteRangeSource) -> Resolved {
        self.resolve_in(Path::new("/"), source)
    }

    /// [`ParallelArgs::resolve`] against an arbitrary filesystem root, for the
    /// reason [`pgdump_query::discover_memory_limit_in`] takes one: no machine
    /// is a v1 hierarchy, an unlimited host and an allocation under the
    /// reserve at once (`pgdump_query-cli/tests/data/runtime/`).
    #[cfg(test)]
    fn resolve_in(&self, root: &Path, source: &dyn ByteRangeSource) -> Resolved {
        self.discover_in(root).resolve(source)
    }

    /// The half of the resolution that needs no dump: the flags as typed, and
    /// the limit this process runs under. A separate step because everything
    /// on this side is true before the file is touched, while opening a fresh
    /// `.xz` walks every stream footer first
    /// (`docs/design/decisions.md`, "D64").
    fn discover(&self) -> Discovered<'_> {
        self.discover_in(Path::new("/"))
    }

    /// [`ParallelArgs::discover`] against an arbitrary filesystem root, for the
    /// same reason `ParallelArgs::resolve_in` takes one.
    fn discover_in<'a>(&'a self, root: &'a Path) -> Discovered<'a> {
        // **Read even where `--parallel-memory` was stated**: the mode is a
        // fact about the run, not the flag. Both status lines state this read
        // — `memory.high` is writable by whoever set it, so a second walk
        // could answer differently.
        //
        // Deficiency register: `deficiency: KD29` — a flagless budget is not
        // taken from this read: `Resolved` calls `Parallelism::discover_in`,
        // which walks the limit again, so a limit rewritten between the two
        // walks is announced as one number and budgeted as another. **(c)
        // unowned**; promoted by a limit seen to move inside a run, or by a
        // change to `Parallelism`'s discovery that can take a limit already
        // read.
        Discovered { args: self, root, limit: pgdump_query::discover_memory_limit_in(root) }
    }
}

/// What a run knows about its own allowance **before the dump is opened**: the
/// two flags exactly as they were typed, and the memory limit this process is
/// running under with the file that stated it. Nothing here is downstream of
/// the source ([`ParallelArgs::discover`]).
struct Discovered<'a> {
    args: &'a ParallelArgs,
    /// The filesystem root the limit was read under, kept because
    /// `Parallelism::discover_in` asks the same root again, for the limit
    /// (`KD29`) and for what the machine reports free.
    root: &'a Path,
    /// The memory limit this process runs under, and the file that stated it.
    limit: Option<pgdump_query::MemoryLimit>,
}

impl Discovered<'_> {
    /// A flag's value exactly as typed, or `(not stated)`.
    ///
    /// Parenthesised, like every other provenance marker on these lines, so
    /// that absence can never be read as a value.
    fn flag_display(value: Option<u64>) -> String {
        match value {
            Some(v) => v.to_string(),
            None => "(not stated)".to_string(),
        }
    }

    /// Say what was stated or discovered, before a byte of the dump is read.
    ///
    /// **The flags are named as flags here, and nowhere else.** Every other
    /// status line names the arrangement in the library's own vocabulary
    /// (`docs/design/decisions.md`, "D64"); this one reports what was *typed*.
    fn announce(&self) {
        let jobs_flag = Self::flag_display(self.args.jobs.map(|j| j as u64));
        let parallel_memory_flag = Self::flag_display(self.args.parallel_memory);
        match &self.limit {
            Some(limit) => tracing::info!(
                limit_bytes = limit.bytes,
                limit_read_from = %limit.read_from.display(),
                jobs_flag = %jobs_flag,
                parallel_memory_flag = %parallel_memory_flag,
                "running inside a stated memory allocation",
            ),
            None => tracing::info!(
                jobs_flag = %jobs_flag,
                parallel_memory_flag = %parallel_memory_flag,
                "no memory limit found: nothing is enforcing one on this process",
            ),
        }
    }

    /// The [`Parallelism`] these flags state over `source`, filling in what was
    /// omitted.
    ///
    /// **A stated flag wins outright; absence is what asks the source**
    /// (`docs/design/decisions.md`, "D64"). There is no spelling for
    /// "discover", and `--jobs 0` is refused by [`parse_jobs`].
    ///
    /// `Parallelism::discover_for` is asked what the environment allows only
    /// where `--parallel-memory` is absent; what the resulting budget affords
    /// still binds through `stream::worker_count`. Only a *recommended* count
    /// is lowered to fit, and by a stated budget
    /// (`Parallelism::recommended_within`, whose bytes are then taken whole)
    /// as well as a discovered one (`docs/design/roadmap.md`, "A default runs
    /// as fast as the allocation permits"); a discovered limit may put the
    /// budget below `DEFAULT_MEMORY_BUDGET`.
    ///
    /// **A stated budget reaches the library at every worker count**, the
    /// serial state carrying one of its own
    /// (`docs/design/decisions.md`, "I/O, memory and parallelism"). Only a
    /// resolved-serial arrangement can state no budget, which is what lets the
    /// status line say `(default)` truthfully: `Workers::memory_bytes` is not
    /// an `Option`, so that variant carries `DEFAULT_MEMORY_BUDGET` bare.
    fn resolve(self, source: &dyn ByteRangeSource) -> Resolved {
        let args = self.args;
        // Asked of the source only where `--jobs` was absent: a stated count
        // is not a recommendation and has nothing to be lowered from.
        let recommended_jobs = match args.jobs {
            Some(_) => None,
            None => Some(source.default_workers()),
        };
        let jobs = args.jobs.or(recommended_jobs).unwrap_or(1);
        let parallelism = match args.parallel_memory {
            // A stated budget is taken whole — the flag wins outright — but a
            // *recommended* count still answers to it, the rule being scoped
            // to the absence of `--jobs`.
            Some(stated) => match recommended_jobs {
                Some(asked) => {
                    Parallelism::recommended_within(asked, source.default_worker_memory(), stated)
                }
                None => Parallelism::workers(jobs, stated),
            },
            None => {
                let discovered =
                    Parallelism::discover_in(self.root, jobs, source.default_worker_memory());
                match (args.jobs, discovered.memory_bytes()) {
                    // A stated count is not lowered by the environment; what
                    // the budget delivers still binds through
                    // `stream::worker_count`.
                    (Some(stated), Some(bytes)) => Parallelism::workers(stated, bytes),
                    _ => discovered,
                }
            }
        };
        Resolved {
            parallelism,
            limit: self.limit,
            budget_stated: args.parallel_memory.is_some(),
            recommended_jobs,
        }
    }
}

/// What a run resolved its two parallelism numbers to, and where each came
/// from — the arrangement plus the provenance [`Parallelism`] has nowhere to
/// carry. **Provenance is the CLI's fact, not the library's**, and cannot be
/// recovered from a [`Parallelism`] downstream
/// (`docs/design/decisions.md`, "D64").
#[derive(Debug, Clone)]
struct Resolved {
    /// The arrangement the library is handed.
    parallelism: Parallelism,
    /// The memory limit this process runs under, and the file that stated it;
    /// `None` means no limit is being *enforced*.
    limit: Option<pgdump_query::MemoryLimit>,
    /// Whether `--parallel-memory` was given.
    budget_stated: bool,
    /// What the source recommended for a worker count, or `None` where
    /// `--jobs` was stated — in which case the count is that flag's, unlowered.
    recommended_jobs: Option<usize>,
}

impl Resolved {
    fn parallelism(&self) -> Parallelism {
        self.parallelism
    }

    /// The worker count and its provenance, for a status line.
    ///
    /// **A recommended count reads differently from a stated one**: a
    /// recommendation is printed lowered to what the allowance affords, a
    /// stated `--jobs` as typed. What did the lowering is named — `by the
    /// allocation` or `by the stated budget`, those being different numbers to
    /// change — with [`Resolved::budget_display`] beside it naming the number
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits").
    fn jobs_display(&self) -> String {
        let jobs = self.parallelism.jobs();
        match self.recommended_jobs {
            None => format!("{jobs} (stated)"),
            Some(asked) if asked > jobs => {
                let by = if self.budget_stated { "the stated budget" } else { "the allocation" };
                format!("{jobs} (recommended by the source; lowered from {asked} by {by})")
            }
            Some(_) => format!("{jobs} (recommended by the source)"),
        }
    }

    /// The byte budget and its provenance.
    ///
    /// Four spellings, for the four ways to arrive at a number: the flag; a
    /// discovered limit, named by the file that stated it (`memory.high`
    /// throttles where `memory.max` kills, and either may be an ancestor's);
    /// the source's own recommendation; and the library's constant, which is
    /// what "no limit found" leaves a source that recommends nothing.
    fn budget_display(&self) -> String {
        let bytes = self.parallelism.memory_bytes().unwrap_or(pgdump_query::DEFAULT_MEMORY_BUDGET);
        if self.budget_stated {
            return format!("{bytes} (stated)");
        }
        match (&self.limit, self.parallelism.memory_bytes()) {
            (Some(limit), _) => format!(
                "{bytes} (discovered: {} states a limit of {} byte(s))",
                limit.read_from.display(),
                limit.bytes
            ),
            (None, Some(_)) => format!("{bytes} (no limit found: what this source asks for)"),
            (None, None) => format!("{bytes} (default: no limit found)"),
        }
    }

    /// The clause a plan note carries when the budget that produced it was
    /// **not** stated — the half of the story the library cannot tell. Every
    /// [`pgdump_query::PlanNote`] names the budget that bound the plan, so the
    /// CLI appends where that number came from: the same
    /// [`Resolved::budget_display`] the mode report prints
    /// (`docs/design/decisions.md`, "D64").
    ///
    /// **Empty where `--parallel-memory` was stated**, the note already naming
    /// the number that person typed.
    fn plan_note_origin(&self) -> String {
        if self.budget_stated {
            return String::new();
        }
        format!(" — the budget in force is {}", self.budget_display())
    }

    /// Say, once per scanning command and before the scan starts, what the
    /// source's recommendation and the allowance fitted to.
    ///
    /// **The only line that can name a count the allowance lowered**, which is
    /// why the report is two lines: neither number exists until the file has
    /// been opened and asked ([`Discovered::announce`] carries the half that
    /// does). A recommendation cut to fit otherwise surfaces as unexplained
    /// slowness — with no limit found pgdq stays inside half of what the
    /// kernel says is available (`RT8`)
    /// (`docs/design/decisions.md`, "D64").
    fn announce(&self) {
        tracing::info!(
            jobs = %self.jobs_display(),
            memory_bytes = %self.budget_display(),
            "resolved the arrangement",
        );
    }
}

/// A `--jobs` value: a worker count, and never zero. Zero would read as one
/// through `Parallelism::workers`, and silently answering "serial" is a
/// surprise a flag should not hold (`docs/design/decisions.md`, "D64").
fn parse_jobs(text: &str) -> std::result::Result<usize, String> {
    match text.parse::<usize>() {
        Ok(0) => Err("a job count of 0 would run no workers; --jobs 1 is the serial path".into()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

/// A `--parallel-memory` value: a byte count, and never zero. Zero affords no
/// buffer of any unit, so every pool would fall back to its one-slot floor and
/// a compressed source to its streaming reader.
fn parse_parallel_memory(text: &str) -> std::result::Result<u64, String> {
    match text.parse::<u64>() {
        Ok(0) => Err("a parallel memory budget of 0 leaves no room for a read buffer".into()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
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
        /// follows (`docs/design/decisions.md`, "D30"). The cache this leaves is a partial one that `pgdq info`
        /// reads like any other.
        #[arg(long)]
        preamble_only: bool,
        /// Bytes requested per read from the dump. The default, 1 MiB, is
        /// the fastest of the six sizes measured on the one device class
        /// where a chunk size shows anything at all, and makes no measurable
        /// difference on the other two (`docs/design/measurements.md`, "What the read
        /// chunk size is worth") — so this is a tuning escape hatch for a
        /// device unlike those, not a knob with a win behind it. A raised
        /// value keeps its buffer pooling and costs memory instead: the read
        /// path holds four buffers of whatever size you ask for, or
        /// `--parallel-memory`'s worth, whichever is fewer.
        #[arg(long, value_name = "BYTES", value_parser = parse_chunk_size)]
        chunk_size: Option<usize>,
        /// The longest line the scan accepts, in bytes. A longer one stops the
        /// scan with an error naming its offset rather than being buffered
        /// without bound; the check is made as each read completes, so a line
        /// can overrun the limit by up to one read chunk before it is refused.
        /// The default, 64 MiB, holds any row of ordinary
        /// values; a dump holding larger ones — a single value of hundreds of
        /// megabytes — needs a larger limit stated here. A row is held whole
        /// while it is scanned, so this is also what one row may cost in
        /// memory.
        #[arg(long, value_name = "BYTES", value_parser = parse_max_line_bytes)]
        max_line_bytes: Option<usize>,
        /// Which per-row-group column statistics to gather: `all`, the
        /// default; `none`; or a comma-separated list of the tables
        /// (`schema.table`, or a bare `table` in any schema) and columns
        /// (`schema.table.column`) to gather them for alone. Statistics record,
        /// for each stretch of a table's data, its row count and each column's
        /// NULL count, and where a column's comparison allows, its least and
        /// greatest value and its distinct values. Gathering reads every value
        /// of every tracked column, which costs the scan time and memory;
        /// `none` scans as fast as the file allows. A block an earlier `parse`
        /// mapped without the statistics asked for here is re-read for them
        /// once the rest of the file is scanned, keeping any it already had.
        #[arg(
            long,
            value_name = "SELECTION",
            value_parser = parse_statistics,
            conflicts_with = "preamble_only"
        )]
        statistics: Option<StatisticsSelection>,
        /// The bytes of a table's data each row group of statistics covers, a
        /// power of two. The default, 1 MiB, is coarse, and doubles for a
        /// table whose data would take more than 4,096 groups until it takes
        /// no more, and for a table whose rows are too wide for its groups to
        /// hold `--statistics-min-rows`; a stated size is kept exactly, and a
        /// smaller group records more finely where values lie and costs memory
        /// and cache space in proportion. Stated, it also re-reads every block
        /// gathered at another size; left unstated, a block keeps the size it
        /// was gathered at.
        #[arg(
            long,
            value_name = "BYTES",
            value_parser = parse_statistics_group_size,
            conflicts_with = "preamble_only"
        )]
        statistics_group_size: Option<NonZeroU64>,
        /// The fewest rows a row group of statistics should hold under the
        /// default group size. Once a table's data is read, its groups double
        /// until at most half of them fall short of this many rows or the
        /// table is one group, so a table of wide rows keeps fewer groups; the default,
        /// 1,024, leaves rows up to about 1 KiB wide at 1 MiB a group, and 0
        /// doubles nothing. Stated, it also re-reads every block sized under
        /// another minimum or at a stated group size; left unstated, a block
        /// keeps the size it was gathered at. A stated
        /// `--statistics-group-size` is kept exactly, so the two are refused
        /// together.
        #[arg(
            long,
            value_name = "ROWS",
            conflicts_with_all = ["preamble_only", "statistics_group_size"]
        )]
        statistics_min_rows: Option<u64>,
        #[command(flatten)]
        parallel: ParallelArgs,
    },
    /// Report what a dump's cache holds. **`info` never scans** — it reads the
    /// cache `pgdq parse` wrote and errors if there is not one, rather than
    /// starting an hours-long scan on your behalf
    /// (`docs/design/decisions.md`, "D61"). A cache from an
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
        /// Also report each `COPY` block's byte offsets and, per column, what
        /// it became: the Arrow type it resolved to, or — for a column that
        /// came back as a string — why. Turns the `user-defined types` count
        /// into a listing of the types themselves, on a compressed dump adds
        /// the container's shape, and closes the listing, above its totals,
        /// with the statistics `parse` gathered, per table and column: over
        /// how many blocks, at what group size, and in how many groups each
        /// column keeps bounds and a dictionary.
        #[arg(long)]
        detail: bool,
        /// List every span the map holds (`docs/design/decisions.md`,
        /// "D34") — DDL objects
        /// and framing included, not just `COPY` blocks — instead of the
        /// per-table listing.
        #[arg(long)]
        map: bool,
        /// Print the internal index as compact JSON instead of the
        /// human-readable listing: the whole cache file — the `DumpIndex`,
        /// every block's per-group statistics included, and the envelope
        /// around it: format version, container kind, seek table and source
        /// identity — with its coverage, its diagnostics, and the
        /// per-`COPY`-block type resolution `--detail` renders as text.
        /// Statistics are not rolled up per table, as `--detail` rolls them.
        /// No schema stability is promised — this is a raw dump of our
        /// internal representation, not a supported interchange format
        /// (`docs/design/decisions.md`, "D67"). Incompatible with
        /// `--detail`/`--map`, which format detail this already carries in
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
        /// them (`docs/design/decisions.md`, "Predicates").
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
        #[arg(long, value_enum, default_value_t)]
        schema_mode: CliSchemaMode,
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
        /// rows printed are the same either way. A value that fails to decode
        /// is reported only where its row is read, so `none` is also how to
        /// find one in data left unread.
        #[arg(long, value_name = "USE", value_enum, default_value_t)]
        statistics: QueryStatistics,
        #[command(flatten)]
        parallel: ParallelArgs,
    },
}

/// A `--chunk-size` value: a byte count, and never zero.
///
/// Zero is refused here rather than at the read loop, whose
/// `min(chunk_size, remaining)` would ask for nothing forever — a scan that
/// never advances and never errors.
fn parse_chunk_size(text: &str) -> std::result::Result<usize, String> {
    match text.parse::<usize>() {
        Ok(0) => Err("a chunk size of 0 would read nothing".to_string()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

/// A `--max-line-bytes` value: a byte count, and never zero.
///
/// Zero would refuse every line that crosses a read, which is every line of
/// a file longer than one chunk — a limit nobody means, so it is refused
/// here rather than reported as a line too long.
fn parse_max_line_bytes(text: &str) -> std::result::Result<usize, String> {
    match text.parse::<usize>() {
        Ok(0) => Err("a line limit of 0 would refuse every line a read splits".to_string()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

/// A `--statistics` value: `none`, `all`, or a comma-separated list of tables
/// and `schema.table.column`s. A name is split at its dots, so a quoted
/// identifier holding one cannot be named here.
fn parse_statistics(text: &str) -> std::result::Result<StatisticsSelection, String> {
    match text {
        "none" => return Ok(StatisticsSelection::None),
        "all" => return Ok(StatisticsSelection::All),
        _ => {}
    }
    let targets = text
        .split(',')
        .map(|entry| {
            let entry = entry.trim();
            let parts: Vec<&str> = entry.split('.').collect();
            if parts.iter().any(|part| part.is_empty()) {
                return Err(format!("{entry:?} is not a table or a schema.table.column"));
            }
            match parts.as_slice() {
                [_] | [_, _] => Ok(StatisticsTarget::Table(entry.to_string())),
                [schema, table, column] => Ok(StatisticsTarget::Column {
                    table: format!("{schema}.{table}"),
                    column: (*column).to_string(),
                }),
                _ => Err(format!("{entry:?} has more parts than schema.table.column")),
            }
        })
        .collect::<std::result::Result<Vec<_>, String>>()?;
    Ok(StatisticsSelection::Only(targets))
}

/// A `--statistics-group-size` value: a byte count, never zero, which would
/// put every row in a group of its own past the end of the data, and a power
/// of two, so that every size a block's groups can merge to nests in it.
fn parse_statistics_group_size(text: &str) -> std::result::Result<NonZeroU64, String> {
    match text.parse::<u64>() {
        Ok(0) => Err("a group size of 0 covers no bytes".to_string()),
        Ok(n) if !n.is_power_of_two() => {
            Err(format!("a group size must be a power of two, and {n} is not"))
        }
        Ok(n) => Ok(NonZeroU64::new(n).expect("not zero")),
        Err(e) => Err(e.to_string()),
    }
}

/// What `parse` gathers, from its three statistics flags: `--statistics`
/// absent is every column, and a size or a minimum beside `none` is refused
/// rather than ignored.
fn statistics_request(
    selection: Option<StatisticsSelection>,
    group_size: Option<NonZeroU64>,
    min_rows: Option<u64>,
) -> Result<StatisticsRequest> {
    let selection = selection.unwrap_or_default();
    if selection == StatisticsSelection::None {
        let sizing = [
            (group_size.is_some(), "--statistics-group-size"),
            (min_rows.is_some(), "--statistics-min-rows"),
        ];
        if let Some((_, flag)) = sizing.iter().find(|(stated, _)| *stated) {
            anyhow::bail!(
                "{flag} sizes the statistics `--statistics none` turns off — drop one of them"
            );
        }
    }
    Ok(StatisticsRequest { selection, group_size, min_rows })
}

/// The two read flags every scanning command carries, as given.
#[derive(Clone, Copy)]
struct ReadFlags {
    chunk_size: Option<usize>,
    max_line_bytes: Option<usize>,
}

/// The [`ScanOptions`] one scanning command runs under: the default, with
/// `--chunk-size` and `--max-line-bytes` applied where they were given and the
/// already-resolved arrangement ([`Discovered::resolve`]).
///
/// **Resolved once per command and passed in, not re-resolved here.** `query`
/// needs the same arrangement in [`QueryOptions`] as in its mapping pass.
fn scan_options(read: ReadFlags, parallel: &Resolved) -> ScanOptions {
    ScanOptions {
        chunk_size: read.chunk_size.unwrap_or(pgdump_query::DEFAULT_CHUNK_SIZE),
        max_line_bytes: read.max_line_bytes.unwrap_or(pgdump_query::DEFAULT_MAX_LINE_BYTES),
        parallelism: parallel.parallelism(),
        ..ScanOptions::default()
    }
}

/// Turn the two projection flags into [`QueryOptions::projection`]. The three
/// states are distinct and none of them is spelled the same way:
/// `--no-columns` is the empty projection (`COUNT(*)`), one or more
/// `--column` is that list in that order, and neither flag is `None` — every
/// column (`docs/design/decisions.md`, "D28").
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

/// The comparison spellings, in the order they are tried **at one position**
/// — longest first, so `>=` is never read as `>` followed by a stray `=`
/// (`docs/design/decisions.md`, "D60").
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
/// bytes it runs for — the two worded infix operators, candidates at the same
/// positions the punctuation spellings are, so **the earliest operator still
/// wins** (`docs/design/decisions.md`, "D60").
///
/// **Whitespace is required on both sides of the phrase**, so a column named
/// `is distinct from` is still askable as `is distinct from=x`. Any run of
/// whitespace separates the words, as in SQL, and the case is free.
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
    // A value has to follow, and be separated from `FROM`: otherwise
    // `v is distinct from` alone splits into an empty value rather than
    // falling through to the usage message.
    if !bytes.get(end).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    Some((end - i, op))
}

/// Split `spec` at its operator.
///
/// **The earliest position wins, and the longest spelling at that position**
/// (`docs/design/decisions.md`, "D60") — scanning by position rather than by
/// operator is what keeps a value holding an operator byte from stealing the
/// split, the two worded operators ([`distinct_from_at`]) included.
///
/// **The scan skips quoted regions**, so a column named `a=b` is askable as
/// `"a=b"=x`. A quote that never closes is its own outcome rather than "no
/// operator": reinterpreting the term without the operator it swallowed is
/// the silent-wrong-answer shape this grammar exists to remove.
fn split_filter_op(spec: &str) -> FilterSplit<'_> {
    let bytes = spec.as_bytes();
    // The scan walks *bytes*, and compares bytes: every character it looks
    // for is ASCII and no byte of a multi-byte UTF-8 character is, so a match
    // is always at a character boundary and the `spec[..i]` slices below are
    // safe. Matching through `str` would panic on the interior byte of a
    // multi-byte character, which trimming's Unicode whitespace admits.
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
/// well-formed quoted string; an unterminated quote and text after the closing
/// one are deliberately the *same* fault, falling back to the unquoted reading
/// handing a user who mistyped one quote a value nobody meant.
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
/// outside the quotes trimmed off, and a quoted part taken exactly as written.
/// `what` names the side for the error message. Trimming is `str::trim`, the
/// same definition the `IS NULL` forms use, so the parser holds one notion of
/// whitespace.
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

/// One `--filter` argument: the term grammar below, and before it
/// [`where_expr::refuse_where_structure`]. That runs first, so a term both
/// structural and malformed earns the structural message —
/// `--filter 'and is null'` is told that `AND` is a reserved spelling.
fn parse_filter_flag(spec: &str) -> Result<Predicate> {
    where_expr::refuse_where_structure(spec)?;
    parse_filter(spec)
}

/// Parse one filter term into a [`Predicate`] — one term of the conjunction a
/// repeated `--filter` builds, and equally the **leaf** of a `--where`
/// expression ([`where_expr`]): `column<op>value` for any of the six
/// comparison spellings, `column IS [NOT] DISTINCT FROM value`, or
/// `column IS NULL` / `column IS NOT NULL`, the worded forms matched
/// case-insensitively (`docs/design/decisions.md`, "D60").
///
/// **The `IS` forms are the fallback, not the first test**: an operator
/// outside quotes is looked for first and the suffix is only stripped from a
/// term that has none, so `note=this is null` is an equality against
/// `this is null`.
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
fn name_taken_verbatim(err: pgdump_query::Error) -> anyhow::Error {
    if let pgdump_query::Error::UnknownProjectionColumn { column, .. } = &err
        && let Some(note) = quoted_name_note("--column", column)
    {
        return anyhow::anyhow!("{err}; {note}");
    }
    err.into()
}

/// One sub-stream's place in `pgdq query`'s k-way merge: at most one batch,
/// held with the two things a `RecordBatch` does not carry and the printer
/// needs (`docs/design/decisions.md`, "D51").
///
/// **One batch per partition is the whole bound**: each sub-stream yields in
/// file order and the sub-streams themselves are in file order, so emitting
/// the held batch with the lowest source offset re-assembles the serial order
/// while never holding more than N batches.
enum Slot {
    /// Nothing held: this sub-stream is polled in the next fill round.
    Empty,
    /// A batch waiting its turn, with the source offset it begins at — the
    /// merge key — and the nested plans of the block it came from, read when
    /// it was taken because its sub-stream may since have moved to a block
    /// with a different schema.
    Held { offset: u64, batch: RecordBatch, plans: Vec<NestedPlan> },
    /// Drained, stopped at an error, or sitting at or after one. Never polled
    /// again, and a batch it was holding is discarded.
    Done,
}

/// Say, once per query and on stderr, which of this query's comparisons do
/// not answer what PostgreSQL's own operator would
/// (`docs/design/decisions.md`, "D59"). Per *term*, not per column: a `text`
/// column filtered with both `<` and `=` warns about the first, not the
/// second.
///
/// **Announced by the CLI rather than by a library channel** — the signal is
/// per-column *and* conditional on a predicate, so L4, while
/// `DumpIndex.diagnostics` is L1 and `ResolvedSchema.notes` L2. An embedder
/// reads `TableStream::comparison_notes` for the same facts; what it *should*
/// be handed is filed in
/// `docs/design/roadmap-P6-embeddable-engine-inbox.md`.
fn announce_comparisons(stream: &pgdump_query::TableStream<'_>) {
    for note in stream.comparison_notes() {
        eprintln!("warning: {}", note.message());
    }
}

/// Say, once per query and on stderr, when the memory budget in force cut the
/// plan short, and what statistics let it skip. Every sub-stream of a
/// partitioned replay carries the same
/// [`pgdump_query::TableStream::plan_notes`], settled before any of them runs,
/// so reading it off the first is reading the whole query's answer — unlike
/// [`announce_comparisons`], this needs no block to have resolved first. A
/// budget's note is followed by where its budget came from
/// ([`Resolved::plan_note_origin`]), which only this layer knows; a skip is
/// no fault, and is a `note:`.
fn announce_plan_notes(stream: &pgdump_query::TableStream<'_>, parallel: &Resolved) {
    let origin = parallel.plan_note_origin();
    for note in stream.plan_notes() {
        match note.kind {
            pgdump_query::PlanNoteKind::StatisticsPruned { .. } => {
                eprintln!("note: {}", note.message());
            }
            _ => eprintln!("warning: {}{origin}", note.message()),
        }
    }
}

/// Say, once the query has drained and only where one fired, what the early
/// stop on sorted data left unread. **Found while rows are read**, so unlike
/// [`announce_plan_notes`] it is summed over every sub-stream after the fact
/// ([`pgdump_query::TableStream::early_stops`]), a block split across
/// sub-streams counted once. A stop planned and never reached prints nothing:
/// the pruning note already says statistics were consulted.
fn announce_early_stops(streams: &[pgdump_query::TableStream<'_>]) {
    let mut unread = std::collections::BTreeMap::<u64, u64>::new();
    for stop in streams.iter().flat_map(|stream| stream.early_stops()) {
        if let Some(bytes) = stop.unread_bytes {
            *unread.entry(stop.header_offset).or_default() += bytes;
        }
    }
    if !unread.is_empty() {
        eprintln!(
            "note: reading stopped early in {} block(s) sorted past the filter's bound, so a \
             further {} byte(s) of rows are not read",
            unread.len(),
            unread.values().sum::<u64>()
        );
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
/// 0.
///
/// A run that found the cache already complete scanned nothing at all and
/// says so, rather than reporting a resume point equal to the file's size —
/// two different facts to a user checking whether a scan finished — unless it
/// re-read blocks for statistics they lacked (`backfilled`), which stderr
/// counts.
fn resume_notice(resumed_from: u64, size: u64, backfilled: usize) -> Option<String> {
    match resumed_from {
        0 => None,
        n if n >= size && backfilled > 0 => Some(format!(
            "the cache already covers all {size} byte(s); only blocks lacking the requested \
             statistics were re-read"
        )),
        n if n >= size => {
            Some(format!("nothing to scan: the cache already covers all {size} byte(s)"))
        }
        n => Some(format!("resumed a previous scan at byte {n} of {size}")),
    }
}

/// Catch `SIGINT` and `SIGTERM` for the duration of a scan, so an interrupted
/// `pgdq parse` saves what it has instead of throwing it away
/// (`docs/design/decisions.md`, "D63"). The guard is **cooperative**: the
/// signal sets a flag the mapping loop reads once per chunk, and the loop
/// persists the index it owns before returning. A **second** signal, of either
/// kind, exits immediately, so a save that wedges cannot hold the process.
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
/// [`render_field_into`], so output is byte-identical whether `--schema-mode`
/// is `typed` or `strings` (`docs/design/decisions.md`, "D66").
///
/// **One buffer for the whole batch.** The line is assembled in a `String`
/// that is cleared per row and keeps its capacity across the batch, so a
/// scalar field is written where it will be printed from.
///
/// `plans` is the stream's own [`pgdump_query::ResolvedSchema::plans`], which
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

/// Wire the library's `tracing` facade to stderr — on by default, uniformly,
/// for `parse`, `info` and `query` alike, with no terminal detection
/// (`docs/design/decisions.md`, "D64").
///
/// One level, `INFO`, and no way yet to raise or lower it — `-vvv` and
/// `--quiet` are deferred and unallocated. RFC3339 timestamps
/// (`UtcTime::rfc_3339`) match the koji orchestrator's logs. No ANSI color,
/// and stderr rather than stdout, which carries `query`'s row data.
fn init_status_output() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .init();
}

/// **A `current_thread` runtime, not a multi-threaded one.** Every unit of
/// work this binary dispatches is a `spawn_blocking` task
/// (`docs/design/decisions.md`, "D12"), so the blocking pool tokio creates on
/// demand carries the scan and the process's thread count follows the
/// concurrency dispatched rather than the CPUs it can see. **Claimed as a
/// thread-count result, not a memory one**: an arena's retention is not
/// proportional to how many there are, so this does not on its own make the
/// process smaller and no reading here says it does.
///
/// The `signal` handlers of [`install_interrupt_guard`] are ordinary
/// `tokio::spawn` tasks and run on this thread: the scan loop awaits a
/// `spawn_blocking` join at every piece, so the runtime is parked in
/// `block_on` — driving the signal driver — whenever work is running.
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // A default build drops this entirely; an `introspect` build writes what
    // the process held to the file `PGDQ_INTROSPECT_OUT` names, on the way
    // out (`src/introspect.rs`).
    let _instrument = introspect::at_exit();
    init_status_output();
    let cli = Cli::parse();
    match cli.command {
        Command::Parse {
            source: file,
            dqcache,
            preamble_only: preamble_only_flag,
            chunk_size,
            max_line_bytes,
            statistics,
            statistics_group_size,
            statistics_min_rows,
            parallel,
        } => {
            let read = ReadFlags { chunk_size, max_line_bytes };
            let statistics =
                statistics_request(statistics, statistics_group_size, statistics_min_rows)?;
            // `parse` scans to persist (`docs/design/decisions.md`, "D61").
            // Reject `--dqcache none` up front, before paying for a scan we
            // won't be allowed to persist.
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let path = mode
                .require_enabled("parse")
                .context("`--dqcache none` cannot be combined with `parse`")?
                .to_path_buf();
            // A cache written for another file is refused here as it is by
            // `info` and `query`, having read no more than the file's leading
            // bytes
            // (`docs/design/decisions.md`, "D20"). The flags and the limit
            // are announced ahead of it, neither waiting on the file (D64).
            let stated = parallel.discover();
            stated.announce();
            let source = open_for_scan(&file, &mode)?;
            let parallel = stated.resolve(source.as_ref());
            parallel.announce();
            if preamble_only_flag {
                let (metadata, diagnostics) =
                    preamble_only(source.as_ref(), &scan_options(read, &parallel), &mode).await?;
                print_metadata(&metadata, false);
                print_diagnostics(&diagnostics);
                println!();
                println!("wrote cache to {}", path.display());
                return Ok(());
            }
            let size = source.size().await?;
            let cancel = Arc::new(AtomicBool::new(false));
            let signalled = install_interrupt_guard(Arc::clone(&cancel))?;
            let scan_options =
                ScanOptions { cancel: Some(cancel), ..scan_options(read, &parallel) };
            let run =
                pgdump_query::map_file(source.as_ref(), &scan_options, &mode, &statistics).await?;
            introspect::statistics_returned(&run.statistics);
            if run.interrupted {
                // No listing: `pgdq info` is the command that reports. Both
                // lines go to stderr, so a caller redirecting stdout gets an
                // empty report rather than a truncated one.
                // A stop inside the statistics back-fill leaves the map
                // whole, and only the same statistics flags pick it up again.
                if run.index.is_complete(size) {
                    eprintln!(
                        "interrupted after re-reading {} of the {} block(s) lacking the requested \
                         statistics — the cache at {} holds those re-read so far",
                        run.backfilled,
                        run.lacking_statistics,
                        path.display()
                    );
                    eprintln!(
                        "re-run `pgdq parse --source {}` with the same statistics flags to continue",
                        file.display()
                    );
                } else {
                    eprintln!(
                        "interrupted at byte {} of {size} — the cache at {} holds the scan so far",
                        run.index.scanned_through,
                        path.display()
                    );
                    eprintln!("re-run `pgdq parse --source {}` to continue", file.display());
                }
                // Exit by signal (130/143), so a script can tell an
                // interrupt from a failure; `SIGINT` is the fallback.
                // `std::process::exit` runs no destructors, so the instrument
                // is asked here rather than left to `main`'s guard.
                introspect::report();
                let number = signalled.load(Ordering::SeqCst);
                std::process::exit(128 + if number == 0 { 2 } else { number });
            }
            // The listing describes the file's state after this run, not
            // this invocation's diff, so the one line that *is* about the
            // invocation goes above it.
            if let Some(notice) = resume_notice(run.resumed_from, size, run.backfilled) {
                println!("{notice}");
                println!();
            }
            // `map_file` reached EOF, so its censuses cover the whole file.
            // `--detail` is off, so the container line is not printed and
            // nothing has to be read for it.
            print_index(&run.index, None, false, false, true);
            println!();
            println!("wrote cache to {}", path.display());
        }
        Command::Info { source: file, dqcache, detail, map, json } => {
            if json && (detail || map) {
                anyhow::bail!(
                    "--json already carries everything --detail/--map would add — drop one of them"
                );
            }
            let Some(file) = file else {
                // Cache-only mode (`docs/design/decisions.md`, "D22"): no
                // live dump file at all, so clap already required
                // `--dqcache` for us.
                let path = dqcache.expect("clap requires --dqcache when --source is omitted");
                return info_offline(&path, detail, map, json).await;
            };
            // `info` never scans, so `--dqcache none` — "ignore the cache" —
            // leaves nothing to answer from, and the message names the way
            // out. Formed here rather than in `Error::CacheDisabled` because
            // it interpolates the user's own `--source` path, which the
            // library error does not have.
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
            // A cache that does not describe this file leaves `info` nothing
            // to report from, and it says so having read no more than the
            // file's leading bytes. This
            // condition keeps `info`'s own sentence, which names the two ways
            // out ahead of the command they enable.
            let source = match open_with_cache(&file, &mode)? {
                Opened::Source(source) => source,
                Opened::SourceChanged { cached_stored_size, live_stored_size } => {
                    let changed =
                        CacheStatus::SourceChanged { cached_stored_size, live_stored_size };
                    anyhow::bail!(unusable_cache_message(&changed, &path, Some(&file)))
                }
            };
            let status = pgdump_query::cache::load(&path, source.as_ref()).await?;
            let (mut index, mtime_changed, total_size, compression, envelope) = match status {
                CacheStatus::Valid { index, mtime_changed, total_size, compression, envelope }
                | CacheStatus::Incomplete {
                    index,
                    mtime_changed,
                    total_size,
                    compression,
                    envelope,
                } => (index, mtime_changed, total_size, compression, envelope),
                unusable => anyhow::bail!(unusable_cache_message(&unusable, &path, Some(&file))),
            };
            if mtime_changed {
                index.diagnostics.push(Diagnostic::cache_mtime_changed());
            }
            report(&index, total_size, compression, &envelope, detail, map, json)?;
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
            chunk_size,
            max_line_bytes,
            statistics,
            parallel,
        } => {
            let read = ReadFlags { chunk_size, max_line_bytes };
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
                // terms, flattened rather than nested.
                Some(spec) => pgdump_query::Expr::And(
                    std::iter::once(where_expr::parse_where(&spec)?)
                        .chain(terms.into_iter().map(pgdump_query::Expr::Term))
                        .collect(),
                ),
            };
            // As `info`: a cache that does not describe this file is
            // reported having read no more than the file's leading bytes.
            // Announced in two lines, the first
            // ahead of the open, as `parse` does.
            let stated = parallel.discover();
            stated.announce();
            let source = open_for_scan(&file, &mode)?;
            let parallel = stated.resolve(source.as_ref());
            parallel.announce();
            let mut header_printed = false;
            let mut any_batch = false;
            let mut rows = 0u64;
            let query_options = QueryOptions {
                database,
                schema_mode: schema_mode.into(),
                filter,
                projection: projection(column, no_columns),
                // The same flags on both passes: one mapping scan and one
                // replay over one source.
                parallelism: parallel.parallelism(),
                use_statistics: statistics == QueryStatistics::All,
                ..QueryOptions::default()
            };
            // Pull mode, not `read_table`: rendering a nested column back to
            // its literal needs the stream's `NestedPlan`s, and push mode
            // hands the resolved schema back only once the whole stream has
            // drained (`docs/design/decisions.md`, "D46").
            //
            // Partitioned, not serial: `Parallelism::Serial` — `--jobs 1` —
            // is one sub-stream, so the serial path is reached through the
            // same call rather than branched to (D51). What `table_stream`
            // reports as its stream's first item, this reports from the
            // `await`.
            let mut streams = pgdump_query::table_stream_partitions(
                source.as_ref(),
                &table,
                scan_options(read, &parallel),
                query_options,
                mode,
            )
            .await
            .map_err(name_taken_verbatim)?;
            // Known from the plan alone, before any block is read — unlike
            // `announce_comparisons` below, which waits on the first
            // resolved schema.
            if let Some(first) = streams.first() {
                announce_plan_notes(first, &parallel);
            }
            let mut announced = false;
            let mut slots: Vec<Slot> = streams.iter().map(|_| Slot::Empty).collect();
            // The lowest-indexed sub-stream that has failed, and its error.
            // **Recorded rather than raised**: sub-stream `k` reads a
            // contiguous run of blocks after `k-1`'s, so raising whichever
            // failed first in time would name a different row on each run
            // over an unchanged file (`docs/design/decisions.md`, "D52").
            let mut failed: Option<(usize, pgdump_query::Error)> = None;
            loop {
                // Everything at or after a failing sub-stream is dead, and
                // **discarding what those slots hold is the load-bearing
                // half**: a batch left in a slot past the failure would print
                // the moment the ones before it drained — rows past the error
                // a serial replay never reached.
                let live = failed.as_ref().map_or(slots.len(), |(index, _)| *index);
                for slot in slots.iter_mut().skip(live) {
                    *slot = Slot::Done;
                }
                // Refill every empty slot at once: every sub-stream in the
                // first round, and after it only the one just drained — which
                // is what makes the merge's bound N x batch rather than a
                // reorder buffer.
                let round = {
                    let fills = streams
                        .iter_mut()
                        .zip(slots.iter_mut())
                        .enumerate()
                        .filter(|(index, (_, slot))| *index < live && matches!(slot, Slot::Empty))
                        .map(|(index, (stream, slot))| async move {
                            match stream.next().await {
                                Some(Ok(batch)) => {
                                    *slot = Slot::Held {
                                        offset: stream.batch_source_offset(),
                                        // Taken now: by the time this batch
                                        // is printed its sub-stream may have
                                        // moved to a block whose header
                                        // named other columns.
                                        plans: stream.resolved_schema().plans,
                                        batch,
                                    };
                                    None
                                }
                                Some(Err(err)) => {
                                    *slot = Slot::Done;
                                    Some((index, err))
                                }
                                None => {
                                    *slot = Slot::Done;
                                    None
                                }
                            }
                        })
                        .collect::<Vec<_>>();
                    // `join_all` answers in argument order, which is
                    // partition order, which is file order — so the first
                    // failure in it is the lowest-indexed of this round's.
                    futures::future::join_all(fills).await.into_iter().flatten().next()
                };
                // Round again rather than printing: the loop head marks the
                // sub-streams this failure killed dead before the merge next
                // picks. It terminates because a recorded failure strictly
                // lowers `live`.
                if let Some(first) = round {
                    failed = Some(first);
                    continue;
                }
                // The k-way merge itself: the held batch that begins earliest
                // in the file is the next one to print.
                let Some(next) = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(index, slot)| match slot {
                        Slot::Held { offset, .. } => Some((*offset, index)),
                        _ => None,
                    })
                    .min()
                    .map(|(_, index)| index)
                else {
                    break;
                };
                let Slot::Held { batch, plans, .. } =
                    std::mem::replace(&mut slots[next], Slot::Empty)
                else {
                    unreachable!("the slot the merge picked is the one it just read")
                };
                if !announced {
                    // Off the first sub-stream, not the one this batch came
                    // from: its first segment starts at a `COPY` header, so a
                    // schema is resolved whether or not it had rows.
                    announce_comparisons(&streams[0]);
                    announced = true;
                }
                any_batch = true;
                // A zero-column projection prints no header: it would be an
                // empty line, and `--no-columns | wc -l` would come back one
                // too many (`docs/design/decisions.md`, "D28").
                if !header_printed && batch.num_columns() > 0 {
                    let names: Vec<String> =
                        batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                    println!("{}", names.join("\t"));
                    header_printed = true;
                }
                print_batch(&batch, &plans)?;
                rows += batch.num_rows() as u64;
            }
            // Every sub-stream before the failing one is drained, so what is
            // held now is the earliest error in the file. Raised after the
            // rows before it have printed.
            if let Some((_, err)) = failed {
                return Err(name_taken_verbatim(err));
            }
            // A query that matched a block but selected no rows still
            // resolved a schema, so the announcement is owed either way.
            if !announced {
                announce_comparisons(&streams[0]);
            }
            announce_early_stops(&streams);
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

/// What [`open_with_cache`] found: the source to read, or the one refusal the
/// cache path settles on its own, before anything is opened. **The second
/// variant is not a refusal this helper can write**: a contradicted
/// compression claim reaches all three commands in one sentence, a stored-size
/// mismatch does not — `parse` and `query` surface
/// `Error::CacheSourceMismatch` and `info` prints the sentence naming the two
/// ways out (`docs/design/decisions.md`, "D20").
enum Opened {
    /// The source, ready to read.
    Source(Arc<dyn pgdump_query::ByteRangeSource>),
    /// The cache at this mode's path records a stored size the file does not
    /// have, so it describes another file — the condition, and the two
    /// numbers, `cache::load` would answer [`CacheStatus::SourceChanged`]
    /// with once a source existed.
    SourceChanged { cached_stored_size: u64, live_stored_size: u64 },
}

/// Open `file`, handing recognition whatever the cache at `cache` says about
/// its compression layer, so an `.xz` source is built from the seek table a
/// previous walk already produced instead of re-walking the file's stream
/// footers (`docs/design/decisions.md`, "D18"). `--dqcache none` claims
/// nothing; cache-only mode never reaches here at all, having no live source.
///
/// Both "written for another file" conditions stop here rather than at each of
/// the three call sites — a contradicted claim as a bail, a stored-size
/// mismatch as an [`Opened`] variant — so an `.xz` source never walks its
/// stream footers to reach a refusal the cache path alone settles. The
/// comparison itself stays in `cache::claim` (D20).
fn open_with_cache(file: &Path, cache: &CacheMode) -> Result<Opened> {
    let claimed_by = match cache {
        CacheMode::Enabled(path) => Some(path.as_path()),
        CacheMode::Disabled | CacheMode::Offline(_) => None,
    };
    let known = match claimed_by {
        Some(path) => match pgdump_query::cache::claim(path, file)? {
            CacheClaim::Compression(known) => known,
            CacheClaim::SourceChanged { cached_stored_size, live_stored_size } => {
                return Ok(Opened::SourceChanged { cached_stored_size, live_stored_size });
            }
        },
        None => KnownCompression::Unknown,
    };
    match open_local(file, known)? {
        Recognized::Source(source) => Ok(Opened::Source(source)),
        Recognized::Mismatch => {
            let path =
                claimed_by.expect("`KnownCompression::Unknown` claims nothing to contradict");
            anyhow::bail!(cache_written_for_another_file(path, file))
        }
    }
}

/// [`open_with_cache`] for the two commands that scan, raising
/// `Error::CacheSourceMismatch` from `CacheMode::source_mismatch` — the same
/// constructor the three scan entry points use, so this refusal is
/// word-for-word the one it pre-empts. The library still refuses on its own:
/// this spares the walk, it does not replace the guarantee
/// (`docs/design/decisions.md`, "D20").
fn open_for_scan(file: &Path, cache: &CacheMode) -> Result<Arc<dyn pgdump_query::ByteRangeSource>> {
    match open_with_cache(file, cache)? {
        Opened::Source(source) => Ok(source),
        Opened::SourceChanged { cached_stored_size, live_stored_size } => {
            Err(cache.source_mismatch(cached_stored_size, live_stored_size).into())
        }
    }
}

/// The sentence all three commands print when recognition finds that the
/// cache at `path` records compression details the file at `source`
/// contradicts.
///
/// Not one of [`unusable_cache_message`]'s: that function matches on
/// [`CacheStatus`], and recognition catches this before a source exists so
/// `load` never sees it. The sentence is its own rather than `Unreadable`'s —
/// "check the path, or run `pgdq parse`" is advice `parse` cannot take, being
/// the command that just refused. The tail is [`TWO_WAYS_OUT`]
/// (`docs/design/decisions.md`, "D20").
fn cache_written_for_another_file(path: &Path, source: &Path) -> String {
    format!(
        "the cache at {} records compression details that {} contradicts, so it was written for \
         another file{TWO_WAYS_OUT}",
        path.display(),
        source.display()
    )
}

/// What a caller does about a cache that describes a different file, in the
/// words both refusals use. There is no third way — no `--force`, no
/// `CacheMode` variant meaning "replace regardless"
/// (`docs/design/decisions.md`, "D20").
/// `pgdump_query::Error::CacheSourceMismatch` carries the same clause for the
/// size-mismatch condition; `refusals_name_both_ways_out`
/// (`tests/partial_reporting.rs`) holds the size-mismatch refusals to one
/// wording, and `every_command_refuses_a_cache_that_does_not_describe_the_file`
/// (`tests/xz_source.rs`) a contradicted compression claim's.
const TWO_WAYS_OUT: &str = " — remove it, or name a different cache path";

/// The sentence `pgdq info` prints for a cache it cannot use — four causes,
/// four sentences, because the fact the user needs to know differs. `parse`
/// scans over `Missing`, `Unreadable` and `UnsupportedVersion` and refuses
/// `SourceChanged`, so that arm names [`TWO_WAYS_OUT`] before it names the
/// command (`docs/design/decisions.md`, "D20").
///
/// **One match, two renderings**, the discipline [`resolution_words`] applies.
/// `source` is `None` in cache-only mode, which states the fault and stops; it
/// cannot reach [`CacheStatus::SourceChanged`] at all, there being no live
/// file to compare against — what its `CacheOffline` diagnostic warns about.
///
/// Takes the whole [`CacheStatus`] rather than a narrowed type so the match
/// stays exhaustive: a usable status reaching here is a caller bug.
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
        CacheStatus::SourceChanged {
            cached_stored_size: cached_size,
            live_stored_size: live_size,
        } => {
            let source = source.expect("cache-only mode has no live source to compare against");
            // The one arm whose remedy is not `pgdq parse` on its own:
            // `parse` refuses this very condition, so the two ways out come
            // first and `parse` then works
            // (`docs/design/decisions.md`, "D20").
            format!(
                "{} has changed since it was parsed ({live_size} bytes now, {cached_size} when \
                 the cache at {} was written), so every offset in the cache could be \
                 wrong{TWO_WAYS_OUT}, then run `pgdq parse --source {}`",
                source.display(),
                path.display(),
                source.display()
            )
        }
        CacheStatus::Valid { .. } | CacheStatus::Incomplete { .. } => {
            unreachable!("a usable cache is reported, not refused")
        }
    }
}

/// `pgdq info` with no `--source`: answer strictly from the cache at `path`
/// (`docs/design/decisions.md`, "D22"). An `Incomplete` cache is
/// reported like any other, with its coverage stated — cache-only mode has no
/// scan to extend it with, but "as far as the scan got" is still an answer.
async fn info_offline(path: &Path, detail: bool, map: bool, json: bool) -> Result<()> {
    let mode = CacheMode::Offline(path.to_path_buf());
    let (index, total_size, compression, envelope) = match mode.load_offline().await? {
        CacheStatus::Valid { index, total_size, compression, envelope, .. }
        | CacheStatus::Incomplete { index, total_size, compression, envelope, .. } => {
            (index, total_size, compression, envelope)
        }
        unusable => anyhow::bail!(unusable_cache_message(&unusable, path, None)),
    };
    report(&index, total_size, compression, &envelope, detail, map, json)
}

/// One column's resolution outcome, in both spellings: a stable token for
/// `--json` and the sentence `info --detail` prints
/// (`docs/design/decisions.md`, "The CLI"). **One match, two renderings**, so
/// a new [`ColumnResolution`] variant is a compile error that has to answer
/// both.
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

/// The sentence half of [`resolution_words`] — `info --detail`'s per-column
/// line.
fn resolution_label(r: &ColumnResolution) -> &'static str {
    resolution_words(r).1
}

/// What one column became in Arrow — the other half of `info --detail`'s
/// per-column line (`docs/design/decisions.md`, "The CLI").
///
/// Arrow's own `Display` is what prints, with one substitution: the five-field
/// range struct, identical for every range column in every dump, collapses to
/// `Range<T>`, `T` being the bound type. The manual states the struct's real
/// layout once, which makes the elision lossless.
///
/// **The substitution is detected from the [`NestedPlan`], never from the
/// field names** — a user composite may declare five fields with exactly
/// those names, and `pgtype::RANGE_STRUCT_FIELDS` reserves dispatch to the
/// plan. A built-in multirange and an array of the matching range render
/// *identically* (`List(Range<Int32>)`): the same Arrow type, different plans,
/// with the declared PostgreSQL type on the same line.
///
/// The type and the plan come from one producer and cannot disagree; this
/// being display code, a disagreeing pair falls back to plain `Display`
/// rather than panicking.
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
/// every other column — read off the column's own [`ComparisonPlan`], where
/// resolution already put them (`docs/design/decisions.md`, "The CLI").
///
/// A domain over an enum answers here too, `pgtype::comparison_user_type`
/// recursing through the domain chain. An *empty* enum is
/// `ComparisonPlan::Refused` and has nothing to list, matching the
/// `empty enum` sentence the line above prints.
fn enum_labels(plan: &ComparisonPlan) -> Option<&[String]> {
    match plan {
        ComparisonPlan::Compared { kind: CompareKind::Enum(labels), .. } => Some(labels),
        _ => None,
    }
}

/// The labels as `info --detail` prints them: each one single-quoted with any
/// interior quote doubled, comma-separated.
///
/// **Quoting is forced by the data**: a label is arbitrary text (`has space`,
/// `has,comma`, `has'quote` are all legal and all in the fixtures), so a bare
/// comma-joined list cannot be read back apart. Single quotes with `''`
/// doubling is both what the dump's own `CREATE TYPE … AS ENUM (…)` writes
/// and what a `--filter` value accepts ([`dequote`]).
///
/// *Rejected:* Rust's `{:?}`, which `arrow_type_label` uses for a composite's
/// field names; it spells a PostgreSQL literal in Rust's escape vocabulary,
/// and the double quote it produces is the *other* quote here.
fn label_list(labels: &[String]) -> String {
    labels.iter().map(|l| format!("'{}'", l.replace('\'', "''"))).collect::<Vec<_>>().join(", ")
}

/// How a database is named in the listing. A `\connect`-less dump has no name
/// to print, and `(unnamed)` is what the listing calls it — one spelling, so
/// the metadata header, the block listing and `--map` cannot disagree.
fn database_label(database: &Option<String>) -> &str {
    match database {
        Some(name) => name,
        None => "(unnamed)",
    }
}

/// Prints a `database: <name>` line each time the database changes, and only
/// when a listing spans more than one.
///
/// **The rows are already in file order and every `\connect` segment is
/// contiguous in the file**, so a header whenever the value changes is the
/// whole grouping rule. It is also what makes an `AmbiguousTable` error's
/// candidate names ones this listing already showed
/// (`docs/design/decisions.md`, "D49").
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
/// user-defined-type counts (`docs/design/decisions.md`, "The CLI"). The
/// `database: <name>` line is shown only when it is informative — a single
/// unnamed database prints no header line.
///
/// Under `detail`, the `user-defined types` count becomes the heading of a
/// listing of the types themselves, one line each, in the order the dump
/// declares them. The count is otherwise their only trace.
fn print_metadata(metadata: &DumpMetadata, detail: bool) {
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
        if detail {
            // Padded to the widest name this database declares, so the kinds
            // line up; the right edge stays ragged.
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
/// (`docs/design/decisions.md`, "The CLI").
///
/// **Every arm renders**, not the enum alone. `Composite { fields: None }`
/// says `(fields not parsed)` explicitly, being the one arm whose absence
/// changes how a column of the type resolves; a `Range` naming no subtype
/// says so for symmetry. Neither shape is one `pg_dump` writes, so both are
/// pinned by this module's unit test. [`type_kind_label`] is `--map`'s
/// one-word name for the same vocabulary.
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
        // The `canonical` function is named where the DDL declares one: it
        // is why a column of this type refuses every filter operator.
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
/// from — the single resolution pass `--detail`'s text and `--json`'s export
/// both render (`docs/design/decisions.md`, "The CLI").
///
/// `complete` says whether `index` covers the file
/// ([`DumpIndex::is_complete`]), which decides whether a block's array-shape
/// census may be believed: a *mapped* block's census is total for that block,
/// but one table's data can occupy several blocks (I2), so a map that stopped
/// short cannot speak for a block past its frontier and every column resolves
/// optimistically until it can (`docs/design/decisions.md`, "D35").
///
/// A header-less block resolves to an empty schema, its column names coming
/// from its first data row, which no index records. It is still listed, so
/// the export's shape does not vary per block.
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

/// `--json`'s shape: the whole cache file — the [`DumpIndex`] and the
/// [`CacheEnvelope`] around it — flattened to one object, plus what the file
/// does not itself carry: the container's shape derived from its seek table,
/// the diagnostics `#[serde(skip)]` drops (`docs/design/decisions.md`, "D22"),
/// and the per-block type resolution, an L2 conclusion an L1 index has no
/// field for. No schema stability is promised — see the `--json` flag's help
/// text.
///
/// **Coverage is components, not a rendered percentage**: `scanned_through`
/// and `total_size` sit side by side, so a script computes its own ratio.
///
/// **Every block's per-group statistics are exported as the cache holds them**,
/// with no per-table rollup, and the document is written compact and straight
/// to stdout, never held whole (`docs/design/decisions.md`, "D67").
#[derive(serde::Serialize)]
struct IndexJson<'a> {
    #[serde(flatten)]
    envelope: &'a CacheEnvelope,
    #[serde(flatten)]
    index: &'a DumpIndex,
    total_size: u64,
    /// The container's shape, `null` for a plain file — the same three
    /// numbers [`compression_line`] prints, exported because `--json` carries
    /// everything `--detail` would add.
    compression: Option<CompressionShape>,
    diagnostics: &'a [Diagnostic],
    resolution: Vec<BlockResolutionJson<'a>>,
}

/// One `COPY` block's resolution, keyed by the block rather than rolled up per
/// table: a table can span blocks (I2) and a header-less block names its
/// columns from its first row, so a per-table rollup needs a merge rule that
/// does not exist (`docs/design/decisions.md`, "D67").
#[derive(serde::Serialize)]
struct BlockResolutionJson<'a> {
    database: Option<&'a str>,
    table: String,
    header_offset: u64,
    columns: Vec<ColumnResolutionJson<'a>>,
}

/// One column's resolution: what the DDL declared, what it became, and why.
/// `arrow_type` is the exact string `info --detail` prints for the same
/// column, so the two renderings cannot disagree about the type either.
#[derive(serde::Serialize)]
struct ColumnResolutionJson<'a> {
    name: &'a str,
    declared: Option<&'a str>,
    outcome: &'static str,
    arrow_type: String,
    plan: &'a NestedPlan,
}

fn print_index_json(
    index: &DumpIndex,
    total_size: u64,
    compression: Option<CompressionShape>,
    envelope: &CacheEnvelope,
    complete: bool,
) -> Result<()> {
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
    let wrapped = IndexJson {
        envelope,
        index,
        total_size,
        compression,
        diagnostics: &index.diagnostics,
        resolution,
    };
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    serde_json::to_writer(&mut out, &wrapped).context("writing the JSON export")?;
    writeln!(out).and_then(|()| out.flush()).context("writing the JSON export")
}

/// How much of the file the index covers, stated **once, at the top**, with
/// nothing below it qualified (`docs/design/decisions.md`, "D67").
///
/// A partial index lacks *records*, not confidence: a block enters the map
/// only at a `CopyEnd` watermark and every mapping pass censuses, so every
/// record it holds is complete in itself, and there is no half-known block.
/// The percentage floors, so it reads 100% only for a finished scan.
fn completion_line(scanned_through: u64, total_size: u64) -> String {
    // A zero-byte file is trivially covered in full, and has no ratio.
    let percent = (scanned_through.min(total_size) * 100).checked_div(total_size).unwrap_or(100);
    format!("Scan completion: {percent}% ({scanned_through} bytes)")
}

/// Every `pgdq info` rendering goes through here: the coverage line, then the
/// listing or the export.
fn report(
    index: &DumpIndex,
    total_size: u64,
    compression: Option<CompressionShape>,
    envelope: &CacheEnvelope,
    detail: bool,
    map: bool,
    json: bool,
) -> Result<()> {
    let complete = index.is_complete(total_size);
    if json {
        return print_index_json(index, total_size, compression, envelope, complete);
    }
    println!("{}", completion_line(index.scanned_through, total_size));
    println!();
    print_index(index, compression, detail, map, complete);
    Ok(())
}

/// The container line `info --detail` prints above the listing, and
/// nothing at all for a plain file, which has no container to describe.
///
/// **Three numbers a user is otherwise sent to `xz --list` for**, which on a
/// many-stream file walks every footer. `largest block` is the largest term of
/// what `--parallel-memory` has to clear for a query to read this file a block
/// at a time — **four times over**, one reader's block beside the further
/// blocks the pool keeps however few readers run, plus a read buffer and the
/// decompressor's own working memory (`docs/design/decisions.md`, "D16").
fn compression_line(shape: &CompressionShape) -> String {
    format!(
        "compression: {} — {} block(s) in {} stream(s), largest block {} bytes uncompressed",
        shape.container, shape.blocks, shape.streams, shape.max_block_uncompressed
    )
}

/// Print the whole listing, below whatever coverage line [`report`] already
/// stated. `complete` is passed straight through to [`block_resolutions`],
/// which is where it means something, and **nothing here is qualified by how
/// much of the file was scanned** — the coverage line says it once
/// (see [`completion_line`]).
fn print_index(
    index: &DumpIndex,
    compression: Option<CompressionShape>,
    detail: bool,
    map: bool,
    complete: bool,
) {
    if let Some(metadata) = &index.metadata {
        print_metadata(metadata, detail);
        println!();
    }

    // Above the diagnostics rather than below them, because the shape is what
    // the compression warning beneath it is *about*.
    if let Some(shape) = compression.filter(|_| detail) {
        println!("{}", compression_line(&shape));
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
            if detail {
                // One line per column that has something to say: a column
                // that did not map says why, one that mapped says what it
                // mapped *to* unless that is `Utf8View` — the no-information
                // answer, and the only Arrow type a non-`Mapped` resolution
                // produces, so the two arms never both fire.
                //
                // An enum column carries its declared labels on a
                // continuation line beneath, uncapped: this is the only place
                // a user can read them without grepping the dump for its
                // `CREATE TYPE`, and a continuation rather than a suffix
                // keeps a long list from breaking the alignment.
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
        if detail {
            println!("    header offset: {}", block.header_offset);
            println!("    data offset:   {}", block.data_offset);
            println!("    terminator:    {}", block.terminator_offset);
            println!("    end offset:    {}", block.end_offset);
        }
    }

    if detail {
        println!();
        print_statistics(index);
    }

    println!();
    // No byte count here: the coverage line above owns that.
    println!("{} COPY block(s), {} row(s)", blocks.len(), index.total_rows());
    if total_unmapped > 0 {
        println!(
            "{total_unmapped} of {total_columns} columns unmapped — run with --detail for details"
        );
    }
}

/// `--detail`'s statistics section: a line per table and one beneath it per
/// column, or one line saying no block carries any
/// ([`info_statistics::table_statistics`]).
fn print_statistics(index: &DumpIndex) {
    let tables = info_statistics::table_statistics(index);
    if tables.iter().all(|t| t.gathered_blocks == 0) {
        println!("statistics: none gathered");
        return;
    }
    println!("statistics:");
    let mut headings = DatabaseHeadings::new(tables.iter().map(|t| t.database));
    for table in &tables {
        headings.before(table.database);
        println!("    {}", table.line());
        if table.gathered_blocks > 0 {
            for column in &table.columns {
                println!("        {}", column.line(table.blocks));
            }
        }
    }
}

/// `DumpIndex::diagnostics` (or, for `--preamble-only`, the diagnostics
/// `preamble_only` reports separately), printed unconditionally with no
/// severity threshold. Cache-only mode's "unverified, historical" banner rides
/// this same path (`DiagnosticKind::CacheOffline`). Returns whether anything
/// was printed.
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
        DiagnosticKind::NonSeekableCompressedSource { block_count } => format!(
            "this .xz source has no seek structure ({block_count} block(s), one stream) — every read decodes the file from byte 0; recompress with `xz -T0` or `--block-size=<size>` for random access"
        ),
    }
}

/// Referenced-role and referenced-tablespace summary
/// (`docs/design/decisions.md`, "D31") — an empty set prints nothing. Returns
/// whether anything was printed, so the caller knows whether to add a
/// separating blank line.
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
/// vocabulary the TOC-coverage diagnostic counts against
/// (`docs/design/decisions.md`, "D31"). Counts `toc_owned` spans, not every
/// attributed one: this is an object *census*, and a follow-on statement
/// (`ALTER ... OWNER TO`, etc.) inherits its governing entry's `toc`, so
/// counting `span.toc.is_some()` would count that object twice. A span with
/// no TOC comment at all contributes to no bucket. Returns whether anything
/// was printed.
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
/// filters it down to (`docs/design/decisions.md`, "D34"). Grouped by
/// [`DatabaseHeadings`], as the ordinary block listing is.
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

/// Short label for a [`TypeKind`] — `--map`'s compact form of the vocabulary
/// `docs/manual/type-handling.md` explains. [`type_kind_summary`] is the
/// `--detail` listing's fuller rendering, payload included.
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

    /// A source recommending whatever it is built with, so that
    /// [`ParallelArgs::resolve`]'s two halves — ask the source, or honour the
    /// flag — can be told apart without a real file of either shape. The three
    /// required methods answer nothing; `resolve` touches no byte.
    struct Recommends {
        jobs: usize,
        memory: Option<pgdump_query::WorkerMemory>,
    }

    impl Recommends {
        /// A source that recommends a worker count and no cost of its own —
        /// the plain file's shape, which asks for no budget.
        fn jobs(jobs: usize) -> Self {
            Self { jobs, memory: None }
        }

        /// A source that recommends both, which is what a compressed one does:
        /// a count, and what **one** of those workers holds.
        fn reader(jobs: usize, per_worker: u64) -> Self {
            Self { jobs, memory: Some(pgdump_query::WorkerMemory::per_worker(per_worker)) }
        }

        /// A block-decoding source's shape: a per-worker charge, and the
        /// retention list the pool shares over a `depth`-slot pool.
        fn block_reader(jobs: usize, per_worker: u64, unit: u64, depth: usize) -> Self {
            Self {
                jobs,
                memory: Some(
                    pgdump_query::WorkerMemory::per_worker(per_worker).pooling(unit, depth),
                ),
            }
        }
    }

    impl ByteRangeSource for Recommends {
        fn read_range(
            &self,
            _offset: u64,
            _len: usize,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = pgdump_query::Result<bytes::Bytes>> + Send + '_>,
        > {
            Box::pin(async { Ok(bytes::Bytes::new()) })
        }
        fn size(
            &self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = pgdump_query::Result<u64>> + Send + '_>,
        > {
            Box::pin(async { Ok(0) })
        }
        fn modified(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = pgdump_query::Result<Option<std::time::SystemTime>>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(None) })
        }
        fn default_workers(&self) -> usize {
            self.jobs
        }
        fn default_worker_memory(&self) -> Option<pgdump_query::WorkerMemory> {
            self.memory
        }
    }

    /// One of the committed runtime roots
    /// (`pgdump_query-cli/tests/data/runtime/README.md`) — a filesystem tree
    /// shaped like a Linux one, so a resolution can be pinned against an
    /// environment this machine is not in.
    fn runtime_root(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/runtime").join(name)
    }

    /// One reader's charge in the flat shape every source but the
    /// block-decoding one states; that shape is `BLOCK_READER` beside its
    /// unit.
    const READER: u64 = 58 << 20;

    /// **A discovered limit is what a flagless run resolves inside, on both
    /// cgroup versions** — the v1 arm being the one no machine here can
    /// produce (`RT4`, `RT6`), reached through `mountinfo`. The pair is
    /// consistent in both: the count is what the allowance affords under the
    /// margin and the budget is what that many readers spend, never the cap
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits").
    #[test]
    fn a_flagless_run_resolves_inside_a_discovered_limit_on_either_cgroup_version() {
        let flagless = ParallelArgs { jobs: None, parallel_memory: None };

        // v2, a 1 GiB `memory.max`: 640 MiB after the reserve, and 563.2 MiB
        // once the margin is left as well — nine readers of 58 MiB.
        let v2 = flagless.resolve_in(&runtime_root("v2-limit"), &Recommends::reader(24, READER));
        assert_eq!(v2.parallelism().jobs(), 9);
        assert_eq!(v2.parallelism().memory_bytes(), Some(9 * READER));
        assert_eq!(v2.limit.as_ref().map(|l| l.bytes), Some(1 << 30));

        // v1, a 512 MiB `memory.limit_in_bytes`: 128 MiB after the reserve,
        // and the margin is inert below the 640 MiB crossover — it allows
        // 153.6, which is more than the cap — so the cap is what two readers
        // of 58 MiB are fitted inside.
        let v1 = flagless.resolve_in(&runtime_root("v1-limit"), &Recommends::reader(24, READER));
        assert_eq!(v1.parallelism().jobs(), 2);
        assert_eq!(v1.parallelism().memory_bytes(), Some(2 * READER));
        assert_eq!(
            v1.limit.as_ref().map(|l| l.read_from.clone()),
            Some(runtime_root("v1-limit").join("sys/fs/cgroup/memory/svc/memory.limit_in_bytes")),
            "the v1 mount is located through mountinfo, not assumed"
        );

        // A plain source recommends nothing and is left where it was, capped
        // by the same limit.
        let plain = flagless.resolve_in(&runtime_root("v2-limit"), &Recommends::jobs(1));
        assert!(plain.parallelism().is_serial());
        assert_eq!(plain.parallelism().memory_bytes(), Some(pgdump_query::DEFAULT_MEMORY_BUDGET));
    }

    /// **A shared pool lowers the recommended count at every allocation.** A
    /// block-decoding source's pool retains a unit for every slot but the one
    /// a reader is filling — `POOL_DEPTH - 1` below four readers and
    /// `jobs - 1` above — so the allowance buys fewer than a division by the
    /// per-reader charge would say. Pinned at both ends: billing no pool would
    /// bind nowhere, billing two units a reader would over-bill.
    #[test]
    fn a_shared_pool_lowers_a_recommended_count_at_every_allocation() {
        let flagless = ParallelArgs { jobs: None, parallel_memory: None };
        const UNIT: u64 = 24 << 20;
        // What one reader of a 24 MiB-block dump holds: the block it is
        // decoding and then retains, the chunk buffer a straddling read is
        // assembled into, and the decoder.
        const BLOCK_READER: u64 = 34 << 20;
        let source = || Recommends::block_reader(24, BLOCK_READER, UNIT, 4);

        // v1, a 512 MiB limit: 128 MiB after the reserve, which a division by
        // the per-reader charge calls three readers. The three units the pool
        // retains beside a single reader leave too little for a second, so one
        // is the honest answer.
        let tight = flagless.resolve_in(&runtime_root("v1-limit"), &source());
        assert_eq!(tight.parallelism().jobs(), 1);
        assert_eq!(tight.parallelism().memory_bytes(), Some(BLOCK_READER + 3 * UNIT));

        // v2, a 1 GiB limit, once the reserve and the margin are both left. A
        // division says sixteen readers; the pool slot each of them past the
        // first also takes is what makes it ten.
        let roomy = flagless.resolve_in(&runtime_root("v2-limit"), &source());
        assert_eq!(roomy.parallelism().jobs(), 10);
        assert_eq!(roomy.parallelism().memory_bytes(), Some(10 * BLOCK_READER + 9 * UNIT));
    }

    /// **No limit found leaves the source's recommendation standing**, capped
    /// only by half of `MemAvailable` (`RT8`) — a `min` against the fallback
    /// constant would make a flagless compressed scan serial on the machine
    /// most likely to run it.
    #[test]
    fn no_limit_found_takes_the_sources_own_answer_under_the_memavailable_cap() {
        let flagless = ParallelArgs { jobs: None, parallel_memory: None };

        // Enough available that half of it does not bind, so all twenty-four
        // readers stand.
        let roomy = flagless.resolve_in(&runtime_root("no-limit"), &Recommends::reader(24, READER));
        assert_eq!(roomy.parallelism().jobs(), 24);
        assert_eq!(roomy.parallelism().memory_bytes(), Some(24 * READER));
        assert!(roomy.limit.is_none(), "every limit file states max");

        // Less available on the same unlimited arrangement: half of it is
        // four readers — the count coming down with the budget rather than
        // being printed beside one it cannot spend.
        let cramped =
            flagless.resolve_in(&runtime_root("cramped"), &Recommends::reader(24, READER));
        assert_eq!(cramped.parallelism().jobs(), 4);
        assert_eq!(cramped.parallelism().memory_bytes(), Some(4 * READER));

        // And a source recommending nothing on an unlimited host is the one
        // arrangement that states no budget at all — what the status line
        // renders `(default: no limit found)`.
        let plain = flagless.resolve_in(&runtime_root("no-limit"), &Recommends::jobs(1));
        assert_eq!(plain.parallelism(), Parallelism::default());
        assert_eq!(plain.budget_display(), format!("{} (default: no limit found)", 64 << 20));
    }

    /// **An allocation at or under the reserve resolves to a budget of zero,
    /// a resolved value rather than an error.** Zero produces one reader on
    /// the streaming path, and the `PlanNote` beside it tells the user their
    /// allocation bound the scan (`pgdump_query/tests/partitioned_replay.rs`,
    /// `a_budget_below_one_readers_worth_says_the_allocation_bound_it`).
    #[test]
    fn an_allocation_under_the_reserve_resolves_to_one_reader_and_no_bytes() {
        let root = runtime_root("below-reserve");
        let flagless = ParallelArgs { jobs: None, parallel_memory: None };

        for source in [Recommends::reader(24, READER), Recommends::jobs(1)] {
            let resolved = flagless.resolve_in(&root, &source);
            assert_eq!(resolved.parallelism().memory_bytes(), Some(0));
            assert_eq!(resolved.parallelism().jobs(), 1, "the floor is on the count");
        }

        // A stated budget still wins outright here: the allocation is what
        // discovery answers, not a ceiling on a person who typed one. The
        // *count* beside it is nobody's statement, so it is cut to what those
        // bytes afford.
        let stated = ParallelArgs { jobs: None, parallel_memory: Some(400 << 20) };
        let resolved = stated.resolve_in(&root, &Recommends::reader(24, READER));
        assert_eq!(resolved.parallelism().memory_bytes(), Some(400 << 20));
        assert_eq!(resolved.parallelism().jobs(), 6);
    }

    /// **A recommended count answers to the allowance however the budget
    /// arrived**, the rule being scoped to the absence of `--jobs`
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits").
    ///
    /// **Only the count moves**: a stated budget is taken whole, unlike
    /// discovery, where the budget handed back is the one the lowered count
    /// spends.
    #[test]
    fn a_stated_budget_lowers_a_recommended_count_and_keeps_its_own_bytes() {
        let root = runtime_root("no-limit");
        let source = Recommends::reader(24, READER);

        // 32 MiB affords no whole reader at all, which is the floor: one.
        let tight = ParallelArgs { jobs: None, parallel_memory: Some(32 << 20) };
        let resolved = tight.resolve_in(&root, &source);
        assert_eq!(resolved.parallelism().jobs(), 1);
        assert_eq!(resolved.parallelism().memory_bytes(), Some(32 << 20));
        assert_eq!(
            resolved.jobs_display(),
            "1 (recommended by the source; lowered from 24 by the stated budget)",
            "the clause names the flag to raise, not a cgroup nobody set"
        );
        assert_eq!(resolved.budget_display(), format!("{} (stated)", 32u64 << 20));

        // Room for more than the source asked for leaves the recommendation
        // standing, and the line says nothing about a lowering.
        let roomy = ParallelArgs { jobs: None, parallel_memory: Some(64 << 30) };
        let resolved = roomy.resolve_in(&root, &source);
        assert_eq!(resolved.parallelism().jobs(), 24);
        assert_eq!(resolved.jobs_display(), "24 (recommended by the source)");

        // A stated count is still printed as typed and never lowered, with
        // the same budget beside it — the asymmetry this arm preserves.
        let both = ParallelArgs { jobs: Some(24), parallel_memory: Some(32 << 20) };
        let resolved = both.resolve_in(&root, &source);
        assert_eq!(resolved.parallelism().jobs(), 24);
        assert_eq!(resolved.jobs_display(), "24 (stated)");

        // A source recommending no per-worker cost has nothing to divide by.
        let plain = tight.resolve_in(&root, &Recommends::jobs(1));
        assert_eq!(plain.parallelism().jobs(), 1);
        assert_eq!(plain.parallelism().memory_bytes(), Some(32 << 20));
    }

    /// **The check `introspect` owes: the instrument must not move the plan it
    /// reports on.** It changes what the process *holds*, which is the point;
    /// what it must not change is what the process *resolves*.
    ///
    /// Compiled into both configurations against the same literals, so
    /// `cargo test -p pgdump_query-cli` and `cargo test -p pgdump_query-cli
    /// --features introspect` are the two halves of the comparison. The roots
    /// are the committed ones rather than this machine's `/`
    /// (`tests/data/runtime/README.md`), and every one of them is swept: the
    /// arms differ in which term binds — a discovered ceiling, `RT8`'s
    /// half-`MemAvailable` cap, the reserve leaving nothing.
    #[test]
    fn the_instrument_build_resolves_what_the_default_build_resolves() {
        let flagless = ParallelArgs { jobs: None, parallel_memory: None };
        let source = Recommends::reader(24, READER);

        let resolved: Vec<(usize, Option<u64>)> =
            ["v2-limit", "v1-limit", "no-limit", "cramped", "below-reserve"]
                .iter()
                .map(|name| {
                    let r = flagless.resolve_in(&runtime_root(name), &source);
                    (r.parallelism().jobs(), r.parallelism().memory_bytes())
                })
                .collect();

        assert_eq!(
            resolved,
            vec![
                (9, Some(9 * READER)),
                (2, Some(2 * READER)),
                (24, Some(24 * READER)),
                (4, Some(4 * READER)),
                (1, Some(0)),
            ],
            "the resolved arrangement moved: `--features introspect` and the default build must \
             read this identically"
        );
    }

    /// **`jobs=` reads differently by provenance, and the report says so**: a
    /// recommended count is printed lowered to what the allowance affords, a
    /// stated `--jobs` as typed — so the same two readers can appear under
    /// either, and only the recommended line says what will run.
    #[test]
    fn the_report_says_which_of_the_two_counts_a_reader_is_looking_at() {
        let root = runtime_root("cramped");
        let source = Recommends::reader(24, READER);

        let flagless = ParallelArgs { jobs: None, parallel_memory: None };
        let recommended = flagless.resolve_in(&root, &source);
        assert_eq!(recommended.parallelism().jobs(), 4);
        assert_eq!(
            recommended.jobs_display(),
            "4 (recommended by the source; lowered from 24 by the allocation)"
        );

        let asked = ParallelArgs { jobs: Some(24), parallel_memory: None };
        let stated = asked.resolve_in(&root, &source);
        assert_eq!(stated.parallelism().jobs(), 24, "a stated count is not lowered");
        assert_eq!(stated.jobs_display(), "24 (stated)");
        // Both arrangements hold the same bytes, which is why the two lines
        // have to read differently.
        assert_eq!(stated.parallelism().memory_bytes(), recommended.parallelism().memory_bytes());
    }

    /// **The budget's provenance has four spellings and each names a different
    /// way of arriving at a number** — the flag, a limit that was read, a
    /// source's own answer under no limit, and the library's constant
    /// (`docs/design/decisions.md`, "D64").
    #[test]
    fn the_budget_line_names_where_its_number_came_from() {
        let reader = Recommends::reader(24, READER);
        let plain = Recommends::jobs(1);

        let stated = ParallelArgs { jobs: None, parallel_memory: Some(400 << 20) };
        assert_eq!(
            stated.resolve_in(&runtime_root("no-limit"), &reader).budget_display(),
            format!("{} (stated)", 400 << 20)
        );

        let flagless = ParallelArgs { jobs: None, parallel_memory: None };
        let discovered = flagless.resolve_in(&runtime_root("v2-limit"), &reader);
        let line = discovered.budget_display();
        assert!(line.starts_with(&format!("{} (discovered:", 9 * READER)), "{line}");
        assert!(line.contains("sys/fs/cgroup/pgdq/memory.max"), "{line}");
        assert!(line.contains(&format!("{}", 1u64 << 30)), "{line}");

        assert_eq!(
            flagless.resolve_in(&runtime_root("no-limit"), &reader).budget_display(),
            format!("{} (no limit found: what this source asks for)", 24 * READER)
        );
        assert_eq!(
            flagless.resolve_in(&runtime_root("no-limit"), &plain).budget_display(),
            format!("{} (default: no limit found)", 64 << 20)
        );
    }

    /// **A plan note names the budget that bound the plan; the clause beside
    /// it names where that budget came from** — under discovery the number to
    /// change is the *allocation*, which the note cannot know about
    /// (`docs/design/decisions.md`, "I/O, memory and parallelism").
    ///
    /// A stated budget gets no clause: the note already names what was typed.
    #[test]
    fn a_plan_note_says_where_the_budget_that_bound_it_came_from() {
        let reader = Recommends::reader(24, READER);

        let flagless = ParallelArgs { jobs: None, parallel_memory: None };
        let clause = flagless.resolve_in(&runtime_root("v1-limit"), &reader).plan_note_origin();
        assert!(clause.starts_with(" — the budget in force is "), "{clause}");
        assert!(clause.contains("memory.limit_in_bytes"), "the file that stated it: {clause}");
        assert!(clause.contains(&format!("{}", 512u64 << 20)), "the limit itself: {clause}");

        // No limit found still earns a clause: the budget is the source's own
        // ask, equally not the user's.
        let unlimited = flagless.resolve_in(&runtime_root("no-limit"), &reader);
        assert_eq!(
            unlimited.plan_note_origin(),
            format!(
                " — the budget in force is {} (no limit found: what this source asks for)",
                24 * READER
            )
        );

        let stated = ParallelArgs { jobs: None, parallel_memory: Some(400) };
        assert_eq!(stated.resolve_in(&runtime_root("v1-limit"), &reader).plan_note_origin(), "");
    }

    /// The budget a flagless resolution lands on **here**, on whatever machine
    /// the suite is running on: `None` — nobody stated one — only where no
    /// memory limit is discovered, and the discovered allowance capped at the
    /// library's own constant where one is.
    ///
    /// **The branch is the thing being pinned, not the number**: the answer
    /// depends on the cgroup the test process is in, and a test naming the
    /// constant would pass on a bare host and fail in a container. Every
    /// precedence claim below is asserted against this rather than around it.
    fn flagless_budget() -> Option<u64> {
        pgdump_query::discover_memory_limit().map(|limit| {
            pgdump_query::DEFAULT_MEMORY_BUDGET
                .min(limit.bytes.saturating_sub(pgdump_query::MEMORY_RESERVE))
        })
    }

    /// **Stating neither flag asks the source**, which is the one thing about
    /// `--jobs` no integration test can see: a partitioned run and a serial
    /// one produce the same bytes, so a default that silently reverted to a
    /// constant would pass every other test in the tree
    /// (`docs/design/measurements.md`, "The apparatus"). Which source
    /// recommends which count is `ByteRangeSource::default_workers`'s test.
    #[test]
    fn stating_no_parallelism_flag_asks_the_source() {
        let budget = flagless_budget();
        let filled = budget.unwrap_or(pgdump_query::DEFAULT_MEMORY_BUDGET);
        let stated = ParallelArgs { jobs: None, parallel_memory: None };

        // A source recommending the serial path gets it, carrying whatever
        // the environment allows — and `None`, rendered `(default)`, where no
        // limit was found.
        assert!(stated.resolve(&Recommends::jobs(1)).parallelism().is_serial());
        assert_eq!(stated.resolve(&Recommends::jobs(1)).parallelism().memory_bytes(), budget);

        // `Parallelism::Workers` has nowhere to record that nobody stated a
        // budget, so it carries the fallback bare.
        assert_eq!(
            stated.resolve(&Recommends::jobs(8)).parallelism(),
            Parallelism::workers(8, filled)
        );

        // A stated flag wins outright over a recommendation in either
        // direction.
        let asked = ParallelArgs { jobs: Some(8), parallel_memory: None };
        assert_eq!(
            asked.resolve(&Recommends::jobs(1)).parallelism(),
            Parallelism::workers(8, filled)
        );
        let serial = ParallelArgs { jobs: Some(1), parallel_memory: None };
        assert!(serial.resolve(&Recommends::jobs(24)).parallelism().is_serial());
        assert_eq!(serial.resolve(&Recommends::jobs(24)).parallelism().memory_bytes(), budget);
    }

    /// **A stated budget survives a serial worker count**: the collapse that
    /// makes `--jobs 1` serial takes the *worker count* down and not the bytes
    /// beside it, so `--parallel-memory` alone is the whole recourse for a
    /// compressed file whose blocks the default cannot hold
    /// (`docs/design/decisions.md`, "I/O, memory and parallelism").
    #[test]
    fn a_stated_budget_reaches_the_library_at_a_serial_job_count() {
        let stated = ParallelArgs { jobs: None, parallel_memory: Some(400 << 20) };
        let serial = Recommends::jobs(1);
        assert!(
            stated.resolve(&serial).parallelism().is_serial(),
            "one worker is still the serial path"
        );
        assert_eq!(stated.resolve(&serial).parallelism().memory_bytes(), Some(400 << 20));

        // Stated explicitly rather than taken from the source: the same
        // value either way.
        let one = ParallelArgs { jobs: Some(1), parallel_memory: Some(400 << 20) };
        assert_eq!(
            one.resolve(&Recommends::jobs(24)).parallelism(),
            stated.resolve(&serial).parallelism()
        );

        // A stated count with no budget beside it is the other half of the
        // pair, and the budget then comes from the environment.
        let jobs_only = ParallelArgs { jobs: Some(1), parallel_memory: None };
        assert_eq!(jobs_only.resolve(&serial).parallelism().memory_bytes(), flagless_budget());
    }

    /// The bare spelling: no whitespace anywhere means nothing to trim and no
    /// quote to strip.
    #[test]
    fn a_bare_term_parses_as_it_reads() {
        assert_eq!(ok("name=alpha"), ("name".into(), PredicateOp::Eq, Some("alpha".into())));
        assert_eq!(ok("v>=5"), ("v".into(), PredicateOp::Ge, Some("5".into())));
        assert_eq!(ok("v!=5"), ("v".into(), PredicateOp::Ne, Some("5".into())));
    }

    /// Whitespace outside quotes is not data, on **both** sides of the
    /// operator — untrimmed, the value side fails loudly on a typed column and
    /// silently on a text one.
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

    /// A quoted value is taken exactly as written, which is what keeps a
    /// space-padded `char(n)` value askable from the command line.
    #[test]
    fn a_quoted_value_is_taken_as_written() {
        assert_eq!(ok(r#"name = " x""#), ("name".into(), PredicateOp::Eq, Some(" x".into())));
        assert_eq!(ok("name = 'x '"), ("name".into(), PredicateOp::Eq, Some("x ".into())));
        assert_eq!(ok("name=''"), ("name".into(), PredicateOp::Eq, Some(String::new())));
    }

    /// Both quote characters open a value: which one a user reaches for is
    /// decided by the shell, the term normally being inside shell single
    /// quotes already.
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

    /// Quotes work on the column side too, and the operator split skips them,
    /// which is what makes a column named `a=b` askable.
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
    /// whitespace between their words — the spelling `PredicateOp::symbol`
    /// names them by, so the grammar and every refusal message agree.
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

    /// The phrase needs whitespace on both sides: a column named
    /// `is distinct from` is still askable unquoted, what follows the phrase
    /// there being `=` rather than a space.
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

    /// **The `IS` forms are the fallback**: an operator outside quotes is
    /// looked for first, so this is the equality it plainly reads as rather
    /// than an `IS NULL` on a column `note=this`.
    #[test]
    fn a_value_ending_in_is_null_is_not_an_is_null_term() {
        assert_eq!(
            ok("note=this is null"),
            ("note".into(), PredicateOp::Eq, Some("this is null".into()))
        );
    }

    /// The `IS` forms parse case-insensitively and take a quoted column name,
    /// which is what lets a column called `is null` be named.
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

    /// A malformed quote is refused, never reinterpreted. Unterminated and
    /// trailing-text are one fault with one message.
    #[test]
    fn a_malformed_quote_is_refused() {
        for spec in ["name='x", "name='x'y", "'name=x", r#"name = "x'"#, "'name' 'is null"] {
            let message = err(spec);
            assert!(message.contains("unbalanced"), "`{spec}`: {message}");
            assert!(message.contains(spec), "`{spec}`: {message}");
        }
    }

    /// A term with neither an operator nor an `IS` form is a usage fault, and
    /// the message quotes the term back.
    #[test]
    fn a_term_with_no_operator_at_all_is_a_usage_fault() {
        let message = err("nonsense");
        assert!(message.contains("--filter must be"), "{message}");
        assert!(message.contains("nonsense"), "{message}");
    }

    /// **The structural refusal runs before the term grammar**, so a term
    /// both structural and unparseable is told which of the two it is:
    /// `and is null` is refused for the reserved spelling, and the remedy is
    /// the quoting the grammar already teaches.
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
    /// the reason the line is printed for every mapped non-`Utf8View` column
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
    /// correct — the declared PostgreSQL type on the same line tells them
    /// apart.
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

    /// Dispatch is the plan's, never the field names': a user composite may
    /// declare five fields with the range struct's names and must still print
    /// as the struct it is.
    #[test]
    fn a_composite_wearing_the_range_structs_field_names_is_not_collapsed() {
        let impostor = range_struct(DataType::Int32);
        let rendered =
            arrow_type_label(&impostor, &NestedPlan::Record(vec![NestedPlan::Scalar; 5]));
        assert!(rendered.starts_with(r#"Struct("lower": Int32"#), "{rendered}");
        assert!(!rendered.contains("Range<"), "{rendered}");
    }

    /// Every `TypeKind` arm renders, with whatever payload it carries. Two
    /// are shapes `pg_dump` does not write — a composite whose body held an
    /// unparseable fragment, a range whose parameter list named no `subtype` —
    /// so this is the only place they are pinned.
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
        // A declared `canonical` function is named: it is why every filter
        // operator refuses a column of this type.
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
