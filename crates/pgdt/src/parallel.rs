//! `--jobs` and `--memory`: what a caller states, what a run discovers, and
//! the arrangement each pass resolves to.

use clap::Args;
use pgdt_io::{Allowance, Arrangement, ArrangementRequest, ByteRangeSource, Parallelism};

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
    /// plain file the read buffer pool's own depth binds before this does: it
    /// holds at most four chunks, and a `parse` above `--jobs 4` queues the
    /// readers past them wherever a read may wait.
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
    // Kept out of the help below: `docs/design/decisions.md`, "I/O, memory
    // and parallelism" and "D84".
    /// How much memory pgdt may hold resident in total, in bytes — the number
    /// you would give the container. Left unstated, pgdt reads the
    /// memory limit it is running under — the smallest cgroup limit that binds
    /// — and uses that; where no limit is set it stays inside half the memory
    /// the machine reports available. A number typed here replaces whatever
    /// was discovered and is carved up exactly as a discovered limit is.
    ///
    /// **What it is carved into.** A 384 MiB reserve comes off the top for
    /// everything pgdt holds that no buffer accounts for — the runtime's
    /// threads, the allocator's per-thread arenas, the decoder state a
    /// compressed source keeps outside its pools, the binary itself. What is
    /// left is what the read buffers may hold, and the buffer budget is what
    /// as many workers spend as leave a fifth of the whole number, plus a
    /// further 256 MiB, unspent — the worker count itself lowered to that
    /// where the source states a charge and `--jobs` did not. So the buffer
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
    /// may pin — charged on a plain file, where a batch pins read buffers the
    /// first term never counted, and not on a block-decoding `.xz`, whose
    /// first term already counts a decoded block. At 64 MiB — which is what a
    /// plain file gets unless a limit or this flag says otherwise — the plain
    /// file's sum already exceeds the budget, so rather than decline the
    /// readers the plan narrows each sub-stream's batch span to seat them:
    /// `--jobs N` buys N sub-streams, bought with batch size rather than with
    /// buffers.
    /// `parse` is unaffected by the second term: it builds no batches, so
    /// nothing on that path pins a span.
    #[arg(long, value_name = "BYTES", value_parser = parse_memory)]
    memory: Option<u64>,
}

impl ParallelArgs {
    /// [`Discovered::resolve`] over the allowance this process runs under,
    /// composing both halves in one call — what a test wants and production
    /// cannot use, a run having a line to print between them.
    #[cfg(test)]
    fn resolve(&self, source: &dyn ByteRangeSource) -> Resolved {
        self.discover().resolve(source, 0)
    }

    /// [`ParallelArgs::resolve`] as though discovery had found `found`: no
    /// machine is a v1 hierarchy, an unlimited host and an allocation under
    /// the reserve at once, and what each finds is `pgdt_io`'s to pin
    /// (`crates/pgdt-io/tests/data/runtime/`).
    #[cfg(test)]
    fn resolve_under(&self, found: Allowance, source: &dyn ByteRangeSource) -> Resolved {
        self.found(found).resolve(source, 0)
    }

    /// The half of the resolution that needs no dump: the flags as typed, and
    /// the allowance this process runs under. A separate step because
    /// everything on this side is true before the file is touched, while
    /// opening a fresh `.xz` walks every stream footer first
    /// (`docs/design/decisions.md`, "D64").
    ///
    /// **Read once, even where `--memory` was stated**: the mode is a fact
    /// about the run, not the flag, and every pass the run carves resolves
    /// from this one reading, so the limit announced is the limit carved.
    pub(crate) fn discover(&self) -> Discovered<'_> {
        self.found(Allowance::discover())
    }

    /// What a run knows once discovery has found `found`.
    fn found(&self, found: Allowance) -> Discovered<'_> {
        Discovered { args: self, found }
    }
}

