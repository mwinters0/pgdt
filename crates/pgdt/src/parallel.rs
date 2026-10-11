//! `--jobs` and `--memory`: what a caller states, what a run discovers, and
//! the arrangement each pass resolves to.

use std::path::Path;

use clap::Args;
use pgdt_io::{ByteRangeSource, Parallelism};

/// The two numbers a caller states its parallelism in, shared by the two
/// scanning commands (`docs/design/decisions.md`, "I/O, memory and parallelism").
///
/// **An omitted `--jobs` is the source's own recommendation**, not a constant
/// here: [`ByteRangeSource::default_workers`] answers it, which is why
/// [`Discovered::resolve`] takes the open source
/// (`docs/design/decisions.md`, "D64"). `--jobs 1` is the serial path as a
/// property of `Parallelism::workers` rather than of anything here.
#[derive(Args)]
pub(crate) struct ParallelArgs {
    /// How many workers pgdt may ask for. Left unstated, the file decides: a
    /// plain dump reads serially, and an `.xz` one takes the CPUs this process
    /// was given, or its own block count where that is smaller — lowered again
    /// to the number of readers the memory allowance can pay for, where that is
    /// fewer, whether that allowance was discovered or stated with `--memory`.
    /// A count stated here is never lowered that way.
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
    /// **On `query`, `--memory` has to cover a second cost before this number
    /// is delivered at all** — see that flag's own doc.
    #[arg(long, value_name = "N", value_parser = parse_jobs)]
    jobs: Option<usize>,
    /// How much memory pgdt may hold resident in total, in bytes — the number
    /// you would give the container. Left unstated, pgdt reads the
    /// memory limit it is running under — the smallest cgroup limit that binds
    /// — and uses that; where no limit is set it stays inside half the memory
    /// the machine reports available. A number typed here replaces whatever
    /// was discovered and is carved up exactly the same way.
    ///
    /// **What it is carved into.** A 384 MiB reserve comes off the top for
    /// everything pgdt holds that no buffer accounts for — the runtime's
    /// threads, the allocator's per-thread arenas, the decoder state a
    /// compressed source keeps outside its pools, the binary itself. What is
    /// left is what the read buffers may hold, and the worker count is lowered
    /// again so that a fifth of the whole number, plus a further 256 MiB,
    /// stays unspent. So the buffer
    /// budget a run reports is always smaller than what you typed, and the
    /// `resolved the arrangement` line on stderr names both.
    ///
    /// **What is left under that margin is what a gathering `parse`'s
    /// statistics may hold**, named as `statistics_bytes=` on the same line. A
    /// block whose statistics will not fit it is skipped — the scan finishes,
    /// stderr names the block's table and offset and what it declined under,
    /// the cache records it, and only a larger allowance re-reads it. Nothing is killed for want
    /// of statistics, and raising this is what fits a wide table.
    ///
    /// It is a real bound rather than a target: a `.xz` file that does not
    /// leave room inside the buffer budget for one reader — one of its blocks,
    /// a read buffer and the decompressor's own working memory, beside the
    /// three further blocks the pool keeps however few workers run — is read
    /// through the streaming decoder instead of being decoded a block at a
    /// time, which is correct but slower on backward reads. Raise this to buy
    /// the block path back on a file written with large blocks (`xz -9 -T0`,
    /// `xz --block-size=`).
    ///
    /// **It is also what decides how many of `--jobs`' workers read at once**,
    /// on both commands: the largest count whose whole cost fits inside the
    /// buffer budget. For an ordinary 24 MiB-block `.xz` the first reader is
    /// about 106 MiB and each one past the fourth about 58, so a 64 MiB buffer
    /// budget affords none of them — which is why a discovered allowance is
    /// sized off the count the source recommends rather than off a constant.
    ///
    /// **On `query` the buffer budget divides by two terms rather than one**:
    /// what a worker costs to read, plus the 64 MiB a sub-stream's held batch
    /// may pin (`docs/design/decisions.md`, "I/O, memory and parallelism") —
    /// charged on a plain file, where a batch pins read buffers the first term
    /// never counted, and not on a block-decoding `.xz`, whose first term
    /// already counts a decoded block. At 64 MiB — which is what a plain file
    /// gets unless a limit or this flag says otherwise — the plain file's sum
    /// already exceeds the budget, so rather than decline the readers the
    /// plan narrows each sub-stream's batch span to seat them
    /// (`docs/design/decisions.md`, "D84"): `--jobs N` buys N sub-streams,
    /// bought with batch size rather than with buffers.
    /// `parse` is unaffected by the second term: it builds no batches, so
    /// nothing on that path pins a span.
    #[arg(long, value_name = "BYTES", value_parser = parse_memory)]
    memory: Option<u64>,
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
    /// reason [`pgdt_io::discover_memory_limit_in`] takes one: no machine
    /// is a v1 hierarchy, an unlimited host and an allocation under the
    /// reserve at once (`crates/pgdt/tests/data/runtime/`).
    #[cfg(test)]
    fn resolve_in(&self, root: &Path, source: &dyn ByteRangeSource) -> Resolved {
        self.discover_in(root).resolve(source, 0)
    }

