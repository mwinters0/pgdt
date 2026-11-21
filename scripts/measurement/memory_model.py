"""The library's memory charge, modelled: what a budget affords and what each reader, pool and
block is charged, and the bands a reading is judged against when it refutes the model."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Sequence

from measurement import formatting


#: `pgdt_io::DEFAULT_MEMORY_BUDGET`, mirrored.
#:
#: Hardcoded rather than read out of the source, on `QUERY_SUBSTREAM_CAP`'s
#: argument: a second authority on the library's own constants goes stale
#: silently, and a literal checked by hand once is exactly as good until the
#: constant moves — at which point the figure is stale on paths its `depends`
#: already names. It is what a run that states no budget gets, which is the one
#: reading `reserve` takes off the budget axis.
LIBRARY_DEFAULT_BUDGET = 64 << 20

#: `pgdt_io::SCAN_CHUNK_DEFAULT_SIZE_BYTES`, mirrored — the read chunk a scan settles
#: at when no `--chunk-size` is stated, and one of the three terms in what a
#: block-decoding reader holds.
#:
#: Named apart from `CHUNK_DEFAULT`, which is the same number wearing the
#: chunk-size figure's hat — that one is *the row every other row is a ratio
#: against* and this one is *what a reader holds a buffer of*. A test holds the
#: two equal, which is the cheap reconciliation: the day they diverge, one of
#: the two meanings has moved and the figure that reads it is wrong.
LIBRARY_CHUNK_BYTES = 1 << 20

#: `xz_seek::Reader::decode_footprint()` on an 8 MiB-dictionary file: the
#: dictionary, the 1 MiB input chunk and `liblzma`'s own 34,592 B of state.
#: Every `.xz` input here is written at preset 6, so every one of them has that
#: dictionary (`INPUTS["control_xz"]`).
#:
#: **The chunk term is a constant across every registered input, not a
#: coincidence of one of them.** `SeekTable::input_chunk()` is the largest
#: block's *compressed* extent **capped at `xz_seek`'s 1 MiB `INPUT_CHUNK`**, and
#: both compressed inputs here compress a block to far more than that — 4.6 MB
#: at 24 MiB blocks, 24.5 MB at 128 MiB — so both saturate the cap. A file whose
#: every block compresses below 1 MiB would charge less and this constant would
#: over-state, which is the direction that costs nothing.
XZ_DECODE_FOOTPRINT = 9_471_776

#: The dictionary term inside `XZ_DECODE_FOOTPRINT`: 8 MiB for an
#: 8 MiB-dictionary file, allocated by `liblzma` through C `malloc` and
#: therefore **invisible to the counting allocator**, which sees only what
#: passes through Rust's `GlobalAlloc`.
#:
#: Named here because it is the one term an attribution must add back by hand:
#: the instrument's `live_*` family cannot see it and its `mallinfo_*`/`malloc_*`
#: family cannot separate it, so a decomposition that subtracted the two would
#: charge it to retention. Read off a stack rather than modelled —
#: `heaptrack_print` attributes exactly 8,388,608 bytes for one decoder through
#: `lzma_lz_decoder_init` ← `lzma_raw_decoder` ← `PayloadDecoder::new`
#: (`measurements.md`, "What an instrument can see").
XZ_DICT_BYTES = 8_388_608


def reader_bytes(unit: int) -> int:
    """What **one** concurrent block-decoding reader of a file with `unit`-sized
    blocks holds: one block slot — the block it is decoding, which is the block
    it then retains — the chunk buffer a straddling read is assembled into, and
    the decoder's own retention.

    `BlockCache::reader_bytes`, mirrored — the per-worker term of the charge
    `BlockCache::affordable` compares a budget against, and what
    `partition_advice` charges a sub-stream. It is not the whole
    charge: `charge_bytes` adds the retention list the pool shares, which is
    `POOL_DEPTH - 1` units at one reader and `jobs - 1` above the depth.

    **It is one number and not two, at the default chunk every leg here runs
    at.** `BlockCache::reader_bytes` takes its chunk term from
    `DecodeBudget::charged_chunk_bytes`, which answers `SCAN_CHUNK_DEFAULT_SIZE_BYTES` whenever
    no read loop has announced a length — so the charge
    `CompressedFileSource::default_worker_memory` recommends against *before* the file is
    open for reading and the charge `BlockCache::affordable` then compares a
    budget to are the same charge. *Rejected:* charging the unannounced pool's
    `POOL_MAX_BYTES` ceiling — it runs the recommendation 7 MiB a reader high
    and costs a reader at every allocation (`source/compressed/blocks.rs`, `charged_chunk_bytes`).
    A flagless run's resolved budget is **never** this number times its count:
    the shared retention list is billed at every count,
    so a budget read off a run's own report carries `pool_bytes` besides, which
    is what `charge_bytes` states. It splits again only for a caller that states
    a `--chunk-size` other than the default, which no leg of this figure does.

    **Hand-computed, on `QUERY_SUBSTREAM_CAP`'s argument.** A Python
    reimplementation of the library's arithmetic is a second authority that goes
    stale silently; a mirror checked by hand against the source once is exactly
    as good until one of its terms moves, at which point the figure is already
    stale on the paths its `depends` names. At 24 MiB blocks this is
    34.03 MiB. *Rejected:* two units a reader — the decode buffer and the
    retained block are one buffer, and the second unit is already the pool's
    own list (`pool_bytes`).
    """
    return unit + LIBRARY_CHUNK_BYTES + XZ_DECODE_FOOTPRINT

#: `pgdt_io`'s `source::pool::POOL_DEPTH`, mirrored: the floor `BufferPool::slots`
#: clamps a pool's slot count to, whatever worker count was announced to it.
#:
#: It lives with the charge rather than with the other library mirrors because
#: it is the second term of that charge: the block pool is sized
#: `POOL_DEPTH.max(jobs)` and `BlockCache::slot` drains to one below it before
#: obtaining the buffer `retain` then pushes back, so the pool holds
#: `(POOL_DEPTH.max(jobs) - 1) x unit` on top of the one unit each reader has in
#: flight. That is `pool_bytes`, which the library bills as what the pool
#: holds (`parallelism.rs`, `WorkerMemory`); naming it as a
#: column of its own is what keeps it out of the residual, where it would read
#: as a term nothing accounts for.
LIBRARY_POOL_DEPTH = 4

#: `pgdt_io::internal::MEMORY_RESERVE`, mirrored: what `Arrangement::resolve`
#: holds back from a discovered limit before it divides.
#:
#: It is the model's upper bound rather than a term in it. The rule's promise is
#: that everything a scan holds above what it billed fits inside this number, so
#: a leg whose unnamed remainder exceeds it is a leg the rule cannot keep inside
#: its allocation — which is the criterion `charge_model_problem` applies, and
#: the reason the bound is a library constant rather than a tolerance the
#: harness chose.
LIBRARY_MEMORY_RESERVE = 384 << 20


def stated_allowance(budget: int) -> int:
    """The `--memory` value that leaves `budget` bytes for the read buffers.

    **`--memory` states a *resident* allowance, not a buffer budget**
    (`docs/design/decisions.md`, "D83"): `Arrangement::resolve` takes
    `MEMORY_RESERVE` off the top before a reader is counted, so a figure
    registered against a buffer budget states that budget plus the reserve and
    goes on measuring what it was registered to measure.

    **It reproduces the old arrangement exactly only up to a 256 MiB budget.**
    The carve also holds the resolved *count* under `margin_allowance`, which a
    typed number never used to answer to, and that ceiling falls below the cap
    once the budget passes `4 x MEMORY_RESERVE - 5 x MEMORY_UNPOOLED_BOUND`.
    Above that a leg resolves fewer readers than it did — `reserve`'s stated
    axis is that arrangement, and `KD34` names what it reads; the number stated
    here is still the one the figure's text names.

    **Not the inverse of the carve**, which has none: nothing recovers the
    allowance a budget came from, because the budget is a `min` of two terms.
    This is the one direction that is a function.
    """
    return budget + LIBRARY_MEMORY_RESERVE

#: `pgdt_io::internal::MEMORY_UNPOOLED_BOUND`, mirrored: this crate's bound on
#: what a scan holds resident outside the pools its charge bills, and what
#: `margin_allowance` predicts a count's resident with.
#:
#: **The inner of the model's two fault lines**, where `LIBRARY_MEMORY_RESERVE`
#: is the outer one. A remainder above this is a finding about the *bound* — the
#: library predicts counts with a number the readings have overrun, so the
#: margin is not leaving what it claims — while a remainder above the reserve is
#: the *rule* failing, an arrangement the discovery cannot keep inside its
#: allocation. One threshold cannot tell those apart, which is why there are
#: two, and both are registered before the sitting.
#:
#: Read off the reserve constant's five-build grid under today's charge rather
#: than fitted: the worst surviving block-path remainder there is 238.6 MiB at
#: 24 MiB blocks and 142.0 MiB at 128, and this is the worst rounded up to a
#: 64 MiB step (`rederived_unpooled_bound`), about 17 MiB above it. The
#: published `reserve` sitting's worst remainder rounds by the same arithmetic
#: to 192 MiB (`docs/design/decisions.md`, "I/O, memory and parallelism").
LIBRARY_MEMORY_UNPOOLED_BOUND = 256 << 20

#: `pgdt_io::internal::MEMORY_MARGIN_PERCENT`, mirrored: the share of its
#: allocation the worst rep of a resolved arrangement must leave unused — the
#: criterion `MEMORY_RESERVE` was chosen against, which `reserve`'s gate
#: reports and does not fail on (`REPORTED_MARGIN_PERCENT`). Held to the
#: library's own by a test, like the two above.
LIBRARY_MEMORY_MARGIN_PERCENT = 20


def pool_bytes(unit: int, jobs: int) -> int:
    """What the block pool retains at `jobs` readers on top of the per-reader
    term: `(POOL_DEPTH.max(jobs) - 1) x unit`, and never zero.

    `WorkerMemory::pool_bytes`, mirrored — the second term of the library's
    charge, stated as what the pool holds rather than as a floor that decays.
    Kept as a column
    of its own rather than folded into `charge_bytes` because it is **unbounded
    in the block size** where every other term is not: at one reader 72 MiB at
    koji's 24 MiB blocks, 384 at 128, 1.5 GiB at 512, and growing by a unit a
    reader past `POOL_DEPTH`. A model that hid it inside a flat remainder would
    read as a term nothing accounts for.

    **Billed at every cell.** *Rejected:* `max(0, POOL_DEPTH - jobs) x unit`
    beside a per-reader term of two units, which clamps off at four readers and
    bills exactly one unit more than the pool holds at every count — a
    one-reader cell reads 130.0 MiB billed against 111.1 held at 24 MiB blocks.

    **It has two consumers and they read it for opposite purposes.**
    `charge_bytes` adds it, because the budget rule bills it; `_depooled`
    subtracts it, because a term known before the sitting has no business in a
    fitted intercept — and the shape that makes it worth billing apart is the
    same shape that makes a straight line across it wrong, being constant below
    `POOL_DEPTH` and per-reader above (`_depooled`). A change to this function moves
    both a charge and a published line.
    """
    return max(LIBRARY_POOL_DEPTH, jobs, 1) * unit - unit


def charge_bytes(unit: int, jobs: int) -> int:
    """What the budget rule charges `jobs` concurrent block-decoding readers of
    a file with `unit`-sized blocks, in bytes: the per-reader term times the
    count, plus the retention list those readers share.

    `WorkerMemory::at`, mirrored — what `Parallelism::fit` solves a cap against
    and what `pgdt_io`'s `worker_count` solves a budget against, so under discovery
    it is also the budget a run reports for itself.
    """
    return jobs * reader_bytes(unit) + pool_bytes(unit, jobs)


def discovered_budget(limit: int) -> int:
    """What a flagless run inside a container of `limit` bytes resolves as its
    budget: `limit − MEMORY_RESERVE`, floored at zero.

    `Parallelism::within_shared`, mirrored — `allowance.saturating_sub(
    MEMORY_RESERVE)` (`parallelism.rs`), which a stated `--memory` reaches by the same
    call (`stated_allowance`).

    **The margin is deliberately left out**, as the recommendation is. The
    resolved *count* also answers to `MEMORY_MARGIN_PERCENT` — its predicted
    resident, `charge + MEMORY_UNPOOLED_BOUND`, must leave a fifth of the
    limit — so what a run reports is at most this and
    often less. Nor does it, or `afforded_readers`, subtract `pgdt`'s
    `RETAINED_BOUND` (D165), which loosens both further. That makes this an upper bound on the
    budget rather than a prediction of it, which is what the use below wants:
    a count comparison that stays an upper bound.

    **Used to reason about the registered axis, never to report a reading.**
    Every number this figure publishes reads the budget back off the run's own
    `scan started` line, because a harness predicting it would be a second
    authority on the rule under test. What this answers instead is a question
    about `RESERVE_LIMITS` itself — whether the limits registered there can put
    three distinct reader counts on a line — which is settled before any run
    exists and cannot be read off one (`reserve_axis_problems`).
    """
    return max(0, limit - LIBRARY_MEMORY_RESERVE)


def block_path_afforded(unit: int, budget: int) -> bool:
    """Whether `budget` admits **one** block-decoding reader of a file with
    `unit`-sized blocks — so whether a leg ran the block path or the streaming
    fallback.

    `BlockCache::affordable`, mirrored: `worker_memory(..).at(1) <= budget`,
    which is `charge_bytes(unit, 1)` — the per-reader charge **plus the
    retention list that one reader leaves standing**, so the line is
    106.03 MiB at 24 MiB blocks and 522.03 at 128 — against 34.03 and 138.03 for
    the per-reader term alone. So `reader_bytes <= budget` is not this
    comparison (`source/compressed/blocks.rs`, `BlockCache::affordable`).

    **One function because the renderer asks the question at four places** — the
    flagless cell's own label, the fit's window, the model check's exclusion and
    the path step's two budgets — and four copies of it can disagree, which is
    a table whose cells silently change mechanism while reading as one series.

    **Asked of a reported budget, never of a container limit.** A flagless run
    resolves `limit − MEMORY_RESERVE` and says so on its own `scan started`
    line, so the budget is a reading and the path follows from it by the
    library's own arithmetic; deriving it from the `-m` token instead would
    re-implement the reserve rule under test.
    """
    return budget >= charge_bytes(unit, 1)


def afforded_readers(unit: int, budget: int) -> int:
    """How many concurrent block-decoding readers of a file with `unit`-sized
    blocks `budget` affords — the largest `k` with `charge_bytes(unit, k) <=
    budget`, and zero where not even one fits.

    `Parallelism::fit`, mirrored, with the recommendation **and the margin**
    left out: the count a run actually resolves is `min(recommendation,
    fit-under-margin)`, the recommendation is the machine's and the margin
    (`MEMORY_MARGIN_PERCENT`) lowers the fit further at every limit,
    so this is an upper bound on the resolved count rather than a prediction of
    it. Both omissions push the same way, which is what keeps the fit-ability
    check below a necessary condition.

    **Used to reason about the registered axis, never to report a reading**, on
    the same terms as `discovered_budget`. What it answers is whether
    `RESERVE_LIMITS` can put three distinct reader counts on a flagless
    family's line at all — which `reserve_axis_problems` asks —
    which is knowable before any run and, because the cap can only *collapse*
    two limits onto one count, is a necessary condition rather than a
    sufficient one. A sitting on a host with fewer cores than there are
    distinct fits publishes a secant, which is the per-sitting half the static
    check cannot reach.
    """
    if budget < charge_bytes(unit, 1):
        return 0
    k = 1
    while charge_bytes(unit, k + 1) <= budget:
        k += 1
    return k


def charge_model(unit: int, jobs: int, held: float) -> tuple[int, int, float]:
    """One leg's resident set, split into the two terms the model names and the
    one it does not: `(billed, floor, unnamed)`, all in bytes.

    - **billed** is what the budget rule charged — `charge_bytes(unit, jobs)`,
      which under discovery is also the budget the run reports for itself.
    - **pool** is `pool_bytes`, the part of that bill the block pool's
      retention list accounts for — inside `billed`, reported beside it because
      it is the one term unbounded in the block size.
    - **unnamed** is `held - billed`, which is glibc's arena retention as far
      as any reading here goes (`measurements.md`, "What a scan holds above the
      budget it was given").

    **This is an account and not a fit** — every term is arithmetic from the
    source, evaluated at the cell, where the alternative is doing it by hand
    over a sitting's readings after the runs have been spent searching for a
    constant (`.claude/skills/evidence/SKILL.md`, rule 1).
    """
    billed = charge_bytes(unit, jobs)
    pool = pool_bytes(unit, jobs)
    return billed, pool, held - billed

#: The three bands `charge_model_problem` reports. Every one of the module's
#: `BAND_*` names carries a stance in `BAND_STANCE` below, which is what
#: `charge_band_problems` holds it to.
BAND_RULE = "rule"

BAND_BOUND = "bound"

BAND_OVER_BILL = "over-bill"

#: The check's own rule, as the enumeration it is: which bands refute the model.
#: **`bound` alone is released**, and the rule is written as a list rather than
#: as a threshold because a threshold silently releases the band nobody has
#: argued about — "no evaluated cell above `MEMORY_RESERVE`" counts the check's
#: *upper* lines and lets the non-negativity side out with them.
#:
#: **Released means the allocation is intact *and* the sitting discharges the
#: finding**, which is the test any further band answers. A bound fault meets
#: both: the remainder is still under `MEMORY_RESERVE`, and the sitting
#: re-derives `MEMORY_UNPOOLED_BOUND` from its own cells
#: (`rederived_unpooled_bound`). An over-bill meets neither — it has to exceed
#: the whole of the rest of the process's footprint before the arithmetic can
#: report it at all, so it is never apparatus scatter the way a 238.6 MiB
#: remainder against a 256 MiB bound is, and it leaves nothing for the sitting
#: to repair.
#:
#: A band is released by mapping to `None`; every other band refutes the model,
#: and its value is *why* — the verdict prints that clause beside the cells, so
#: a band cannot be given a refuting stance without the reason being written
#: down in the same place. A band absent from the mapping refutes too, so a
#: fourth fault line added later reads as a refutation until somebody argues it
#: out rather than defaulting into the released half; `charge_band_problems` is
#: what makes that argument visible instead of silent.
#:
#: **A refutation does not bar publication** — see the verdict in
#: `run_reserve`: every cell is a real reading, and only a killed leg
#: (`KILL_TOLERANT`) keeps a figure out of the document.
BAND_STANCE: dict[str, str | None] = {
    BAND_OVER_BILL: (
        "an over-bill is bytes the rule charged that nothing holds, so it admitted fewer "
        "readers than the allocation afforded"
    ),
    BAND_BOUND: None,
    BAND_RULE: (
        "a remainder above `MEMORY_RESERVE` is an arrangement the discovery could not keep "
        "inside its allocation"
    ),
}

#: Why an unlisted band refutes. It is a reason and not an error because the
#: verdict must still render: `--check` is where an unargued band is reported
#: (`charge_band_problems`), and a sitting that runs before that is read should
#: say plainly why it refuted rather than raise.
UNARGUED_BAND_REFUTES = (
    "no stance is recorded for it, so it refutes until somebody argues it out"
)


def band_refutes(band: str) -> str | None:
    """Why this band refutes the model, or `None` where the rule releases it.

    The default is to refute: a fault line added later does not inherit the
    released half by being unmentioned.
    """
    return BAND_STANCE.get(band, UNARGUED_BAND_REFUTES)

#: The step `MEMORY_UNPOOLED_BOUND` is read off in: the reserve constant's
#: candidate builds were 64 MiB apart, so the bound is the worst observed
#: remainder rounded up to a step of it and `rederived_unpooled_bound` re-does
#: that arithmetic over a sitting's own cells.
UNPOOLED_BOUND_STEP = 64 << 20


@dataclass(frozen=True)
class ChargeFault:
    """One cell's fault: which of the model's three lines it crossed, and the
    sentence that says so.

    **The band is the half the verdict consumes.** A verdict that collapses
    every fault into one refutation calls a cell the inner line reads as *a
    finding about the bound, the allocation intact* a failure of the model
    exactly as a breach of the rule is. A fault carries its band so that the
    verdict can say which, and so that `BAND_STANCE` can be read by name.
    """

    band: str
    text: str

    @property
    def refutes(self) -> bool:
        """Whether this cell refutes the model.

        Read off `BAND_STANCE`, which is the rule written as an enumeration:
        `bound` is released, because the remainder is still inside
        `MEMORY_RESERVE` *and* the sitting re-derives the constant it overran.
        Every other band refutes, an unlisted one included — a band with no
        stance recorded is one nobody has argued out, not one the check lets
        through.
        """
        return band_refutes(self.band) is not None


def rederived_unpooled_bound(worst_unnamed: float) -> int:
    """What `MEMORY_UNPOOLED_BOUND` would be if it were read off these cells:
    the smallest 64 MiB step covering the worst unnamed remainder among them.

    This is the arithmetic the shipped constant was read off — 238.6 MiB worst
    over the reserve constant's five-build grid, the next step up being 256 —
    re-done over whatever sitting is in hand, which is why a cell above the
    bound is a finding rather than a refutation: the re-derivation is arithmetic
    over readings already taken, not a re-take (`docs/design/decisions.md`,
    "I/O, memory and parallelism").
    """
    steps = (max(0, int(worst_unnamed)) + UNPOOLED_BOUND_STEP - 1) // UNPOOLED_BOUND_STEP
    return max(1, steps) * UNPOOLED_BOUND_STEP


def charge_model_problem(unit: int, jobs: int, held: float) -> ChargeFault | None:
    """Why this leg refutes the charge model, or `None` where it does not.

    **Three fault lines, and none of them is a tolerance somebody picked.**

    - **Non-negative.** A negative remainder is an *over-bill*: the rule charged
      bytes nothing holds, so it admitted fewer readers than the allocation
      afforded. It is the failure a grid search over reserve constants cannot
      report at all, because a too-large charge shows up there as headroom.
    - **No larger than `MEMORY_UNPOOLED_BOUND`**, the inner line: that constant
      is what `margin_allowance` predicts a count's resident with, so a cell
      above it is a leg whose headroom is smaller than the library promised —
      a finding about the *bound*, which was read off the reserve constant's
      five-build grid.
    - **No larger than `MEMORY_RESERVE`**, the outer line: the reserve is by
      construction what covers everything the charge does not bill, so a
      remainder above it is a leg the *rule* cannot keep inside its allocation
      — the rule failing, stated per cell instead of per sitting.

    **Two lines rather than one, because they are different findings.** Between
    them the allocation still holds and the number the count is predicted
    against is wrong; above the outer one the allocation does not. Moving the
    single threshold inward would have made a slightly low bound read as a
    failed rule, and leaving it at the reserve would have made a bound the
    readings overran invisible until a kill.

    **The band travels with the sentence**, because the verdict that reads this
    names the band, and a distinction spent before it reaches its only consumer
    is not a distinction.

    Asked only of a leg that took the block path and survived: the streaming
    fallback holds none of these terms, and a censored leg's reading is a bound
    rather than a number.
    """
    billed, pool, _ = charge_model(unit, jobs, held)
    return charge_fault(billed, pool, jobs, held)


def charge_fault(billed: int, pool: int, jobs: int, held: float) -> ChargeFault | None:
    """`charge_model_problem`'s three lines, asked of any charge: `billed` what
    the rule charged `jobs` workers, `pool` the part of it a retention list
    holds (zero where the source pools nothing), `held` the process's peak.

    The lines are the library's, not a codec's — `MEMORY_RESERVE` and
    `MEMORY_UNPOOLED_BOUND` bound whatever any source's `WorkerMemory` leaves
    unbilled — so a figure whose charge is not `.xz`'s block path, as
    `gzip-resident`'s first pass is not, asks this with its own bill."""
    unnamed = held - billed
    if unnamed < 0:
        return ChargeFault(
            BAND_OVER_BILL,
            f"**over-billed by {formatting._fmt_budget_bytes(-unnamed)}** — {jobs} reader(s) were charged "
            f"{formatting._fmt_budget_bytes(billed)}"
            + (f", of which {formatting._fmt_budget_bytes(pool)} is the pool's retention list" if pool else "")
            + f", and the whole process held {formatting._fmt_budget_bytes(held)}. The rule admitted "
            "fewer readers than the allocation affords.",
        )
    if unnamed > LIBRARY_MEMORY_RESERVE:
        return ChargeFault(
            BAND_RULE,
            f"**{formatting._fmt_budget_bytes(unnamed)} unnamed**, above the "
            f"{formatting._fmt_budget_bytes(LIBRARY_MEMORY_RESERVE)} `MEMORY_RESERVE` that is meant to "
            f"cover it — {jobs} reader(s) billed {formatting._fmt_budget_bytes(billed)}"
            + (f", of which {formatting._fmt_budget_bytes(pool)} is the pool's list," if pool else "")
            + f" against {formatting._fmt_budget_bytes(held)} held. **The rule does not hold here**: the "
            "discovery cannot keep this arrangement inside its allocation.",
        )
    if unnamed > LIBRARY_MEMORY_UNPOOLED_BOUND:
        return ChargeFault(
            BAND_BOUND,
            f"**{formatting._fmt_budget_bytes(unnamed)} unnamed**, above the "
            f"{formatting._fmt_budget_bytes(LIBRARY_MEMORY_UNPOOLED_BOUND)} `MEMORY_UNPOOLED_BOUND` the "
            f"margin predicts with — {jobs} reader(s) billed {formatting._fmt_budget_bytes(billed)}"
            + (f", of which {formatting._fmt_budget_bytes(pool)} is the pool's list," if pool else "")
            + f" against {formatting._fmt_budget_bytes(held)} held. **A finding about the bound, not the "
            "rule**: it is still inside `MEMORY_RESERVE`, so the allocation holds and what is "
            "wrong is the number the count is predicted against.",
        )
    return None


def charge_verdict(
    model_faults: Sequence[tuple[ChargeFault, str]],
    model_unnamed: Sequence[float],
    evaluated: int,
    nowhere: str,
) -> str:
    """What a figure's cells say about the charge model, read off each faulting
    cell's band (`BAND_STANCE`): the refuting cells and why their bands refute,
    the cells inside the rule and the bound their remainders re-derive, or that
    every cell met it — and `nowhere`, the figure's own words for a sitting
    that evaluated no cell, which is not the same claim as one that met it.

    `model_faults` pairs each fault with its cell's line, `model_unnamed` is
    every evaluated cell's remainder, faulting or not, and `evaluated` is how
    many cells there were. One function for every figure that runs the check
    (`reserve`, `gzip-resident`), so a band's reading cannot differ by figure.
    """
    if model_faults:
        refuting = [line for fault, line in model_faults if fault.refutes]
        inside = [line for fault, line in model_faults if not fault.refutes]
        bands = []
        if refuting:
            # One clause per band actually hit, rather than one sentence over
            # the whole stanza: the bands refute for different reasons, and one
            # sentence asserts a single band's reason over every cell in it.
            hit = sorted({fault.band for fault, _ in model_faults if fault.refutes})
            bands.append(
                "**The model is refuted, and by these cells:**\n\n"
                + "\n".join(refuting)
                + f"\n\nThe `{BAND_BOUND}` band is the one a sitting discharges itself, and each "
                "cell above is in a band it is not: "
                + "; ".join(f"`{band}`, where {band_refutes(band)}" for band in hit)
                + ". The table publishes all the same: a refutation is a reading of the library, "
                "and only a killed leg keeps a figure out of the document."
            )
        if inside:
            # The re-derivation runs over every evaluated cell rather than over
            # the faulting ones — the bound is what covers the worst remainder
            # the sitting saw, and the cells under it are as much evidence of
            # that as the cells over it — but only where nothing bars.
            #
            # Withheld from the whole sitting wherever the model is refuted,
            # rather than from the refuting family alone: a refuted cell
            # describes an arrangement the rule did not keep inside its
            # allocation, or a charge nothing holds, and a bound is a term of
            # the model that sitting has just contradicted.
            tail = ""
            if any(fault.band == BAND_BOUND for fault, _ in model_faults):
                if refuting:
                    tail = (
                        "\n\nNo bound is re-derived from this sitting: the model is refuted "
                        "in it, so its worst remainder is not a reading a term of that model "
                        "may be sized to."
                    )
                else:
                    worst = max(model_unnamed)
                    tail = (
                        "\n\nThe worst remainder over the "
                        f"{evaluated} evaluated cell(s) is {formatting._fmt_budget_bytes(worst)}, so "
                        "these readings re-derive `MEMORY_UNPOOLED_BOUND` at "
                        f"**{formatting._fmt_budget_bytes(rederived_unpooled_bound(worst))}** — the "
                        f"smallest {formatting._fmt_budget_bytes(UNPOOLED_BOUND_STEP)} step that covers "
                        "it, which is the shipped bound's own arithmetic re-done over this "
                        "sitting and not a re-take."
                    )
            bands.append(
                "**Inside the rule, and so a finding rather than a refutation** — the "
                f"`{BAND_BOUND}` band is the one a sitting discharges itself, and every cell "
                "here is in it:\n\n"
                + "\n".join(inside)
                + tail
            )
        return "\n\n".join(bands)
    if evaluated:
        return (
            "**The model holds at every cell above**: nothing here crosses any of its three "
            "lines."
        )
    return nowhere


def charge_band_problems() -> list[str]:
    """Fault bands the code defines that `BAND_STANCE` says nothing about.

    **A rule written as a threshold releases a band nobody argued about.** "No
    evaluated cell above `MEMORY_RESERVE`" is a true sentence about two of
    `charge_model_problem`'s three lines and silently lets the third — the
    over-bill side — through. The rule is an enumeration (`BAND_STANCE`), and
    this is what holds the enumeration to the bands that exist: a fourth line
    added later refutes by default and is reported here until its stance is
    recorded, rather than inheriting whichever half its author had in mind.

    **Asked of the constants rather than of a sitting**, so it fails at
    `--check` time: two things that must agree, with nothing else reading
    both.

    The definition site it reads is the naming: every `BAND_*` string constant
    in this module is one of `charge_model_problem`'s bands, which is why the
    unlisted band's reason is spelled `UNARGUED_BAND_REFUTES` rather than with
    the prefix.
    """
    bad = []
    for name, value in sorted(globals().items()):
        if not name.startswith("BAND_") or not isinstance(value, str):
            continue
        if value not in BAND_STANCE:
            bad.append(
                f"{name} ({value!r}) has no stance in `BAND_STANCE`, so it refutes the model "
                "by default — record it as released or refuting"
            )
    return bad
