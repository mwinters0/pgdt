use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use anyhow::{Context, Result};
use arrow::array::RecordBatch;
use arrow::datatypes::DataType;
use clap::{Args, Parser, Subcommand};
use futures::StreamExt;
use pgdump_query::cache::{CacheClaim, CacheMode, CacheStatus, CompressionShape};
use pgdump_query::pgtype::RANGE_STRUCT_FIELDS;
use pgdump_query::resolve::{ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
use pgdump_query::{
    ArrayShape, ByteRangeSource, CompareKind, ComparisonPlan, DataBlock, Diagnostic,
    DiagnosticKind, DumpIndex, DumpMetadata, KnownCompression, NestedPlan, Parallelism, Predicate,
    PredicateOp, QueryOptions, Recognized, ScanOptions, Severity, Span, SpanBody, TypeKind,
    open_local, preamble_only, render_field_into,
};

mod alloc;
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

/// CLI spelling of [`SchemaMode`] — see "Output model" in
/// `docs/design/decisions.md`.
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

/// The two numbers a caller states its parallelism in, shared by the two
/// scanning commands (`docs/design/decisions.md`, "I/O, memory and parallelism").
///
/// **An omitted `--jobs` is the source's own recommendation**, not a constant
/// here: [`ByteRangeSource::default_workers`] answers the serial path for a
/// plain file and this machine's core count for an `.xz` one, which is why
/// [`Discovered::resolve`] takes the open source. The library still defaults
/// to `Parallelism::default()` — it is the *CLI* that is a program a person ran
/// on purpose. `--jobs 1` is the serial path as a property of
/// `Parallelism::workers` rather than of anything here.
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
    /// [`Discovered::resolve`] under `/`, composing both halves in one
    /// call — which is what a test wants and what production cannot use,
    /// a run having a line to print between them.
    #[cfg(test)]
    fn resolve(&self, source: &dyn ByteRangeSource) -> Resolved {
        self.resolve_in(Path::new("/"), source)
    }

    /// [`ParallelArgs::resolve`] against an arbitrary filesystem root, for the
    /// reason [`pgdump_query::discover_memory_limit_in`] takes one: the arms
    /// worth pinning are a v1 hierarchy, an unlimited host and an allocation
    /// under the reserve, and no machine is more than one of those at a time
    /// (`pgdump_query-cli/tests/data/runtime/`).
    #[cfg(test)]
    fn resolve_in(&self, root: &Path, source: &dyn ByteRangeSource) -> Resolved {
        self.discover_in(root).resolve(source)
    }

    /// The half of the resolution that needs no dump: the flags as typed, and
    /// the limit this process runs under.
    ///
    /// **It is a separate step because the two halves become knowable at
    /// different moments, and one of those moments is 85 s later.** Opening an
    /// `.xz` source with no persisted seek table walks every stream footer
    /// before it can advise anything — the koji download's 31,150 of them cost
    /// 85 s (`CLAUDE.local.md`) — and until the source has been asked there is
    /// no recommendation to lower and no arrangement to report. Everything on
    /// this side is already true before the file is touched, so a run says it
    /// first and a mistyped flag is confirmed against the walk it did not
    /// affect rather than after it
    /// (`docs/design/decisions.md`, "D64").
    fn discover(&self) -> Discovered<'_> {
        self.discover_in(Path::new("/"))
    }

    /// [`ParallelArgs::discover`] against an arbitrary filesystem root, for the
    /// same reason [`ParallelArgs::resolve_in`] takes one.
    fn discover_in<'a>(&'a self, root: &'a Path) -> Discovered<'a> {
        // **Read even where `--parallel-memory` was stated**, because the mode
        // is a fact about the run and not about the flag: a user who pinned a
        // budget inside a 512 MiB cgroup is still owed the sentence saying so.
        // It costs a handful of small reads and no I/O against the dump.
        //
        // Read **once**, here, rather than by each of the two lines that
        // reports it: a second walk could answer differently — `memory.high`
        // is writable by whoever set it — and two status lines disagreeing
        // about the allocation is worse than either being stale.
        Discovered { args: self, root, limit: pgdump_query::discover_memory_limit_in(root) }
    }
}

/// What a run knows about its own allowance **before the dump is opened**: the
/// two flags exactly as they were typed, and the memory limit this process is
/// running under with the file that stated it.
///
/// Nothing here is downstream of the source, which is the whole of why it is
/// its own step ([`ParallelArgs::discover`]).
struct Discovered<'a> {
    args: &'a ParallelArgs,
    /// The filesystem root the limit was read under, kept because
    /// `Parallelism::discover_in` asks the same root again for what the
    /// machine reports free.
    root: &'a Path,
    /// The memory limit this process runs under, and the file that stated it.
    limit: Option<pgdump_query::MemoryLimit>,
}