    /// The half of the resolution that needs no dump: the flags as typed, and
    /// the limit this process runs under. A separate step because everything
    /// on this side is true before the file is touched, while opening a fresh
    /// `.xz` walks every stream footer first
    /// (`docs/design/decisions.md`, "D64").
    pub(crate) fn discover(&self) -> Discovered<'_> {
        self.discover_in(Path::new("/"))
    }

    /// [`ParallelArgs::discover`] against an arbitrary filesystem root, for the
    /// same reason `ParallelArgs::resolve_in` takes one.
    fn discover_in<'a>(&'a self, root: &'a Path) -> Discovered<'a> {
        // **Read even where `--memory` was stated**: the mode is a
        // fact about the run, not the flag. The first status line states this
        // read, and the second too where no `--memory` is stated —
        // `memory.high` is writable by whoever set it, so a second walk could
        // answer differently.
        //
        // Deficiency register: `deficiency: KD29` — a flagless read-buffer
        // budget is not taken from this read: each `Discovered::resolve` calls
        // `Parallelism::discover_holding_in`, which walks the limit again —
        // once per pass `query` carves — so a limit rewritten between two
        // walks is announced — and carved into a statistics allowance, which
        // does take this read — as one number while the pools are budgeted
        // from another. **(c) unowned**; promoted
        // by a limit seen to move inside a run, or by a change to
        // `Parallelism`'s discovery that can take a limit already read.
        Discovered { args: self, root, limit: pgdt_io::discover_memory_limit_in(root) }
    }
}

