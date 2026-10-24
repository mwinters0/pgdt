//! What a query's plan settles, said to its caller: each [`PlanNote`], the
//! settings it names, and the blocks an early stop left unread.

use pgdt_codec::compare::ComparisonSemantics;
use pgdt_model::{Finding, Severity};

/// One fact about *this query's plan*, as opposed to a fact about the file
/// ([`pgdt_model::DiagnosticKind`], the model), about one column
/// (`pgdt_codec::ResolvedSchema::notes`, the codec) or about one predicate
/// term ([`ComparisonNote`](crate::predicate::ComparisonNote), query) — a fourth channel, deliberately not a widening of
/// any of the other three (`docs/design/decisions.md`, "D19"), and a
/// [`Finding`] like each of them, so one sink drains all four. See
/// [`TableStream::plan_notes`](crate::stream::table::TableStream::plan_notes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanNote {
    pub kind: PlanNoteKind,
    /// What a caller could change that would move what this note reports,
    /// **for this source and this budget** — empty for a note naming no
    /// budget. Computed where the plan reads the source's cost and the budget
    /// it was carved, so a caller mapping each lever onto its own setting
    /// names none that is inert here without re-deriving either
    /// ([`PlanLever`]).
    ///
    /// **A subset of what the sentence names, never more**: the message
    /// offers a lever wherever it can apply and says where it cannot, as
    /// [`PlanNoteKind::ParallelismBudgetLimited`]'s does of the budget on a
    /// plain source, while this lists only the ones that apply.
    pub levers: Vec<PlanLever>,
}

/// One of the settings a caller holds that a [`PlanNote`] can name, in the
/// library's vocabulary and in the order [`PlanNote::levers`] lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlanLever {
    /// A larger resident allowance, carved as [`Arrangement::resolve`](pgdt_io::Arrangement::resolve) carves
    /// one — listed only where it would raise the budget the note quotes
    /// ([`Parallelism::allowance_raises_budget`](pgdt_io::Parallelism::allowance_raises_budget)), which on a source
    /// recommending no per-reader cost stops at
    /// `pgdt_io::DEFAULT_MEMORY_BUDGET`.
    LargerAllowance,
    /// A smaller read chunk (`pgdt_dump::ReadOptions::chunk_size_bytes`) —
    /// listed only where the source sizes a reader from it
    /// ([`pgdt_io::Partitioning::sized_by_read_chunk`]) or the batch span
    /// the plan charged sits on the chunk floor (`derived_source_span`),
    /// never for a span stated below the chunk, which is charged as stated.
    SmallerReadChunk,
    /// Fewer sub-streams asked for (`Parallelism::jobs`), which leaves each a
    /// larger batch ([`PlanNoteKind::BatchSpanNarrowed`]).
    FewerSubStreams,
}