impl Discovered<'_> {
    /// A flag's value exactly as typed, or `(not stated)`.
    ///
    /// **Parenthesised, like every other provenance marker on these lines**, so
    /// that absence can never be read as a value — and spelled out rather than
    /// left off, because the line exists for the person checking what their
    /// shell actually passed.
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
    /// (`docs/design/decisions.md`, "D64"); this one reports what
    /// was *typed*, so the CLI's own spelling is the only one that answers the
    /// question it is printed for.
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
    /// **A stated flag wins outright; absence is what asks the source.** There
    /// is no spelling for "discover" — `--jobs 0` is refused by
    /// [`parse_jobs`], since zero already reads as one through
    /// `Parallelism::workers` and a third meaning at the CLI would diverge from
    /// what the library makes of the same number.
    ///
    /// **The source is asked for both numbers, and the environment caps the
    /// second.** A source's answer can be either because both are downstream
    /// of recognition, which the caller has already paid for by the time it
    /// has a source to hand here (`docs/design/decisions.md`, "I/O, memory and parallelism"); what the *environment* allows is
    /// `Parallelism::discover_for`'s question, and it is asked only where
    /// `--parallel-memory` is absent. What the resulting budget affords still
    /// binds afterwards, `stream::worker_count` solving every count against it
    /// alike.
    ///
    /// **The two answers come back as a pair, and only a *recommended* count
    /// is lowered to fit.** `discover_for` is given a per-worker cost and a
    /// count, and where the allowance affords fewer workers it hands back the
    /// smaller count with the budget that count spends — which is the whole of
    /// "never allocate workers there is no memory for"
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits"). Where the count came from `--jobs` the stated value is kept
    /// and only the budget is taken, because that rule governs the absence of
    /// a flag and never its presence: `--jobs` states what is asked for, not
    /// what is delivered, and what is delivered is `stream::worker_count`'s to
    /// decide from the budget as it always was.
    ///
    /// **A stated budget does not exempt a recommended count from that
    /// rule.** The flag it is scoped to is `--jobs`, so where
    /// `--parallel-memory` is typed and `--jobs` is not, the source's
    /// recommendation is still cut to what those bytes afford — through
    /// `Parallelism::recommended_within`, which is `discover_for`'s lowering
    /// over a budget that arrived typed rather than discovered. Only the count
    /// moves: the budget is taken whole, a stated flag winning outright.
    ///
    /// **A discovered limit can put the budget below `DEFAULT_MEMORY_BUDGET`,
    /// and that is the point.** An allocation at or under the reserve leaves
    /// nothing of it, and the three floors inside the mechanism make that one
    /// reader's worth on the streaming path — where reasserting the constant
    /// would hand a tight cgroup the same 64 MiB an unlimited host gets.
    ///
    /// **A stated budget reaches the library at every worker count**, the
    /// serial state carrying one of its own — so `--parallel-memory` is worth
    /// stating beside a serial `--jobs`, which is what buys back a compressed
    /// file's block path without also asking for a second worker
    /// (`docs/design/decisions.md`, "I/O, memory and parallelism").
    ///
    /// **Only a resolved-serial arrangement can state no budget**, which is
    /// what lets the status line say `(default)` truthfully there: the CLI's
    /// own fallback and the library's are the same number, and printing it as
    /// though it had been asked for is the only way that line can lie. A
    /// `Workers` arrangement has nowhere to put "nobody stated one" —
    /// `memory_bytes` is not an `Option` on that variant — so it carries
    /// `DEFAULT_MEMORY_BUDGET` bare, exactly as a stated `--jobs 8` always did.
    fn resolve(self, source: &dyn ByteRangeSource) -> Resolved {
        let args = self.args;
        // Asked of the source only where `--jobs` was absent — a stated count
        // is not a recommendation and has nothing to be lowered from, which is
        // what the mode report reads this back for.
        let recommended_jobs = match args.jobs {
            Some(_) => None,
            None => Some(source.default_workers()),
        };
        let jobs = args.jobs.or(recommended_jobs).unwrap_or(1);
        let parallelism = match args.parallel_memory {
            // A stated budget is taken whole — the flag wins outright — but a
            // *recommended* count still answers to it, exactly as it answers
            // to a discovered one. What the rule is scoped to is the absence
            // of `--jobs`, which is absent on this arm too.
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
                    // A stated count is not lowered by the environment: the
                    // flag states what is asked for, and what the budget
                    // delivers still binds through `stream::worker_count`.
                    // Only the count `discover_for` was *recommending* is its
                    // to reduce.
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
/// from — the arrangement itself plus the provenance
/// [`Parallelism`] has nowhere to carry
/// (`docs/design/decisions.md`, "D64").
///
/// **Provenance is the CLI's fact, not the library's.** Whether a number was
/// typed is knowable only here, and whether a limit was read is knowable only
/// to the walk that read it — so neither can be recovered from a
/// [`Parallelism`] downstream, and the library's own `scan started` line keeps
/// saying what bound applies rather than where it came from.
#[derive(Debug, Clone)]
struct Resolved {
    /// The arrangement the library is handed.
    parallelism: Parallelism,
    /// The memory limit this process runs under, and the file that stated it —
    /// `None` meaning no limit is being *enforced*, which is a complete
    /// statement however the process was started.
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
    /// **A recommended count reads differently from a stated one**, and the
    /// difference is what is being reported: a recommendation is lowered to
    /// what the allowance affords and printed lowered, while a stated `--jobs`
    /// is printed as typed and what it actually delivers stays
    /// `stream::worker_count`'s to decide from the budget. So the same two
    /// readers can appear under `jobs=2` and under `jobs=24`, and only the
    /// first is telling the user what will run
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits").
    ///
    /// **What did the lowering is named, because the two are different
    /// numbers to change.** A discovered budget is the environment's, so the
    /// clause says `by the allocation`; a `--parallel-memory` the user typed
    /// lowers the count just as hard and the recourse is their own flag, so it
    /// says `by the stated budget`. [`Resolved::budget_display`] on the line
    /// beside it then says which number that was.
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
    /// Four spellings, because there are four ways to arrive at a number and
    /// only the first is the user's own: the flag; a discovered limit, named
    /// by the file that stated it, since `memory.high` throttles where
    /// `memory.max` kills and either may be an ancestor's; the source's own
    /// recommendation, taken whole because nothing capped it; and the
    /// library's constant, which is what "no limit found" leaves a source that
    /// recommends nothing.
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
    /// **not** stated — the half of the story the library cannot tell.
    ///
    /// Every [`pgdump_query::PlanNote`] names a memory budget as the thing that
    /// bound the plan, and the widest of them is the compressed block path
    /// going serial, which is a throughput cliff. Where that budget came off
    /// the environment the note alone leaves an operator to infer *which*
    /// number to change from a status line that says only what was resolved, so
    /// the CLI appends the provenance — the same [`Resolved::budget_display`]
    /// the mode report prints, so the two cannot part company
    /// (`docs/design/decisions.md`, "D64").
    ///
    /// **Empty where `--parallel-memory` was stated**, because the note already
    /// names the number that person typed and the recourse is to raise it.
    fn plan_note_origin(&self) -> String {
        if self.budget_stated {
            return String::new();
        }
        format!(" — the budget in force is {}", self.budget_display())
    }

    /// Say, once per scanning command and before the scan starts, what the
    /// source's recommendation and the allowance fitted to.
    ///
    /// **This is the only line that can name a count the allowance lowered**,
    /// and it is why the report is two lines rather than one: the lowering is
    /// the source's recommendation meeting the budget, so neither number exists
    /// until the file has been opened and asked
    /// ([`Discovered::announce`] carries the half that does).
    ///
    /// **The mode is reported because the quiet failure is a recommendation
    /// nobody can see was reduced.** Under an orchestrator the operator
    /// assigned an allocation and pgdq fills it; with no limit found pgdq is a
    /// guest on a machine nobody promised it and stays inside half of what the
    /// kernel says is available (`RT8`) — and in that second arrangement a
    /// worker count cut to fit surfaces as unexplained slowness unless the run
    /// says so ([2026-09-10](../../docs/status/history/2026-09-10.md), "The
    /// no-limit cap is affirmed, and a run says which mode it is in").
    fn announce(&self) {
        tracing::info!(
            jobs = %self.jobs_display(),
            memory_bytes = %self.budget_display(),
            "resolved the arrangement",
        );
    }
}

/// A `--jobs` value: a worker count, and never zero. Zero would read as one
/// through `Parallelism::workers`, but a person who typed it meant something,
/// and silently answering "serial" is the kind of surprise a flag should not
/// hold.
fn parse_jobs(text: &str) -> std::result::Result<usize, String> {
    match text.parse::<usize>() {
        Ok(0) => Err("a job count of 0 would run no workers; --jobs 1 is the serial path".into()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

/// A `--parallel-memory` value: a byte count, and never zero. Zero affords no
/// buffer of any unit, so every pool would fall back to its one-slot floor and
/// a compressed source to its streaming reader — a configuration nobody wants
/// and one the flag should refuse rather than honour.
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
        #[command(flatten)]
        parallel: ParallelArgs,
    },
    /// Report what a dump's cache holds. **`info` never scans** — it reads the
    /// cache `pgdq parse` wrote and errors if there is not one, rather than
    /// starting an hours-long scan on your behalf
    /// (`docs/design/decisions.md`, "The CLI"). A cache from an
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
        /// into a listing of the types themselves, and on a compressed dump
        /// adds the container's shape.
        #[arg(long)]
        detail: bool,
        /// List every span the map holds (`docs/design/decisions.md`,
        /// "D34") — DDL objects
        /// and framing included, not just `COPY` blocks — instead of the
        /// per-table listing.
        #[arg(long)]
        map: bool,
        /// Print the internal index as JSON instead of the human-readable
        /// listing: the whole `DumpIndex`, its coverage, its diagnostics, and
        /// the per-`COPY`-block type resolution `--detail` renders as text.
        /// No schema stability is promised — this is a raw dump of our
        /// internal representation, not a supported interchange format
        /// (`docs/design/decisions.md`, "The CLI"). Incompatible with
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
        #[command(flatten)]
        parallel: ParallelArgs,
    },
}

/// A `--chunk-size` value: a byte count, and never zero.
///
/// Zero is refused here rather than at the read loop because the loop's
/// `min(chunk_size, remaining)` would ask for nothing, forever — a scan that
/// never advances and never errors, which is the one input shape a knob like
/// this can turn into a hang.
fn parse_chunk_size(text: &str) -> std::result::Result<usize, String> {
    match text.parse::<usize>() {
        Ok(0) => Err("a chunk size of 0 would read nothing".to_string()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

/// The [`ScanOptions`] one scanning command runs under: the default, with
/// `--chunk-size` applied where it was given and the already-resolved
/// arrangement ([`Discovered::resolve`]).
///
/// **Resolved once per command and passed in, not re-resolved here.** `query`
/// needs the same arrangement in [`QueryOptions`] as in its mapping pass, and
/// resolving twice would read the environment twice and announce it twice.
fn scan_options(chunk_size: Option<usize>, parallel: &Resolved) -> ScanOptions {
    ScanOptions {
        chunk_size: chunk_size.unwrap_or(pgdump_query::DEFAULT_CHUNK_SIZE),
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
/// `docs/design/decisions.md`, "D60").
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
/// (`docs/design/decisions.md`, "D60").
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
/// (`docs/design/decisions.md`, "Predicates", the comparison register).
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
/// One sub-stream's place in `pgdq query`'s k-way merge: at most one batch,
/// held with the two things a `RecordBatch` does not carry and the printer
/// needs (`docs/design/decisions.md`, "D51").
///
/// **One batch per partition is the whole bound.** Each sub-stream yields in
/// file order and the sub-streams themselves are in file order, so emitting
/// the held batch with the lowest source offset re-assembles the serial order
/// while never holding more than N batches — where an unordered stream merged
/// by buffering until the gap closes is bounded by nothing.
enum Slot {
    /// Nothing held: this sub-stream is polled in the next fill round.
    Empty,
    /// A batch waiting its turn, with the source offset it begins at — the
    /// merge key — and the nested plans of the block it came from, read at
    /// the moment it was taken because its sub-stream may since have moved to
    /// a block with a different schema.
    Held { offset: u64, batch: RecordBatch, plans: Vec<NestedPlan> },
    /// Drained, stopped at an error, or sitting at or after one — every row
    /// past the earliest failure belongs to a serial replay that never got
    /// there. Never polled again, and a batch it was holding is discarded.
    Done,
}

fn announce_comparisons(stream: &pgdump_query::TableStream<'_>) {
    for note in stream.comparison_notes() {
        eprintln!("warning: {}", note.message());
    }
}

/// Say, once per query and on stderr, when the memory budget in force cut the
/// plan short. Every sub-stream of a partitioned replay carries the
/// same [`pgdump_query::TableStream::plan_notes`], settled before any of them
/// runs, so reading it off the first is reading the whole query's answer —
/// unlike [`announce_comparisons`], this needs no block to have resolved
/// first.
///
/// **Each note is followed by where its budget came from**
/// ([`Resolved::plan_note_origin`]), which is the CLI's fact and not the
/// library's: a note says a budget declined something, and only this layer
/// knows whether that number was typed or read off a cgroup.
fn announce_plan_notes(stream: &pgdump_query::TableStream<'_>, parallel: &Resolved) {
    let origin = parallel.plan_note_origin();
    for note in stream.plan_notes() {
        eprintln!("warning: {}{origin}", note.message());
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
/// (`docs/design/decisions.md`, "D63").
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
/// [`render_field_into`], so output is byte-identical whether `--schema-mode`
/// is `typed` or `strings` (`docs/design/decisions.md`,
/// "The CLI").
///
/// **One buffer for the whole batch.** The line is assembled in a `String`
/// that is cleared per row and keeps its capacity across the batch, so a
/// scalar field is written where it will be printed from — in place of a
/// `String` allocated per field, collected into a `Vec` and then copied again
/// by `join`.
///
/// `plans` is the stream's own [`pgdump_query::ResolvedSchema::plans`], which
/// is what says whether a `List<Struct{…}>` column is written as an array of
/// ranges or as a multirange. A column with no entry falls back to
/// `NestedPlan::Scalar`, which is right for every non-nested type.
///
/// The `Result` is `render_field_into`'s refusal of a value with no PostgreSQL
/// text form, which **no batch this binary prints can hold**: every typed
/// column here is filled by a decoder whose range its renderer can write back.
/// It is propagated rather than unwrapped because an unreachable panic in the
/// output path is a worse answer than an error message. A refused value can
/// leave a partial field in the buffer; nothing prints it, because the error
/// ends the query.
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
/// for `parse`, `info` and `query` alike (`docs/design/decisions.md`,
/// "D64"). A
/// per-command default would be a rule the manual has to explain, and gating
/// on whether stderr is a terminal makes the output depend on invocation
/// context — which is exactly the case that left the koji verification's
/// first attempt with nothing but `dmesg` to diagnose from.
///
/// One level, `INFO`, and no way yet to raise or lower it — `-vvv` and
/// `--quiet` are deferred and unallocated. RFC3339 timestamps
/// (`UtcTime::rfc_3339`) are the convention the koji orchestrator logs
/// already use, so a `pgdq` line correlates directly with one from either.
/// No ANSI color: these lines are as likely to land in a redirected log file
/// as a terminal, and `query` writes row data to stdout, so stderr is the
/// only place this can go without corrupting a pipe.
fn init_status_output() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .init();
}

/// **A `current_thread` runtime, not a multi-threaded one.** Every unit of work
/// this binary dispatches is a `spawn_blocking` task — the positioned reads,
/// the fused decode-and-parse workers, and the sub-streams of a partitioned
/// replay alike (`docs/design/decisions.md`, "I/O, memory and parallelism") — so the reactor never runs any of it, and a pool of reactor
/// threads sized from the host's CPU count is threads the work never touches.
/// The blocking pool tokio creates on demand is what actually carries the
/// scan, so the process's thread count follows the concurrency dispatched
/// rather than the number of CPUs it can see.
///
/// **Claimed as a thread-count result, not a memory one.** Fewer threads means
/// fewer glibc arenas seeded, but an arena's retention is not proportional to
/// how many there are — a probe on koji's `.xz` at `--jobs 4`, on a build whose
/// runtime still sized itself from the host, found `--cpus 4` cutting 24 arenas
/// to 8 (and the runtime's own threads with them) and anonymous resident only
/// ~536 to ~476 MiB, a reading in no published figure — so this does not on
/// its own make the
/// process smaller, and no reading here says it does. What it buys is that the
/// process no longer sizes itself from a number nobody stated.
///
/// The `signal` handlers of [`install_interrupt_guard`] are ordinary
/// `tokio::spawn` tasks and run on this thread: the scan loop awaits a
/// `spawn_blocking` join at every piece, so the runtime is parked in
/// `block_on` — driving the signal driver — for all of the time the work is
/// actually running.
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // A default build drops this entirely; an `introspect` build writes what
    // the process held to the file `PGDQ_INTROSPECT_OUT` names, on the way
    // out, whether this returns `Ok` or an error propagates through it
    // (`src/introspect.rs`).
    let _instrument = introspect::at_exit();
    init_status_output();
    let cli = Cli::parse();
    match cli.command {
        Command::Parse {
            source: file,
            dqcache,
            preamble_only: preamble_only_flag,
            chunk_size,
            parallel,
        } => {
            // `parse` is the only scanner (`docs/design/decisions.md`,
            // "The CLI"). Reject `--dqcache none` up front, before paying
            // for a scan we won't be allowed to persist.
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let path = mode
                .require_enabled("parse")
                .context("`--dqcache none` cannot be combined with `parse`")?
                .to_path_buf();
            // A cache whose compression claim this file contradicts is a
            // cache for some other file, and `parse` refuses it exactly as
            // `info` and `query` do: it would otherwise scan and overwrite
            // it, which is the one thing this library does not do on its own
            // (`docs/design/decisions.md`, "The compressed source and the cache"). Refused having
            // read nothing, so no footer walk is spent reaching it — and the
            // same is true of the stored-size mismatch, which `open_for_scan`
            // answers with the library's own error before opening anything.
            //
            // **The flags and the limit are announced ahead of this**, since
            // neither waits on the file: opening a fresh `.xz` walks its
            // stream footers first, which is 85 s on the koji download, and a
            // mistyped `--parallel-memory` should not go unconfirmed through it
            // (`docs/design/decisions.md`, "D64").
            let stated = parallel.discover();
            stated.announce();
            let source = open_for_scan(&file, &mode)?;
            let parallel = stated.resolve(source.as_ref());
            parallel.announce();
            if preamble_only_flag {
                let (metadata, diagnostics) =
                    preamble_only(source.as_ref(), &scan_options(chunk_size, &parallel), &mode)
                        .await?;
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
                ScanOptions { cancel: Some(cancel), ..scan_options(chunk_size, &parallel) };
            let run = pgdump_query::map_file(source.as_ref(), &scan_options, &mode).await?;
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
                //
                // `std::process::exit` runs no destructors, so the instrument
                // is asked here rather than left to `main`'s guard — an
                // interrupted scan is exactly the run whose resident account
                // someone wants.
                introspect::report();
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
                // Cache-only mode (`docs/design/decisions.md`,
                // "The compressed source and the cache"): no live dump file at all, so
                // clap already required `--dqcache` for us.
                let path = dqcache.expect("clap requires --dqcache when --source is omitted");
                return info_offline(&path, detail, map, json).await;
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
            // `info` never scans, so a cache that does not describe this file
            // leaves nothing to report from — and it says so having read
            // nothing, rather than spending an `.xz` file's footer walk to
            // reach an error it was always going to reach. Both conditions
            // stop before the open; this one keeps `info`'s own sentence,
            // which names the two ways out ahead of the command they enable.
            let source = match open_with_cache(&file, &mode)? {
                Opened::Source(source) => source,
                Opened::SourceChanged { cached_stored_size, live_stored_size } => {
                    let changed =
                        CacheStatus::SourceChanged { cached_stored_size, live_stored_size };
                    anyhow::bail!(unusable_cache_message(&changed, &path, Some(&file)))
                }
            };
            let status = pgdump_query::cache::load(&path, source.as_ref()).await?;
            let (mut index, mtime_changed, total_size, compression) = match status {
                CacheStatus::Valid { index, mtime_changed, total_size, compression }
                | CacheStatus::Incomplete { index, mtime_changed, total_size, compression } => {
                    (index, mtime_changed, total_size, compression)
                }
                unusable => anyhow::bail!(unusable_cache_message(&unusable, &path, Some(&file))),
            };
            if mtime_changed {
                index.diagnostics.push(Diagnostic::cache_mtime_changed());
            }
            report(&index, total_size, compression, detail, map, json);
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
            parallel,
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
            // As `info`: a cache that does not describe this file is
            // reported having read nothing, rather than paying a footer walk
            // and a whole scan over a map that cannot be trusted. `parse` is
            // the command that rebuilds it.
            // Announced in two lines, the first ahead of the open, exactly as
            // `parse` does and for the same reason.
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
                // The same flags on both passes: `pgdq query` runs one mapping
                // scan and one replay over one source, so the number a person
                // typed is the number both of them work inside.
                parallelism: parallel.parallelism(),
                ..QueryOptions::default()
            };
            // Pull mode, not `read_table`: rendering a nested column back to
            // its literal needs the stream's `NestedPlan`s, and push mode
            // only hands the resolved schema back once the whole stream has
            // been drained (`docs/design/decisions.md`, "D46"). The scan itself is the same one —
            // `read_table` drains this stream internally.
            //
            // Partitioned, not serial: the split is where `--jobs` becomes
            // something other than a bound on what the source retains, and
            // `Parallelism::Serial` — `--jobs 1` — is one sub-stream, so the
            // serial path is reached through the same call rather than
            // branched to (`docs/design/decisions.md`, "D51"). What `table_stream` reports as its stream's first
            // item, this reports from the `await`; both are the same errors
            // with the same wording.
            let mut streams = pgdump_query::table_stream_partitions(
                source.as_ref(),
                &table,
                scan_options(chunk_size, &parallel),
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
            // **Recorded rather than raised**, which is the whole of the
            // ordering rule on this side: sub-stream `k` reads a contiguous run
            // of blocks after `k-1`'s, so a failure in a lower-indexed one is
            // earlier in the file however much later it arrives, and raising
            // whichever failed first in time would name a different row on each
            // run over an unchanged file
            // (`docs/design/decisions.md`, "D65").
            let mut failed: Option<(usize, pgdump_query::Error)> = None;
            loop {
                // Everything at or after a failing sub-stream is dead, and
                // **discarding what those slots hold is the load-bearing
                // half**: the first round fills every slot, so the sub-streams
                // after the failing one are routinely holding a batch, and left
                // there it would print the moment the ones before it drained —
                // rows past the error that a serial replay never reached. Only
                // the sub-streams *before* the failure go on being drained, and
                // one of them failing in turn moves the frontier down again.
                let live = failed.as_ref().map_or(slots.len(), |(index, _)| *index);
                for slot in slots.iter_mut().skip(live) {
                    *slot = Slot::Done;
                }
                // Refill every empty slot at once. In the first round that is
                // every sub-stream; after it, only the one just drained — so
                // the reads a sub-stream ahead of the printer issues stop at
                // one batch, which is what makes the merge's bound N × batch
                // rather than a reorder buffer.
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
                                        // The plans belong to the block this
                                        // batch came from, so they are taken
                                        // now: by the time it is printed its
                                        // own sub-stream may have moved on to
                                        // a block whose header named other
                                        // columns.
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
                    // `join_all` answers in argument order, which is partition
                    // order, which is file order — so the first failure in it
                    // is the lowest-indexed of this round's, and every slot
                    // this round could fill was already below whatever failed
                    // before it.
                    futures::future::join_all(fills).await.into_iter().flatten().next()
                };
                // Round again rather than printing: the sub-streams this
                // failure has just killed may be holding batches, and the loop
                // head is what marks them dead before the merge next picks.
                // It terminates because a recorded failure strictly lowers
                // `live` and a new one can only come from a slot below it.
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
                    // Off the first sub-stream, not off the one this batch
                    // came from: its first segment starts at a `COPY` header,
                    // so it has resolved a schema by now whether or not it had
                    // rows to show for it — which is the block the serial path
                    // announced from too.
                    announce_comparisons(&streams[0]);
                    announced = true;
                }
                any_batch = true;
                // A zero-column projection prints no header. The header would
                // be an empty line, and the row count `--no-columns | wc -l`
                // is asked for would come back one too many
                // (`docs/design/decisions.md`, "D28").
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
            // held now is the earliest error in the file — the one a serial
            // replay would have stopped at, and the one every re-run gets.
            // Raised after the rows before it have printed, exactly as the
            // serial path prints up to the row it dies on.
            if let Some((_, err)) = failed {
                return Err(name_taken_verbatim(err));
            }
            // A query that matched a block but selected no rows still
            // resolved a schema, so the announcement is owed either way; it
            // is made at the first batch when there is one so it precedes the
            // rows rather than trailing them.
            if !announced {
                announce_comparisons(&streams[0]);
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

/// What [`open_with_cache`] found: the source to read, or the one refusal the
/// cache path settles on its own, before anything is opened.
///
/// **The second variant is not a refusal this helper can write.** The
/// contradicted compression claim below reaches all three commands in one
/// sentence, so `open_with_cache` bails on it; a stored-size mismatch does
/// not — `parse` and `query` surface the library's own
/// `Error::CacheSourceMismatch` and `info` prints the sentence that names the
/// two ways out before `pgdq parse`
/// (`docs/design/decisions.md`, "D20"). Handing the condition back is what keeps those three wordings where
/// they already are while the walk is spared.
enum Opened {
    /// The source, ready to read.
    Source(Arc<dyn pgdump_query::ByteRangeSource>),
    /// The cache at this mode's path records a stored size the file does not
    /// have, so it describes another file. The very condition — and the very
    /// two numbers — `cache::load` would have answered
    /// [`CacheStatus::SourceChanged`] with once a source existed.
    SourceChanged { cached_stored_size: u64, live_stored_size: u64 },
}

/// Open `file`, handing recognition whatever the cache at `cache` says about
/// its compression layer, so an `.xz` source is built from the seek table a
/// previous walk already produced instead of re-walking the file's stream
/// footers (`docs/design/decisions.md`, "The compressed source and the cache").
///
/// `--dqcache none` claims nothing, which is what makes an opted-out cache
/// cost exactly the walk it always did; cache-only mode never reaches here at
/// all, having no live source to open.
///
/// **A contradicted claim is refused here rather than at each of the three
/// call sites.** All three commands answer it identically — the cache at that
/// path was written from another file, so it is not this one's to overwrite
/// (`docs/design/decisions.md`, "The compressed source and the cache") — and the mode holding the
/// claim is the mode holding the path the message names, so the refusal has
/// everything it needs without a caller passing it back down.
///
/// **A cache recorded against a file of another stored size stops here too**,
/// as an [`Opened`] variant rather than as a bail: the file is never opened,
/// so an `.xz` source never walks its stream footers to reach a refusal the
/// cache path alone already settles (`docs/design/decisions.md`, "The compressed source and the cache"). The comparison itself stays in `cache::claim`, so this is the same
/// verdict the library reaches a moment later rather than a second reading of
/// the same rule.
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

/// [`open_with_cache`] for the two commands that scan. Both surface the
/// library's own `Error::CacheSourceMismatch` for a cache written against
/// another file, so both raise it here — from `CacheMode::source_mismatch`,
/// the same constructor the three scan entry points use, which is what makes
/// the earlier refusal word-for-word the one it pre-empts
/// (`docs/design/decisions.md`, "The compressed source and the cache").
///
/// The library still refuses on its own: this spares the walk, it does not
/// replace the guarantee, which is the library's to keep for an embedder that
/// never goes through this binary.
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
/// **This is the second of the two "written for another file" conditions**,
/// and it is deliberately not one of [`unusable_cache_message`]'s: that
/// function matches on [`CacheStatus`], and this condition is not one —
/// recognition catches it before a source exists, so `load` never sees it
/// (`docs/design/decisions.md`, "The compressed source and the cache"). It borrowed
/// `Unreadable`'s sentence until the refusal made the two answers differ:
/// "check the path, or run `pgdq parse`" is advice `parse` cannot take, being
/// the command that just refused, and the bytes at that path *are* a pgdq
/// cache — for some other file.
///
/// The tail is the pair `Error::CacheSourceMismatch` names for the other
/// condition, in the same words: the two ways out of a cache that is valid
/// for a file that is not this one (D5 — there is no override).
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
/// `CacheMode` variant meaning "replace regardless" — because a flag like that
/// is set once in a script and never reconsidered
/// (`docs/design/decisions.md`, "The compressed source and the cache").
///
/// `pgdump_query::Error::CacheSourceMismatch` carries the same clause for
/// the size-mismatch condition, which reaches `parse` and `query` from the
/// library rather than from here; `refusals_name_both_ways_out`
/// (`tests/partial_reporting.rs`) is what holds the three of them to one
/// wording.
const TWO_WAYS_OUT: &str = " — remove it, or name a different cache path";

/// The sentence `pgdq info` prints for a cache it cannot use. All four causes
/// end in `pgdq parse`, and they are still four different sentences: the fact
/// the user needs to know differs — "you have never parsed this file" and
/// "your file changed since you parsed it" send a reader to different places.
///
/// **Three of the four reach `pgdq parse` directly and one does not.** `parse`
/// scans over `Missing`, `Unreadable` and `UnsupportedVersion` — there is
/// nothing at that path worth keeping — and refuses `SourceChanged`, so that
/// arm names [`TWO_WAYS_OUT`] before it names the command
/// (`docs/design/decisions.md`, "The compressed source and the cache").
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
        CacheStatus::SourceChanged {
            cached_stored_size: cached_size,
            live_stored_size: live_size,
        } => {
            let source = source.expect("cache-only mode has no live source to compare against");
            // The one arm whose remedy is not `pgdq parse` on its own. `parse`
            // refuses this very condition rather than scanning over it, so
            // sending a reader straight there would send them to a second
            // refusal; the two ways out come first, and `parse` then works
            // (`docs/design/decisions.md`, "The compressed source and the cache").
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
/// (`docs/design/decisions.md`, "The compressed source and the cache"). An `Incomplete` cache is
/// reported like any other, with its coverage stated — cache-only mode has no
/// scan to extend it with, but "as far as the scan got" is still an answer,
/// and refusing it was what this phase removed.
async fn info_offline(path: &Path, detail: bool, map: bool, json: bool) -> Result<()> {
    let mode = CacheMode::Offline(path.to_path_buf());
    let (index, total_size, compression) = match mode.load_offline().await? {
        CacheStatus::Valid { index, total_size, compression, .. }
        | CacheStatus::Incomplete { index, total_size, compression, .. } => {
            (index, total_size, compression)
        }
        unusable => anyhow::bail!(unusable_cache_message(&unusable, path, None)),
    };
    report(&index, total_size, compression, detail, map, json);
    Ok(())
}

/// One column's resolution outcome, in both spellings: a stable token for
/// `--json` and the sentence `info --detail` prints
/// (`docs/design/decisions.md`, "The CLI").
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

/// The sentence half of [`resolution_words`] — `info --detail`'s per-column
/// line.
fn resolution_label(r: &ColumnResolution) -> &'static str {
    resolution_words(r).1
}

/// What one column became in Arrow — the other half of `info --detail`'s
/// per-column line (`docs/design/decisions.md`, "The CLI").
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
/// where resolution already put them (`docs/design/decisions.md`,
/// "The CLI").
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

/// The labels as `info --detail` prints them: each one single-quoted with any
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
/// user-defined-type counts (`docs/design/decisions.md`,
/// "The CLI"). The `database: <name>` line is only shown when it's informative —
/// a single unnamed database (a plain, non-`--create` dump: the overwhelming
/// common case) is printed with no header line, since one would just be
/// noise.
///
/// Under `detail`, the `user-defined types` count becomes the heading of a
/// listing of the types themselves, one line each, in the order the dump
/// declares them. The count is otherwise their only trace: nothing else in
/// `info` names a user-defined type, so a user cannot learn from it that
/// `public.mood` exists, let alone what it holds.
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
/// (`docs/design/decisions.md`, "The CLI").
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
/// from — the single resolution pass `--detail`'s text and `--json`'s export
/// both render (`docs/design/decisions.md`, "The CLI"). Two passes is
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
/// (`docs/design/decisions.md`, "D35").
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
/// (`docs/design/decisions.md`, "The compressed source and the cache"), and the per-block type
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
    /// The container's shape, `null` for a plain file — the same three
    /// numbers [`compression_line`] prints, exported because `--json` claims
    /// to carry everything `--detail` would add and this is part of it.
    compression: Option<CompressionShape>,
    diagnostics: &'a [Diagnostic],
    resolution: Vec<BlockResolutionJson<'a>>,
}

/// One `COPY` block's resolution, keyed by the block rather than rolled up per
/// table. A table can span blocks (I2) and a header-less block names its
/// columns from its first row, so a per-table rollup needs a merge rule that
/// does not exist yet; leaving the grouping to the consumer is where it
/// honestly sits (`docs/design/decisions.md`, "The CLI").
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
    complete: bool,
) {
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
    let wrapped =
        IndexJson { index, total_size, compression, diagnostics: &index.diagnostics, resolution };
    println!("{}", serde_json::to_string_pretty(&wrapped).expect("DumpIndex is always valid JSON"));
}

/// How much of the file the index covers, stated **once, at the top**, with
/// nothing below it qualified (`docs/design/decisions.md`, "The CLI").
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
fn report(
    index: &DumpIndex,
    total_size: u64,
    compression: Option<CompressionShape>,
    detail: bool,
    map: bool,
    json: bool,
) {
    let complete = index.is_complete(total_size);
    if json {
        print_index_json(index, total_size, compression, complete);
        return;
    }
    println!("{}", completion_line(index.scanned_through, total_size));
    println!();
    print_index(index, compression, detail, map, complete);
}

/// The container line `info --detail` prints above the listing, and
/// nothing at all for a plain file, which has no container to describe.
///
/// **Three numbers a user is otherwise sent to `xz --list` for**, which on the
/// shape that most wants asking (many concatenated streams) is a walk of every
/// footer in the file. `largest block` is the largest term of what
/// `--parallel-memory` has to clear for a query to read this file a block at a
/// time — **four times over**, one reader's block beside the three further
/// blocks the pool keeps however few readers run, plus a read buffer and the
/// decompressor's own working memory — so
/// the flag that says *raise it* is most of the way answered here, and a query
/// that declines the block path names the whole of it
/// (`docs/design/decisions.md`, "The compressed source and the cache").
fn compression_line(shape: &CompressionShape) -> String {
    format!(
        "compression: {} — {} block(s) in {} stream(s), largest block {} bytes uncompressed",
        shape.container, shape.blocks, shape.streams, shape.max_block_uncompressed
    )
}

/// Print the whole listing, below whatever coverage line [`report`] already
/// stated. `complete` is passed straight through to [`block_resolutions`],
/// which is where it means something.
///
/// **Nothing here is qualified by how much of the file was scanned.** The
/// coverage line above says it once; a partial index's records are each
/// complete in themselves (see [`completion_line`]), so repeating the caveat
/// per block would suggest a variation that does not exist.
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
        if detail {
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
            "{total_unmapped} of {total_columns} columns unmapped — run with --detail for details"
        );
    }
}

/// `DumpIndex::diagnostics` (or, for `--preamble-only`, the diagnostics
/// `preamble_only` reports separately), printed unconditionally — this is
/// (`docs/design/decisions.md`, "The compressed source and the cache"). Cache-only mode's
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
        DiagnosticKind::NonSeekableCompressedSource { block_count } => format!(
            "this .xz source has no seek structure ({block_count} block(s), one stream) — every read decodes the file from byte 0; recompress with `xz -T0` or `--block-size=<size>` for random access"
        ),
    }
}

/// Referenced-role and referenced-tablespace summary
/// (`docs/design/decisions.md`, "D31")
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
/// (`docs/design/decisions.md`, "D31"). Counts `toc_owned`
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
/// filters it down to (`docs/design/decisions.md`,
/// "D34"). Grouped by [`DatabaseHeadings`], the
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
/// readers. [`type_kind_summary`] is the `--detail` type listing's fuller
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

    /// A source recommending whatever it is built with, so that
    /// [`ParallelArgs::resolve`]'s two halves — ask the source, or honour the
    /// flag — can be told apart without a real file of either shape. The three
    /// required methods answer nothing: `resolve` reads exactly one method and
    /// never touches a byte.
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
    /// shaped like a Linux one, so that a resolution can be pinned against an
    /// environment this machine is not in.
    fn runtime_root(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/runtime").join(name)
    }

    /// A source recommending twenty-four readers of 58 MiB each and no pool
    /// of its own — the flat shape every source but the block-decoding one
    /// states. The block-decoding shape is `BLOCK_READER` beside its unit.
    const READER: u64 = 58 << 20;

    /// **A discovered limit is what a flagless run resolves inside, on both
    /// cgroup versions.** The v1 arm is the one no machine here can produce
    /// (`RT4`, `RT6`), and it is reached through `mountinfo` rather than
    /// through the conventional mount — so what this pins is that a resolution,
    /// not merely the reader beneath it, comes out of the shape the register
    /// describes.
    ///
    /// The pair is consistent in both: the count is what the allowance affords
    /// under the margin and the budget is exactly what that many readers
    /// spend, never the cap itself (`docs/design/roadmap.md`, "A default runs
    /// as fast as the allocation permits").
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

    /// **A shared pool lowers the recommended count at every allocation**, and
    /// hardest at the small ones. A block-decoding source's pool retains a
    /// unit for every slot but the one a reader is filling — `POOL_DEPTH - 1`
    /// of them below four readers and `jobs - 1` above — so the allowance
    /// always buys fewer than a division by the per-reader charge would say.
    /// Pinned at both ends, because a charge that billed no pool would bind
    /// nowhere and one billing two units a reader would over-bill.
    #[test]
    fn a_shared_pool_lowers_a_recommended_count_at_every_allocation() {
        let flagless = ParallelArgs { jobs: None, parallel_memory: None };
        const UNIT: u64 = 24 << 20;
        // What one reader of an ordinary 24 MiB-block dump holds: the block it
        // is decoding — which is the block it then retains — the chunk buffer
        // a straddling read is assembled into, and the decoder.
        const BLOCK_READER: u64 = 34 << 20;
        let source = || Recommends::block_reader(24, BLOCK_READER, UNIT, 4);

        // v1, a 512 MiB limit: 128 MiB after the reserve, which a division by
        // the per-reader charge calls three readers. The three units the pool
        // retains beside a single reader leave only 22 MiB of that, so a
        // second reader does not fit and one is the honest answer — spending
        // 34 + 3 x 24 = 106 MiB of the 128.
        let tight = flagless.resolve_in(&runtime_root("v1-limit"), &source());
        assert_eq!(tight.parallelism().jobs(), 1);
        assert_eq!(tight.parallelism().memory_bytes(), Some(BLOCK_READER + 3 * UNIT));

        // v2, a 1 GiB limit: 563.2 MiB once the reserve and the margin are
        // both left. A division says sixteen readers; the pool slot each of
        // them past the first also takes is what makes it ten.
        let roomy = flagless.resolve_in(&runtime_root("v2-limit"), &source());
        assert_eq!(roomy.parallelism().jobs(), 10);
        assert_eq!(roomy.parallelism().memory_bytes(), Some(10 * BLOCK_READER + 9 * UNIT));
    }

    /// **No limit found leaves the source's recommendation standing**, capped
    /// only by half of `MemAvailable` (`RT8`) — which is the branch a `min`
    /// against the fallback constant would have broken, making a flagless
    /// compressed scan serial on the machine most likely to run it.
    #[test]
    fn no_limit_found_takes_the_sources_own_answer_under_the_memavailable_cap() {
        let flagless = ParallelArgs { jobs: None, parallel_memory: None };

        // ~19 GiB available, so half of it is not the binding number and all
        // twenty-four readers stand.
        let roomy = flagless.resolve_in(&runtime_root("no-limit"), &Recommends::reader(24, READER));
        assert_eq!(roomy.parallelism().jobs(), 24);
        assert_eq!(roomy.parallelism().memory_bytes(), Some(24 * READER));
        assert!(roomy.limit.is_none(), "every limit file states max");

        // 512 MiB available on the same unlimited arrangement: half of it is
        // 256 MiB, which is four readers — the count coming down with the
        // budget rather than being printed beside one it cannot spend.
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
    /// and it is a resolved value rather than an error.** What zero produces
    /// is one reader on the streaming path, out of three floors already in the
    /// mechanism — and the `PlanNote` beside it is what tells the user their
    /// allocation bound the scan
    /// (`pgdump_query/tests/partitioned_replay.rs`,
    /// `a_budget_below_one_readers_worth_says_the_allocation_bound_it`).
    ///
    /// Pinned here so that a later change to the reserve, to the floors, or to
    /// the fit cannot silently make a container under it something else.
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
        // discovery answers, not a ceiling imposed on a person who typed one.
        // The *count* beside it is nobody's statement, so it is cut to what
        // those bytes afford — six readers of 58 MiB inside 400 MiB.
        let stated = ParallelArgs { jobs: None, parallel_memory: Some(400 << 20) };
        let resolved = stated.resolve_in(&root, &Recommends::reader(24, READER));
        assert_eq!(resolved.parallelism().memory_bytes(), Some(400 << 20));
        assert_eq!(resolved.parallelism().jobs(), 6);
    }

    /// **A recommended count answers to the allowance however the budget
    /// arrived.** The rule is scoped to the absence of `--jobs`
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits"), and `--jobs` is absent when only `--parallel-memory` is
    /// typed — so the source's recommendation is cut by a stated budget
    /// exactly as it is by a discovered one, and the run no longer announces a
    /// count it will not deliver.
    ///
    /// **Only the count moves.** The budget is the user's own number and is
    /// taken whole, which is what separates this from discovery, where the
    /// budget handed back is the one the lowered count spends.
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

        // A source recommending no per-worker cost has nothing to divide by,
        // so its count is left where it is.
        let plain = tight.resolve_in(&root, &Recommends::jobs(1));
        assert_eq!(plain.parallelism().jobs(), 1);
        assert_eq!(plain.parallelism().memory_bytes(), Some(32 << 20));
    }

    /// **The check `introspect` owes: the instrument must not move the plan it
    /// reports on.** A counting `#[global_allocator]` and a `mallinfo2` call
    /// at exit change what the process *holds*, which is the point — what they
    /// must not change is what it *resolves*, or every reading describes an
    /// arrangement the shipped binary does not make.
    ///
    /// This is compiled into both configurations against the same literals, so
    /// `cargo test -p pgdump_query-cli` and `cargo test -p pgdump_query-cli
    /// --features introspect` are the two halves of the comparison and neither
    /// can drift without failing. The roots are the committed ones rather than
    /// this machine's `/` (`tests/data/runtime/README.md`), since a resolution
    /// read off the host would differ between the two runs for reasons that
    /// have nothing to do with the instrument.
    ///
    /// It sweeps every root rather than a chosen one: the arms differ in which
    /// term binds — a discovered ceiling, `RT8`'s half-`MemAvailable` cap, the
    /// reserve leaving nothing — and an instrument is exactly the kind of
    /// change that would move one of them and not the others.
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

    /// **`jobs=` reads differently by provenance, and the report says so.** A
    /// recommended count is lowered to what the allowance affords and printed
    /// lowered; a stated `--jobs` is printed as typed, what it actually
    /// delivers staying `stream::worker_count`'s to decide from the budget. So
    /// the same two readers can appear under `jobs=2` and under `jobs=24`, and
    /// only the first line is telling the user what will run.
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
        // Both arrangements hold the same bytes, which is the whole reason the
        // two lines have to read differently.
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

    /// **A plan note names the budget that bound the plan; the clause beside it
    /// names where that budget came from.** The widest of the three notes is a
    /// compressed source declining the block path, which is a throughput cliff
    /// — and under discovery the number to change is the *allocation*, which
    /// the note itself cannot know about (`docs/design/decisions.md`,
    /// "I/O, memory and parallelism").
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

        // No limit found still earns a clause — the budget is the source's own
        // ask, which is equally not the user's.
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
    /// **The branch is the thing being pinned, not the number.** Before
    /// discovery these assertions could name 64 MiB outright; now the answer
    /// depends on the cgroup the test process is in, and a test that named the
    /// constant would pass on a bare host and fail in a container — which is
    /// the environment this default exists for. Every precedence claim below
    /// is asserted against this rather than around it, so the flags' behaviour
    /// stays pinned in both.
    fn flagless_budget() -> Option<u64> {
        pgdump_query::discover_memory_limit().map(|limit| {
            pgdump_query::DEFAULT_MEMORY_BUDGET
                .min(limit.bytes.saturating_sub(pgdump_query::MEMORY_RESERVE))
        })
    }

    /// **Stating neither flag asks the source**, which is the one thing about
    /// `--jobs` no integration test can see: the whole design promise is that a
    /// partitioned run and a serial one produce the same bytes, so nothing in
    /// the output distinguishes them and a default that silently reverted to a
    /// constant would pass every other test in the tree. That drift is exactly
    /// what happened to the measurement harness
    /// (`docs/design/measurements.md`, "The apparatus"), so the wiring is
    /// pinned here rather than left to a doc comment.
    ///
    /// The two counts are the two shipped sources' answers — one for a plain
    /// file, the machine's cores for an `.xz` one — and which source gives
    /// which is `ByteRangeSource::default_workers`'s own test.
    #[test]
    fn stating_no_parallelism_flag_asks_the_source() {
        let budget = flagless_budget();
        let filled = budget.unwrap_or(pgdump_query::DEFAULT_MEMORY_BUDGET);
        let stated = ParallelArgs { jobs: None, parallel_memory: None };

        // A source recommending the serial path gets it, carrying whatever the
        // environment allows — and `None`, which the status line renders
        // `(default)`, exactly where no limit was found to allow anything.
        assert!(stated.resolve(&Recommends::jobs(1)).parallelism().is_serial());
        assert_eq!(stated.resolve(&Recommends::jobs(1)).parallelism().memory_bytes(), budget);

        // A source that recommends more gets it. `Parallelism::Workers` has
        // nowhere to record that nobody stated a budget, so it carries the
        // fallback bare.
        assert_eq!(
            stated.resolve(&Recommends::jobs(8)).parallelism(),
            Parallelism::workers(8, filled)
        );

        // And a stated flag wins outright, over a recommendation in either
        // direction: this pins the precedence rather than the plumbing.
        let asked = ParallelArgs { jobs: Some(8), parallel_memory: None };
        assert_eq!(
            asked.resolve(&Recommends::jobs(1)).parallelism(),
            Parallelism::workers(8, filled)
        );
        let serial = ParallelArgs { jobs: Some(1), parallel_memory: None };
        assert!(serial.resolve(&Recommends::jobs(24)).parallelism().is_serial());
        assert_eq!(serial.resolve(&Recommends::jobs(24)).parallelism().memory_bytes(), budget);
    }

    /// **A stated budget survives a serial worker count.** `--jobs 1` is the
    /// serial path as a property of the value, and the collapse that makes it
    /// one takes the *worker count* down and not the bytes beside it — so
    /// `--parallel-memory` alone is the whole recourse for a compressed file
    /// whose blocks the 64 MiB default cannot hold, with no second worker
    /// needing to be asked for
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

        // Stated explicitly rather than taken from the source: the same value
        // either way, and the source's own recommendation cannot change it.
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