/// What a run knows about its own allowance **before the dump is opened**: the
/// two flags exactly as they were typed, and the memory limit this process is
/// running under with the file that stated it. Nothing here is downstream of
/// the source ([`ParallelArgs::discover`]).
pub(crate) struct Discovered<'a> {
    args: &'a ParallelArgs,
    /// The filesystem root the limit was read under, kept because
    /// `Parallelism::discover_holding_in` asks the same root again, for the limit
    /// (`KD29`) and for what the machine reports free.
    root: &'a Path,
    /// The memory limit this process runs under, and the file that stated it.
    limit: Option<pgdt_io::MemoryLimit>,
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
    /// **The flags are reported as typed here.** Every other status line names
    /// the arrangement in the library's own vocabulary
    /// (`docs/design/decisions.md`, "D64"), except that a stated `--memory` is
    /// quoted beside the budget carved from it ([`Resolved::budget_display`]).
    pub(crate) fn announce(&self) {
        let jobs_flag = Self::flag_display(self.args.jobs.map(|j| j as u64));
        let memory_flag = Self::flag_display(self.args.memory);
        match &self.limit {
            Some(limit) => tracing::info!(
                limit_bytes = limit.bytes,
                limit_read_from = %limit.read_from.display(),
                jobs_flag = %jobs_flag,
                memory_flag = %memory_flag,
                "running inside a stated memory allocation",
            ),
            None => tracing::info!(
                jobs_flag = %jobs_flag,
                memory_flag = %memory_flag,
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
    /// **A stated `--memory` and a discovered limit are the same number to
    /// the library** (`docs/design/decisions.md`, "D83"): the allowance is
    /// typed or read, and `Parallelism::within` carves both the same way, so
    /// the environment's allowance is carved only where the flag is absent. Only a
    /// *recommended* count is lowered to fit (`docs/design/roadmap.md`, "A
    /// default runs as fast as the allocation permits"); either allowance may
    /// put the budget below `DEFAULT_MEMORY_BUDGET`, and what it affords still
    /// binds through `pgdt_io`'s `worker_count`.
    ///
    /// **A carved budget reaches the library at every worker count**, the
    /// serial state carrying one of its own
    /// (`docs/design/decisions.md`, "I/O, memory and parallelism"). Only a
    /// resolved-serial arrangement can state no budget, which is what lets the
    /// status line say `(default)` truthfully: `Workers::memory_bytes` is not
    /// an `Option`, so that variant carries `DEFAULT_MEMORY_BUDGET` bare.
    ///
    /// **`held` is what the pass keeps resident besides its workers while
    /// they are carved**: the heap a cache's statistics hold once a pass loads
    /// them (`CacheClaim::Settles`), billed against the margin's ceiling as the
    /// provider bills a resident map, so it lowers the count first and, where
    /// the count cannot fall, the budget (`Parallelism::within_shared`;
    /// `docs/design/decisions.md`, "D85"). A pass holding none passes `0`
    /// ([`Discovered::resolve_query`]).
    pub(crate) fn resolve(&self, source: &dyn ByteRangeSource, held: u64) -> Resolved {
        let args = self.args;
        // Asked of the source only where `--jobs` was absent: a stated count
        // is not a recommendation and has nothing to be lowered from.
        let recommended_jobs = match args.jobs {
            Some(_) => None,
            None => Some(source.default_workers()),
        };
        let jobs = args.jobs.or(recommended_jobs).unwrap_or(1);
        // One carving, whichever end the allowance came from: a stated
        // `--memory` replaces the discovered limit rather than bypassing the
        // reserve and the margin.
        let memory = source.default_worker_memory();
        let carved = match args.memory {
            Some(stated) => Parallelism::within_shared(jobs, memory, stated, 0, held),
            None => Parallelism::discover_holding_in(self.root, jobs, memory, held),
        };
        let parallelism = match (args.jobs, carved.memory_bytes()) {
            // A stated count is not lowered by the allowance; what the budget
            // delivers still binds through `pgdt_io`'s `worker_count`.
            (Some(stated), Some(bytes)) => Parallelism::workers(stated, bytes),
            _ => carved,
        };
        // **The allowance, whichever end it came from** — and with neither, a
        // machine that has said nothing is read at half of what it reports
        // available, the number "A default runs as fast as the allocation
        // permits" already uses (`RT8`). `MemAvailable` unreadable leaves no
        // allowance at all, which declines nothing
        // (`docs/design/decisions.md`, "D85").
        let allowance = args
            .memory
            .or_else(|| self.limit.as_ref().map(|limit| limit.bytes))
            .or_else(|| pgdt_io::available_memory_in(self.root).map(|free| free / 2));
        Resolved {
            parallelism,
            limit: self.limit.clone(),
            allowance_stated: args.memory,
            allowance,
            recommended_jobs,
        }
    }

    /// **`query`'s two passes, carved apart**: each from the one allowance
    /// with nothing drawn, since they never run at once, and each billed what
    /// it holds. The mapping pass holds the loaded map while its readers
    /// extend it, so it is carved around `cached_statistics`; the replay holds
    /// none of them — the library's segments drop a block's statistics, and
    /// the map is gone before a row is read — so it is carved around nothing.
    /// Billing the replay the blocks it matched instead would need them known
    /// before the mapping pass has found them.
    pub(crate) fn resolve_query(
        &self,
        source: &dyn ByteRangeSource,
        cached_statistics: u64,
    ) -> QueryPasses {
        QueryPasses {
            mapping: self.resolve(source, cached_statistics),
            replay: self.resolve(source, 0),
            held: cached_statistics,
        }
    }
}

/// The arrangement each of `query`'s passes runs under
/// ([`Discovered::resolve_query`]).
pub(crate) struct QueryPasses {
    /// The mapping pass's, in [`pgdt_dump::ScanOptions::parallelism`]: its count and
    /// budget are on its own `scan started` line, printed only when it reads,
    /// inside the span [`QueryPasses::mapping_span`] opens.
    pub(crate) mapping: Resolved,
    /// The replay's, in [`pgdt_query::QueryOptions::parallelism`]: what `resolved the
    /// arrangement` announces and a plan note's origin clause quotes, the
    /// only one that reads over a map already settling the table.
    pub(crate) replay: Resolved,
    /// The heap the cache's statistics hold once loaded, which the mapping
    /// pass was carved around and the replay was not.
    held: u64,
}

impl QueryPasses {
    /// The mapping pass's count and the replay's, where the statistics the
    /// mapping pass holds cut the first below the second. The two passes are
    /// carved from one allowance and differ in `held`, and, with no `--memory`,
    /// in what each pass's own read of the limit and of `MemAvailable` returns
    /// (KD29's walk); a stated `--jobs` is never lowered, and
    /// only its budget can differ.
    fn mapping_shortfall(&self) -> Option<(usize, usize)> {
        let (jobs, asked) = (self.mapping.parallelism().jobs(), self.replay.parallelism().jobs());
        (jobs < asked).then_some((jobs, asked))
    }

    /// The span the mapping pass's own lines — `scan started`, `scan
    /// arrangement`, `scan complete` — are said inside, so each carries what
    /// the pass was carved around: `held_bytes`, and `replay_jobs`, the count
    /// `resolved the arrangement` named, where the pass runs fewer. Its
    /// presence is the cause, so no `bound_by=` rides it
    /// (`docs/design/decisions.md`, "D64", "D85").
    ///
    /// **Opened whenever the cache holds statistics**, not only where they cut
    /// the count: the pass's budget can then differ from the replay's, stated
    /// `--jobs` or not. It prefixes only what the pass emits, so a cache that
    /// already settles the table, whose query maps nothing, prints none of it.
    pub(crate) fn mapping_span(&self) -> tracing::Span {
        if self.held == 0 {
            return tracing::Span::none();
        }
        let span = tracing::info_span!(
            "mapping",
            held_bytes = self.held,
            replay_jobs = tracing::field::Empty,
        );
        if let Some((_, replay_jobs)) = self.mapping_shortfall() {
            span.record("replay_jobs", replay_jobs);
        }
        span
    }
}

/// What a run resolved its two parallelism numbers to, and where each came
/// from — the arrangement plus the provenance [`Parallelism`] has nowhere to
/// carry. **Provenance is the CLI's fact, not the library's**, and cannot be
/// recovered from a [`Parallelism`] downstream
/// (`docs/design/decisions.md`, "D64").
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    /// The arrangement the library is handed.
    parallelism: Parallelism,
    /// The memory limit this process runs under, and the file that stated it;
    /// `None` means no limit is being *enforced*.
    limit: Option<pgdt_io::MemoryLimit>,
    /// The resident allowance `--memory` stated, or `None` where it was not
    /// given — in which case the allowance is the discovered limit, else half
    /// of what the machine reports available, or nothing.
    allowance_stated: Option<u64>,
    /// **The resident allowance in force**, whichever end it came from:
    /// `--memory`, else the discovered limit, else half of what the machine
    /// reports available (`RT8`). `None` is a host that has said nothing and
    /// whose `MemAvailable` could not be read, under which nothing declines
    /// (`docs/design/decisions.md`, "D85").
    allowance: Option<u64>,
    /// What the source recommended for a worker count, or `None` where
    /// `--jobs` was stated — in which case the count is that flag's, unlowered.
    recommended_jobs: Option<usize>,
}

impl Resolved {
    pub(crate) fn parallelism(&self) -> Parallelism {
        self.parallelism
    }

    /// The worker count and its provenance, for a status line.
    ///
    /// **A recommended count reads differently from a stated one**: a
    /// recommendation is printed lowered to what the allowance affords, a
    /// stated `--jobs` as typed. What did the lowering is named — `by the
    /// allocation` or `by the stated allowance`, those being different numbers
    /// to change — with [`Resolved::budget_display`] beside it naming the number
    /// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
    /// permits").
    fn jobs_display(&self) -> String {
        let jobs = self.parallelism.jobs();
        match self.recommended_jobs {
            None => format!("{jobs} (stated)"),
            Some(asked) if asked > jobs => {
                let by = if self.allowance_stated.is_some() {
                    "the stated allowance"
                } else {
                    "the allocation"
                };
                format!("{jobs} (recommended by the source; lowered from {asked} by {by})")
            }
            Some(_) => format!("{jobs} (recommended by the source)"),
        }
    }

    /// The **read-buffer budget** and where the allowance behind it came
    /// from.
    ///
    /// **The number here is never the number typed**, `--memory` being a
    /// resident allowance the reserve and the margin are carved out of
    /// (`docs/design/decisions.md`, "D83"), so a stated allowance is named
    /// beside the budget exactly as a discovered limit is rather than being
    /// abbreviated to `(stated)`.
    ///
    /// Four spellings: the flag; a discovered limit, named by the file that
    /// stated it (`memory.high` throttles where `memory.max` kills, and either
    /// may be an ancestor's); the source's own recommendation; and the
    /// library's constant, which is what "no limit found" leaves a source that
    /// recommends nothing, spelled so where that leaves the arrangement serial.
    ///
    /// Deficiency register: `deficiency: KD50` — two more ways to a number
    /// are spelled as the source's recommendation, both on a host stating no
    /// limit. Half of `MemAvailable`: a source asking for 24 readers, cut to
    /// 4, prints a budget "what this source asks for" and a count "lowered …
    /// by the allocation" beside a first line saying nothing enforces one.
    /// And the library's constant under a stated `--jobs` past one over a
    /// source recommending nothing, which the library carries as a budget
    /// rather than as none. **(c) unowned**; closing it means a spelling of
    /// each one's own, on this line and on [`Resolved::jobs_display`]'s.
    fn budget_display(&self) -> String {
        let bytes = self.parallelism.memory_bytes().unwrap_or(pgdt_io::DEFAULT_MEMORY_BUDGET);
        if let Some(allowance) = self.allowance_stated {
            return format!("{bytes} (stated: --memory allows {allowance} resident byte(s))");
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

    /// The clause a budget-quoting plan note carries — the half of the story
    /// the library cannot tell. A [`pgdt_query::PlanNote`] that names the
    /// budget which bound the plan
    /// ([`pgdt_query::PlanNote::budget_bytes`]) gets where that number came
    /// from appended: the same [`Resolved::budget_display`] the mode report
    /// prints (`docs/design/decisions.md`, "D64"). One that names no budget —
    /// a statistics skip — takes no clause, there being no number to source.
    ///
    /// **Printed whichever end the allowance came from.** A note names the
    /// budget that bound the plan, and under `--memory` that is a carved
    /// number rather than the one typed, so the clause is what connects the
    /// two (`docs/design/decisions.md`, "D83").
    pub(crate) fn plan_note_origin(&self) -> String {
        format!(" — the budget in force is {}", self.budget_display())
    }

    /// **What this run's statistics may hold**, carved from the allowance in
    /// force after the workers: what the margin leaves once the read-buffer
    /// budget is spent (`docs/design/decisions.md`, "D85"). `None` — no flag,
    /// no limit and no `MemAvailable` — declines nothing.
    pub(crate) fn statistics_allowance(&self) -> Option<u64> {
        let budget = self.parallelism.memory_bytes().unwrap_or(pgdt_io::DEFAULT_MEMORY_BUDGET);
        self.allowance.map(|allowance| pgdt_io::statistics_allowance(allowance, budget))
    }

    /// The statistics allowance and where the number behind it came from,
    /// worded as [`Resolved::budget_display`] words the buffer budget.
    fn statistics_allowance_display(&self) -> String {
        let Some(bytes) = self.statistics_allowance() else {
            return "(none: no limit found and the machine's available memory could not be read)"
                .to_string();
        };
        let origin = match (self.allowance_stated, &self.limit) {
            (Some(_), _) => "stated",
            (None, Some(_)) => "discovered",
            (None, None) => "half of what the machine reports available",
        };
        format!("{bytes} ({origin})")
    }

    /// Say, once per scanning command and before the scan starts, what the
    /// source's recommendation and the allowance fitted to.
    ///
    /// **The line that names the count the allowance lowered** — a query's
    /// mapping pass, carved around its cached statistics, says what cut its
    /// count on its own lines' span ([`QueryPasses::mapping_span`]), and a
    /// leader's shortfall is `scan arrangement`'s `bound_by=` — which is why
    /// the report is two lines: neither number
    /// exists until the file has been opened and asked
    /// ([`Discovered::announce`] carries the half that does). A recommendation
    /// cut to fit otherwise surfaces as unexplained slowness — with no limit
    /// found pgdt stays inside half of what the kernel says is available
    /// (`RT8`)
    /// (`docs/design/decisions.md`, "D64").
    pub(crate) fn announce(&self) {
        tracing::info!(
            jobs = %self.jobs_display(),
            memory_bytes = %self.budget_display(),
            statistics_bytes = %self.statistics_allowance_display(),
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

/// A `--memory` value: a byte count, and never zero. Zero is not an allowance
/// a process can run inside at all.
///
/// **Anything at or below `MEMORY_RESERVE` is accepted and carves to a budget
/// of zero**, which the floors inside the mechanism turn into one reader on the
/// streaming path — a refusal there would be this crate deciding how small an
/// allocation may be (`docs/design/decisions.md`, "D83").
fn parse_memory(text: &str) -> std::result::Result<u64, String> {
    match text.parse::<u64>() {
        Ok(0) => Err("a memory allowance of 0 leaves no room to run in".into()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests;