/// What a [`PlanNote`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanNoteKind {
    /// `--jobs` asked for more concurrent sub-streams than the stated memory
    /// budget affords, counting both terms of the charge [`worker_count`](pgdt_io::internal::worker_count)
    /// solves against: `footprint`, the widest touched block's decode cost
    /// (`pgdt_io::Partitioning::partition_bytes`), and `max_source_span`
    /// (`crate::options::QueryOptions::max_source_span`) — `None` where the
    /// caller left the span unbounded or the source retains by the partition
    /// (`pgdt_io::RetainedUnit`). Never a reason to refuse the query:
    /// `planned` sub-streams run regardless.
    ///
    /// **`max_source_span` here is what was charged, not what was stated**:
    /// the span is derived down before the count is cut
    /// (`derived_source_span`), so this note beside a
    /// [`PlanNoteKind::BatchSpanNarrowed`] names that smaller number and the
    /// span is not among the levers left.
    ///
    /// **Both levers that are left are named, because neither always works**
    /// (`KD32`, as [`PlanNoteKind::BatchSpanNarrowed`] states it too, and `KD41`):
    /// a source recommending no per-reader cost is left on
    /// `pgdt_io::DEFAULT_MEMORY_BUDGET` however large an allowance is stated
    /// (`docs/design/decisions.md`, "D83"), so a message offering the budget
    /// alone is inert exactly where this fires on a plain one. The other is
    /// the announced read chunk
    /// (`pgdt_dump::ReadOptions::chunk_size_bytes`), which a replay plan
    /// never announces (`KD41`), and which a source cutting by it sizes
    /// `footprint` from, a fixed multiple of it
    /// (`pgdt_io::Partitioning::partition_bytes`), and which floors the
    /// span unless the span was stated below it (`derived_source_span`).
    /// The arm with no span names it with that condition; **the span arm
    /// names it only where [`PlanNote::levers`] lists it**, where it moves
    /// one of the two terms, and is otherwise left with the budget alone.
    ///
    /// **What a seat buys is not said here**: whether the sub-streams seated
    /// run concurrently is decided by how the caller drains them, which a plan
    /// cannot see, so it is the front end's to say (`pgdt query`'s merge,
    /// `KD57`).
    ParallelismBudgetLimited {
        requested: usize,
        planned: usize,
        footprint: u64,
        max_source_span: Option<u64>,
        memory_bytes: u64,
    },
    /// The compressed source this query reads declined the block-decode
    /// path: the memory budget in force does not afford one block-decoding
    /// reader, so it reads each unit — an `.xz` block, the span from one gzip
    /// checkpoint to the next — **in pieces**, through the *streaming
    /// decoder* this note and the manual name, one live forward decode behind
    /// a mutex, and every **backward** read decodes forward from its unit's
    /// start (`docs/design/decisions.md`, "D15").
    /// Never a reason to refuse the query, though the forward mapping pass
    /// pays too: a declined source advises a single partition and so reads
    /// serially whatever `--jobs` says.
    ///
    /// `codec` names the container (`pgdt_io::CompressionIndex::codec`), whose
    /// words the sentence speaks; `max_block_uncompressed` is the file's
    /// largest unit, `block_count` says how much seeking the file would
    /// otherwise offer, and `reader_bytes` is what the budget was compared
    /// against — stated by the source rather than derived here
    /// ([`pgdt_io::ByteRangeSource::block_decode_bytes`]).
    ///
    /// **Not a [`pgdt_model::DiagnosticKind`]**, for the reason its
    /// sibling above is not one (`docs/design/decisions.md`, "D19").
    CompressedBlockPathDeclined {
        codec: &'static str,
        block_count: usize,
        max_block_uncompressed: u64,
        reader_bytes: u64,
        memory_bytes: u64,
    },
    /// The compressed source this query reads has no complete index yet —
    /// a gzip file whose first pass is still under way, building the index
    /// as it reads — so it is read **serially**, one forward decode, whatever
    /// `--jobs` says, until a read reaches the file's end; from then on the
    /// index it built cuts its reads at checkpoints, as does every later run
    /// over the cache a save of it writes.
    ///
    /// `codec` names the container.
    ///
    /// **A note, not a [`pgdt_model::DiagnosticKind`]** (`docs/design/decisions.md`,
    /// "D19"): it is a property of this run, the pass making the file
    /// seekable and nothing the caller does making it so sooner, so it names
    /// no lever and no budget.
    CompressedFirstPass { codec: &'static str },
    /// The budget in force affords less than a **single** reader of this
    /// source, so the plan runs at the one-slot floors already inside the
    /// mechanism (`pgdt_io::WorkerMemory::affords`' one-worker floor,
    /// `pgdt_io::BufferPool`'s clamp to one slot, and a compressed source's
    /// refusal to decode a whole block).
    ///
    /// **It is the arrangement below the reserve, named rather than left to
    /// emerge** (`docs/design/decisions.md`, "D3"): a memory limit at or under
    /// `pgdt_io::internal::MEMORY_RESERVE` leaves the budget at zero, and
    /// [`PlanNoteKind::ParallelismBudgetLimited`] fires only where `requested`
    /// exceeds what was planned, which at one worker is never.
    ///
    /// `unit_bytes` is `pgdt_io::Partitioning::partition_bytes` and
    /// `memory_bytes` the budget it was compared against. The **span** term is
    /// deliberately not added: a plain `query` at the default budget already
    /// exceeds it once a batch's pin is counted. Never a reason to refuse
    /// anything.
    ///
    /// **It names its levers as its siblings do**: a larger memory budget
    /// always, with their caveat that a larger allowance is not one on a
    /// source recommending no per-reader cost (`KD32`), and a smaller read
    /// chunk only where [`PlanNote::levers`] lists one — where the source
    /// sizes a reader from the chunk and the budget is above zero, a budget of
    /// zero being below every reader however small its chunk. On a plain
    /// source the three arrangements differ: a zero budget moves with memory
    /// alone, one between zero and a reader with memory or the chunk, and one
    /// at `pgdt_io::DEFAULT_MEMORY_BUDGET` under a chunk past it with the
    /// chunk alone.
    AllocationBelowFloor { unit_bytes: u64, memory_bytes: u64 },
    /// The batch span this query stated
    /// (`crate::options::QueryOptions::max_source_span`) did not leave room for
    /// the workers it asked for, so the plan charged `planned_bytes` instead
    /// of `stated_bytes` and the sub-streams' batches are that much smaller
    /// (`derived_source_span`; `docs/design/decisions.md`, "D84").
    ///
    /// **Not a shortfall in itself**: `workers` is what the narrowed span did
    /// buy, and where it still came up short of what was asked for a
    /// [`PlanNoteKind::ParallelismBudgetLimited`] says so beside this. A span
    /// at one announced read chunk
    /// (`pgdt_dump::ReadOptions::chunk_size_bytes`) is the floor, and what
    /// moves a span off it is what `derived_source_span` divides: a larger
    /// `memory_bytes`, a smaller `Parallelism::jobs`, or a smaller charge per
    /// reader — which a source sizing its reader from the read chunk
    /// ([`pgdt_io::Partitioning::sized_by_read_chunk`]) lowers with the
    /// chunk.
    ///
    /// **The sub-streams and the budget are always named, because only the
    /// first is always the caller's** (`KD32`): a source recommending no
    /// per-reader cost is left on `pgdt_io::DEFAULT_MEMORY_BUDGET` however
    /// large an allowance is stated (`docs/design/decisions.md`, "D83"), and
    /// which end this run's `memory_bytes` came from is a caller's own fact to
    /// add ([`PlanNote::budget_bytes`]; `docs/design/decisions.md`, "D64").
    /// **The chunk is named only where [`PlanNote::levers`] lists it**, in
    /// [`PlanNoteKind::AllocationBelowFloor`]'s clause.
    ///
    /// **Not a [`pgdt_model::DiagnosticKind`]**, for the reason its
    /// siblings are not (`docs/design/decisions.md`, "D19").
    BatchSpanNarrowed { stated_bytes: u64, planned_bytes: u64, workers: usize, memory_bytes: u64 },
    /// The row-group statistics a mapping pass stored proved that
    /// `skipped_groups` of the `groups` listed by the blocks carrying them
    /// hold no row the filter keeps, so their `skipped_bytes` of rows are
    /// never parsed — read at most as the tail of a kept run's last chunk —
    /// out of the `bytes` of rows every matched block holds
    /// (`crate::options::QueryOptions::use_statistics`). Stated wherever a
    /// block's statistics were consulted, a skip of nothing included; never
    /// where none were, nor where the filter keeps every row
    /// (`docs/design/decisions.md`, "D19").
    ///
    /// **A block is consulted by holding statistics that fit it**, not by
    /// holding a usable one for a column the filter reads: a zero also covers
    /// a filtered column left out of the gathered selection, one whose recorded
    /// declared type or collation no longer matches, and one holding only NULL
    /// counts under an operator they cannot answer.
    StatisticsPruned { skipped_groups: u64, groups: u64, skipped_bytes: u64, bytes: u64 },
    /// A column this query materializes holds `values` values PostgreSQL
    /// accepts for its declared type and its type cannot hold, which the
    /// query reads as NULL ([`pgdt_codec::UnrepresentableMode::Null`];
    /// `docs/design/decisions.md`, "D98"). One per such column.
    ///
    /// **A property of the table, never of the run**: `values` is the map's
    /// count over every block of the table, in the tiers the query's semantics
    /// cannot hold, so it does not move with a `LIMIT`, a pruned group or a
    /// dynamic filter's skip, which decide only how many of them a run met.
    ///
    /// The sentence names the filter term that tells those values from the
    /// NULLs the dump holds, in the spelling of the front end `semantics`
    /// names (`docs/design/decisions.md`, "D101").
    ReadAsNull {
        table: String,
        column: String,
        declared_type: String,
        values: u64,
        semantics: ComparisonSemantics,
    },
}