/// What a run knows about its own allowance **before the dump is opened**: the
/// two flags exactly as they were typed, and the allowance this process runs
/// under — a memory limit with the file that stated it, or half of what the
/// machine reports available. Nothing here is downstream of the source
/// ([`ParallelArgs::discover`]).
pub(crate) struct Discovered<'a> {
    args: &'a ParallelArgs,
    /// What discovery found, whether or not `--memory` replaces it.
    found: Allowance,
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
        match &self.found {
            Allowance::Limit(limit) => tracing::info!(
                limit_bytes = limit.bytes,
                limit_read_from = %limit.read_from.display(),
                jobs_flag = %jobs_flag,
                memory_flag = %memory_flag,
                "running inside a stated memory allocation",
            ),
            _ => tracing::info!(
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
    /// typed or read, and `pgdt_io::Arrangement::resolve` carves both the same
    /// way, so the environment's allowance is carved only where the flag is
    /// absent. Only a
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
    /// them — the opened cache's index — billed
    /// against the margin's ceiling as the provider bills a resident map, so
    /// it lowers the count first and, where the count cannot fall, the budget,
    /// under an allowance carved with a margin — half of what the machine
    /// reports available is not, and holds nothing off (`pgdt_io::ArrangementRequest::held`;
    /// `docs/design/decisions.md`, "D85"). A pass holding none passes `0`
    /// ([`Discovered::resolve_pass`]).
    pub(crate) fn resolve(&self, source: &dyn ByteRangeSource, held: u64) -> Resolved {
        // A stated `--memory` replaces the discovered allowance rather than
        // bypassing the reserve and the margin: one carving, whichever end
        // the allowance came from. With neither, a machine that has said
        // nothing is read at half of what it reports available (`RT8`), and
        // `MemAvailable` unreadable leaves no allowance at all, which declines
        // nothing (`docs/design/decisions.md`, "D85").
        let allowance = self.args.memory.map_or_else(|| self.found.clone(), Allowance::Stated);
        let request = ArrangementRequest { held, ..ArrangementRequest::of(source, self.args.jobs) };
        Resolved(Arrangement::resolve(&allowance, request))
    }

    /// **`query`'s passes, each carved around what it holds**: the mapping
    /// pass around the statistics a loaded cache hands it, `held` its index's
    /// heap, and the replay — carved only once that pass has settled the
    /// table — around the settled table's, which the table and its plan keep
    /// (`pgdt_query::TablePartitions::under`). Each from the one allowance
    /// with nothing drawn, since the two never run at once and the map is
    /// gone before a row is read (`docs/design/decisions.md`, "D85").
    pub(crate) fn resolve_pass(&self, source: &dyn ByteRangeSource, held: u64) -> QueryPass {
        QueryPass { resolved: self.resolve(source, held), held }
    }
}

/// One of `query`'s passes, the arrangement it runs under beside the
/// statistics heap it was carved around ([`Discovered::resolve_pass`]).
pub(crate) struct QueryPass {
    /// The mapping pass's in [`pgdt_dump::ScanOptions::parallelism`], its
    /// count and budget on its own `scan started` line; the replay's in
    /// [`pgdt_query::QueryOptions::parallelism`], which `resolved the
    /// arrangement` announces and a plan note's origin clause quotes.
    pub(crate) resolved: Resolved,
    /// The statistics heap the pass holds while it runs.
    held: u64,
}

impl QueryPass {
    /// The span the mapping pass's own lines — `scan started`, `scan
    /// arrangement`, `scan complete` — are said inside, carrying
    /// `held_bytes`, what the pass was carved around. Its presence is the
    /// cause, so no `bound_by=` rides it (`docs/design/decisions.md`, "D64",
    /// "D85").
    ///
    /// **Opened whenever the cache holds statistics**, not only where they
    /// cut the count: the pass's budget can then differ from what it would
    /// be holding none, stated `--jobs` or not. It prefixes only what the
    /// pass emits, so a cache that already settles the table, whose query
    /// maps nothing, prints none of it.
    pub(crate) fn mapping_span(&self) -> tracing::Span {
        if self.held == 0 {
            return tracing::Span::none();
        }
        tracing::info_span!("mapping", held_bytes = self.held)
    }

    /// The replay's `resolved the arrangement`, said once the mapping pass
    /// has settled the table and the replay is carved: [`Resolved::announce`],
    /// with `held_bytes=` where the settled table holds statistics. Said once,
    /// after the pass: before it the line would name a count the held
    /// statistics may lower, and the pass's own carve is printed where the
    /// pass maps nothing.
    pub(crate) fn announce_replay(&self) {
        self.resolved.announce_holding(self.held);
    }
}

/// What a run resolved its two parallelism numbers to, and where each came
/// from — the library's [`Arrangement`], spelled for `pgdt`'s status lines.
/// **Provenance never enters [`Parallelism`]** and cannot be recovered from
/// one downstream (`docs/design/decisions.md`, "D64"); the arrangement carries
/// it beside.
#[derive(Debug, Clone)]
pub(crate) struct Resolved(Arrangement);

impl Resolved {
    pub(crate) fn parallelism(&self) -> Parallelism {
        self.0.parallelism()
    }

    /// The library's arrangement itself, which a gathering pass takes whole
    /// so its parallelism and its statistics allowance stay one carve
    /// (`pgdt_mapping::map_file`).
    pub(crate) fn arrangement(&self) -> &Arrangement {
        &self.0
    }

    /// Whether the allowance was typed rather than discovered.
    fn stated(&self) -> bool {
        matches!(self.0.allowance(), Allowance::Stated(_) | Allowance::Setting(_))
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
        let jobs = self.parallelism().jobs();
        match self.0.recommended_jobs() {
            None => format!("{jobs} (stated)"),
            Some(asked) if asked > jobs => {
                let by = if self.stated() { "the stated allowance" } else { "the allocation" };
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
        let parallelism = self.parallelism();
        let bytes = parallelism.budget_bytes();
        match (self.0.allowance(), parallelism.memory_bytes()) {
            (Allowance::Stated(allowance) | Allowance::Setting(allowance), _) => {
                format!("{bytes} (stated: --memory allows {allowance} resident byte(s))")
            }
            (Allowance::Limit(limit), _) => format!(
                "{bytes} (discovered: {} states a limit of {} byte(s))",
                limit.read_from.display(),
                limit.bytes
            ),
            (_, Some(_)) => format!("{bytes} (no limit found: what this source asks for)"),
            (_, None) => format!("{bytes} (default: no limit found)"),
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
    /// force after the workers ([`Arrangement::statistics_allowance`]). `None`
    /// — no flag, no limit and no `MemAvailable` — declines nothing.
    pub(crate) fn statistics_allowance(&self) -> Option<u64> {
        self.0.statistics_allowance()
    }

    /// The statistics allowance and where the number behind it came from,
    /// worded as [`Resolved::budget_display`] words the buffer budget.
    fn statistics_allowance_display(&self) -> String {
        let Some(bytes) = self.statistics_allowance() else {
            return "(none: no limit found and the machine's available memory could not be read)"
                .to_string();
        };
        let origin = match self.0.allowance() {
            Allowance::Stated(_) | Allowance::Setting(_) => "stated",
            Allowance::Limit(_) => "discovered",
            Allowance::HalfAvailable(_) | Allowance::NoneFound => {
                "half of what the machine reports available"
            }
        };
        format!("{bytes} ({origin})")
    }

    /// Say, once per scanning command and before the pass it arranges
    /// starts, what the source's recommendation and the allowance fitted to
    /// — `query`'s replay's once its mapping pass has settled the table
    /// ([`QueryPass::announce_replay`]).
    ///
    /// **The line that names the count the allowance lowered** — a query's
    /// mapping pass, carved around its cached statistics, says what it was
    /// carved around on its own lines' span ([`QueryPass::mapping_span`]), and a
    /// leader's shortfall is `scan arrangement`'s `bound_by=` — which is why
    /// the report is two lines: neither number
    /// exists until the file has been opened and asked
    /// ([`Discovered::announce`] carries the half that does). A recommendation
    /// cut to fit otherwise surfaces as unexplained slowness — with no limit
    /// found pgdt stays inside half of what the kernel says is available
    /// (`RT8`)
    /// (`docs/design/decisions.md`, "D64").
    pub(crate) fn announce(&self) {
        self.announce_holding(0);
    }

    /// [`Resolved::announce`], naming `held` as `held_bytes=` where the pass
    /// was carved around statistics it holds.
    fn announce_holding(&self, held: u64) {
        let (jobs, memory_bytes) = (self.jobs_display(), self.budget_display());
        let statistics_bytes = self.statistics_allowance_display();
        if held == 0 {
            tracing::info!(%jobs, %memory_bytes, %statistics_bytes, "resolved the arrangement");
        } else {
            tracing::info!(
                %jobs,
                %memory_bytes,
                %statistics_bytes,
                held_bytes = held,
                "resolved the arrangement",
            );
        }
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