/// One block a [`TableStream`](super::table::TableStream) replayed under an early stop — the filter, or
/// a [`DynamicFilter`](crate::stream::replay::dynamic::DynamicFilter)'s state, requiring a bound the block's stored row
/// order closes — and what the stop left unread ([`TableStream::early_stops`](crate::stream::table::TableStream::early_stops)),
/// reported after the fact, in bytes.
///
/// **Per block, not a count**, because a partitioned replay can hand one
/// block's pieces to several sub-streams and a piece past the stopping row
/// stops at its own first row: a caller summing over sub-streams counts a
/// block once by its `header_offset`, and adds its bytes. Rejected: counting
/// rows past the stop, never read and so only estimable; a zero for a stop
/// planned and never reached; exact bytes when split, read past each piece's
/// limit. Reopens: an account needing exact bytes, which each piece reporting
/// its first row's start would give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlyStop {
    /// The block, by its [`CopyBlock::header_offset`](pgdt_model::CopyBlock::header_offset).
    pub header_offset: u64,
    /// The bytes of rows the stop left unread in the pieces of the block this
    /// stream replayed, summed; `None` where it ended no piece before its last
    /// row — the bound never passed, or passed only there — which saved
    /// nothing.
    ///
    /// **Measured from the end of the stopping row to the piece's limit, that
    /// byte included**, or
    /// to the block's `\.` where the piece runs to it. A piece owns the row
    /// straddling its limit, whose rest past the limit is not counted, so this
    /// is short of the unread bytes by at most that one row's tail per piece
    /// and never counts a byte a skipped group's
    /// [`PlanNoteKind::StatisticsPruned`] `skipped_bytes` does: the two add.
    pub unread_bytes: Option<u64>,
}

impl PlanNote {
    pub(super) fn compressed_first_pass(codec: &'static str) -> Self {
        Self { kind: PlanNoteKind::CompressedFirstPass { codec }, levers: Vec::new() }
    }

    pub(super) fn compressed_block_path_declined(
        codec: &'static str,
        block_count: usize,
        max_block_uncompressed: u64,
        reader_bytes: u64,
        memory_bytes: u64,
        allowance_raises: bool,
    ) -> Self {
        Self {
            kind: PlanNoteKind::CompressedBlockPathDeclined {
                codec,
                block_count,
                max_block_uncompressed,
                reader_bytes,
                memory_bytes,
            },
            levers: levers(allowance_raises, false, false),
        }
    }

    pub(super) fn allocation_below_floor(
        unit_bytes: u64,
        memory_bytes: u64,
        allowance_raises: bool,
        chunk_sized: bool,
    ) -> Self {
        Self {
            kind: PlanNoteKind::AllocationBelowFloor { unit_bytes, memory_bytes },
            levers: levers(allowance_raises, chunk_sized && memory_bytes > 0, false),
        }
    }

    pub(super) fn batch_span_narrowed(
        stated_bytes: u64,
        planned_bytes: u64,
        workers: usize,
        memory_bytes: u64,
        allowance_raises: bool,
        chunk_sized: bool,
    ) -> Self {
        Self {
            kind: PlanNoteKind::BatchSpanNarrowed {
                stated_bytes,
                planned_bytes,
                workers,
                memory_bytes,
            },
            levers: levers(allowance_raises, chunk_sized, true),
        }
    }

    pub(super) fn parallelism_budget_limited(
        requested: usize,
        planned: usize,
        footprint: u64,
        max_source_span: Option<u64>,
        memory_bytes: u64,
        allowance_raises: bool,
        chunk_moves: bool,
    ) -> Self {
        Self {
            kind: PlanNoteKind::ParallelismBudgetLimited {
                requested,
                planned,
                footprint,
                max_source_span,
                memory_bytes,
            },
            levers: levers(allowance_raises, chunk_moves, false),
        }
    }

    /// The read-buffer budget this note's [`PlanNote::message`] quotes, where
    /// it quotes one — every kind but [`PlanNoteKind::StatisticsPruned`] and
    /// [`PlanNoteKind::ReadAsNull`], whose facts are about what the map holds,
    /// and [`PlanNoteKind::CompressedFirstPass`], about whether the source's
    /// index is complete, which name no budget at all.
    ///
    /// **It exists so a caller can say where that number came from.**
    /// Provenance is the caller's fact and never the library's
    /// (`docs/design/decisions.md`, "D64"), so the clause naming it is
    /// appended outside; which notes want one is a property of the note and
    /// not of the severity a caller prints it at — a narrowed span quotes a
    /// budget and is no fault ([`PlanNoteKind::BatchSpanNarrowed`];
    /// `docs/design/decisions.md`, "D84").
    pub fn budget_bytes(&self) -> Option<u64> {
        match &self.kind {
            PlanNoteKind::ParallelismBudgetLimited { memory_bytes, .. }
            | PlanNoteKind::CompressedBlockPathDeclined { memory_bytes, .. }
            | PlanNoteKind::AllocationBelowFloor { memory_bytes, .. }
            | PlanNoteKind::BatchSpanNarrowed { memory_bytes, .. } => Some(*memory_bytes),
            PlanNoteKind::StatisticsPruned { .. }
            | PlanNoteKind::ReadAsNull { .. }
            | PlanNoteKind::CompressedFirstPass { .. } => None,
        }
    }

    /// The clause offering a smaller read chunk, exactly where
    /// [`PlanNote::levers`] lists one, and empty elsewhere — one wording for
    /// every sentence that appends it.
    fn chunk_clause(&self) -> &'static str {
        if self.levers.contains(&PlanLever::SmallerReadChunk) {
            ", or a smaller read chunk, which this source sizes a reader from"
        } else {
            ""
        }
    }
}

impl Finding for PlanNote {
    /// `Info` for a fact that is no fault — a skip, a span the plan narrowed
    /// to seat the readers asked for (`docs/design/decisions.md`, "D84"),
    /// where the `Warning` beside it is what says a count still came up short,
    /// and a first pass indexing its source — and `Warning` for every note
    /// saying the budget in force declined something asked of it, and for
    /// values read as NULL. Derived from the kind, as
    /// a `ResolvedColumn`'s is from its resolution, so a caller printing it and one
    /// draining it into a sink cannot disagree.
    fn severity(&self) -> Severity {
        match self.kind {
            PlanNoteKind::StatisticsPruned { .. }
            | PlanNoteKind::BatchSpanNarrowed { .. }
            | PlanNoteKind::CompressedFirstPass { .. } => Severity::Info,
            PlanNoteKind::ParallelismBudgetLimited { .. }
            | PlanNoteKind::CompressedBlockPathDeclined { .. }
            | PlanNoteKind::AllocationBelowFloor { .. }
            | PlanNoteKind::ReadAsNull { .. } => Severity::Warning,
        }
    }

    /// One sentence saying what the plan did and why — for a decline, what to
    /// raise to close the gap — in the library's own vocabulary rather than
    /// any caller's flag names, as `ComparisonNote`'s is.
    fn message(&self) -> String {
        match &self.kind {
            PlanNoteKind::ParallelismBudgetLimited {
                requested,
                planned,
                footprint,
                max_source_span,
                memory_bytes,
            } => match max_source_span {
                Some(span) => {
                    let chunk = if self.levers.contains(&PlanLever::SmallerReadChunk) {
                        "a smaller read chunk, which sets one or both of those costs on this \
                         source, or "
                    } else {
                        ""
                    };
                    format!(
                        "asked for up to {requested} sub-stream(s), but a memory budget of \
                         {memory_bytes} byte(s) affords only {planned}: each costs {footprint} \
                         byte(s) to decode plus {span} byte(s) held by its own batch — the batch \
                         span is already as small as the plan will make it, so what seats more \
                         is {chunk}a larger memory budget, which is not the same as a larger \
                         allowance on a source that recommends no per-reader cost of its own"
                    )
                }
                None => format!(
                    "asked for up to {requested} sub-stream(s), but a memory budget of \
                     {memory_bytes} byte(s) affords only {planned} at {footprint} byte(s) to \
                     decode each — what seats more is a smaller read chunk, which a source \
                     cutting by one sizes that cost from, or a larger memory budget, which is \
                     not the same as a larger allowance on a source that recommends no \
                     per-reader cost of its own"
                ),
            },
            PlanNoteKind::CompressedBlockPathDeclined {
                codec,
                block_count,
                max_block_uncompressed,
                reader_bytes,
                memory_bytes,
            } => {
                let (source, unit) = container_words(codec);
                format!(
                    "this {source} has {block_count} {unit}(s) to seek by, but its largest is \
                     {max_block_uncompressed} byte(s) and a memory budget of {memory_bytes} \
                     byte(s) leaves no room for one reader of it — so it is read through the \
                     streaming decoder and every backward read decodes forward from its {unit}'s \
                     start; raise the memory budget to {reader_bytes} byte(s) or more to read it \
                     a {unit} at a time"
                )
            }
            PlanNoteKind::CompressedFirstPass { codec } => {
                let (source, _) = container_words(codec);
                format!(
                    "this {source} has no complete index yet, so it is read with one reader, \
                     through the first pass that places its checkpoints as it goes; once a read \
                     reaches its end, its reads are cut at those checkpoints, and so are a later \
                     run's through any cache this run saves"
                )
            }
            PlanNoteKind::AllocationBelowFloor { unit_bytes, memory_bytes } => {
                let chunk = self.chunk_clause();
                format!(
                    "a memory budget of {memory_bytes} byte(s) is less than the {unit_bytes} \
                     byte(s) one reader of this source holds, so this runs at its one-slot floor \
                     whatever concurrency is asked for — what lifts it off the floor is a larger \
                     memory budget, which is not the same as a larger allowance on a source that \
                     recommends no per-reader cost of its own{chunk}"
                )
            }
            PlanNoteKind::BatchSpanNarrowed {
                stated_bytes,
                planned_bytes,
                workers,
                memory_bytes,
            } => {
                let chunk = self.chunk_clause();
                format!(
                    "a memory budget of {memory_bytes} byte(s) cannot seat the sub-streams asked \
                     for beside batches spanning {stated_bytes} byte(s) of the source each, so \
                     each batch spans at most {planned_bytes} byte(s) instead and {workers} \
                     sub-stream(s) were planned — asking for fewer sub-streams leaves each a \
                     larger batch; so does a larger memory budget, which is not the same as a \
                     larger allowance on a source that recommends no per-reader cost of its \
                     own{chunk}"
                )
            }
            PlanNoteKind::StatisticsPruned { skipped_groups, groups, skipped_bytes, bytes } => {
                format!(
                    "row-group statistics rule out {skipped_groups} of {groups} group(s), so \
                     {skipped_bytes} of the {bytes} byte(s) of rows this table holds are not read"
                )
            }
            PlanNoteKind::ReadAsNull { table, column, declared_type, values, semantics } => {
                let term = match semantics {
                    ComparisonSemantics::Postgres => format!("`{column} IS UNREPRESENTABLE`"),
                    ComparisonSemantics::DataFusion => {
                        format!("`pgdump_unrepresentable({column})`")
                    }
                };
                format!(
                    "{table}.{column} holds {values} value(s) its type `{declared_type}` cannot \
                     hold, read as NULL — {term} tells them from the NULLs the dump holds"
                )
            }
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// How a sentence names a compressed source of `codec`
/// (`pgdt_io::CompressionIndex::codec`), and what it calls the unit a reader
/// decodes whole: an `.xz` file's block, the span between two of a gzip
/// file's checkpoints.
fn container_words(codec: &str) -> (&'static str, &'static str) {
    match codec {
        "gzip" => ("gzip source", "unit"),
        "xz" => (".xz source", "block"),
        _ => ("compressed source", "unit"),
    }
}

/// [`PlanNote::levers`] from which of them apply, in [`PlanLever`]'s order.
fn levers(allowance: bool, chunk: bool, fewer: bool) -> Vec<PlanLever> {
    [
        (allowance, PlanLever::LargerAllowance),
        (chunk, PlanLever::SmallerReadChunk),
        (fewer, PlanLever::FewerSubStreams),
    ]
    .into_iter()
    .filter_map(|(applies, lever)| applies.then_some(lever))
    .collect()
}
