"""`reserve`: what a scan holds resident above the budget it was told it could have."""

from __future__ import annotations

from typing import Sequence
from typing import TYPE_CHECKING

from measurement import borrowing
from measurement import config
from measurement import formatting
from measurement import gating
from measurement import memory_model
from measurement import runspec
from measurement import shapes
from measurement.builds import leg_binaries
from measurement.figures import gate_table
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.timing import Session


#: The limit the mechanism leg is read at, and the input it is read over. The
#: capped instrument leg takes the same pair, so the black-box delta and the
#: introspective one describe the same arrangement.
#:
#: **One limit and one block size, deliberately.** This figure is gated
#: `warm-parallel` and crossing the mechanism leg with the limits and the
#: block sizes buys a second cross of the expensive axis for no question
#: anybody asked. The limit is the smallest on the axis. **At it the 24 MiB
#: leg resolves one reader on the block path, where the uncapped process
#: already runs two arenas**, so `MALLOC_ARENA_MAX=2` is inert at this cell —
#: the published leg reads it at −112 KiB — and says nothing about the arenas
#: more readers open. The path step beside it is the one pair that states a
#: budget, so that the block path and the streaming fallback are compared
#: inside this one allocation.
RESERVE_MECHANISM_LIMIT = "512m"

#: How far the instrument build's resident set may sit from the shipped
#: build's before the check calls the two different runs, as a percentage.
#:
#: **Stated rather than read off the shipped leg's spread.** This is a different
#: binary — its `.text` is its own and every allocation goes through a counter —
#: so three reps of the shipped build say nothing about how far it may
#: legitimately be. Ten percent is the same order `measurements.md` records for
#: two builds of one source differing by code layout alone; what the check is
#: for is catching an instrument leg that ran a *different arrangement*, which
#: the resolved-count half answers exactly, and this half is the coarse bound
#: beside it.
INSTRUMENT_TOLERANCE_PCT = 10

#: The limits the **instrument** legs are read at: the flagless arrangement,
#: run on the introspection build, so the process states its own terms instead
#: of being differenced.
#:
#: **The whole flagless axis at one block size**, so the instrument's
#: decomposition can be read against the black-box fit leg for leg rather than
#: at a single cell. One block size, for `RESERVE_MECHANISM_LIMIT`'s reason:
#: whether a term scales with the unit is the black-box axis's question, and it
#: already crosses both. The other block size's instrument legs are
#: `RESERVE_DIAGNOSTIC_LIMITS`, a family of their own that enters no fit.
RESERVE_INSTRUMENT_LIMITS: tuple[str, ...] = tuple(token for token, _ in shapes.RESERVE_LIMITS)

#: The **diagnostic** instrument legs: the flagless arrangement at the block
#: size where the shipped build holds more above its charge than the `system`
#: build does, read on the introspection build so the process says which heap
#: holds it. The limits are the two flagless cells at that block size whose
#: unnamed remainder left `MEMORY_UNPOOLED_BOUND` on the shipped build alone —
#: `1536m`, which also fails the gate's margin, and `2g` beside it — so the
#: legs are the failing arrangement's and nothing wider.
#:
#: **A second family, not more limits on the first.** The counter's own line
#: (`_fit_or_secant` over `RESERVE_INSTRUMENT_LIMITS`' legs) is a fit at
#: `RESERVE_MECHANISM_UNIT`, and its pool subtraction and its comparison with
#: `reader_bytes` are per unit; legs at another block size in that window would
#: put two units' readers on one line. These are rendered beside the charge
#: their own run was billed and checked against the shipped leg of the same
#: input and limit, and enter no fit.
#:
#: **What would falsify each candidate**, read off one row against `Billed`:
#: Rust's live high-water at or under the bill with mimalloc's committed
#: high-water well above it names mimalloc's retention of freed blocks; a live
#: high-water itself above the bill names the program, not the allocator; and a
#: glibc high-water carrying the excess names C. A row where none of the three
#: exceeds what the bill and the decoder dictionaries leave is no attribution,
#: and says so by its numbers.
#:
#: **The three are not exclusive**: a row can name two heaps, and a live
#: high-water is read against the bill's Rust terms, `Billed` carrying C's
#: decoder footprint besides. Where two hold excess, which one the gate's
#: failure is owed to is the `system` twin's to say, not this criterion's: a
#: term both builds hold predates the allocator and is filed, one the shipped
#: build holds alone is the allocator's (`docs/design/measurements.md`, "The
#: apparatus").
RESERVE_DIAGNOSTIC_INPUT = "control_xz128"

RESERVE_DIAGNOSTIC_LIMITS: tuple[str, ...] = ("1536m", "2g")


def reserve_axis_problems() -> list[str]:
    """What is wrong with the registered flagless axis: a block size at which
    the registered limits cannot reach `RESERVE_FIT_MIN_COUNTS` distinct reader
    counts.

    **It does not ask whether the pool term is billed anywhere**, because
    `(POOL_DEPTH.max(jobs) − 1) × unit` is billed at **every** cell and is the
    larger half of the charge above four readers: there is no window for a limit
    to be registered inside and no cell that evades it.

    **Nor does it ask that the axis straddle `POOL_DEPTH`.** Below-depth
    coverage is real and is why `544m` and `1088m` are registered
    (`RESERVE_LIMITS`), but it is a property of what the *fit* needs, and the
    fit is given the charge's own two-regime model to subtract rather than a
    straight line to find across the kink (`_depooled`). A check that the axis
    straddles the depth would be guarding a requirement nothing has; the term it
    would protect is already held to the library's own constants by
    `test_the_mirrored_pool_depth_and_constants_are_the_librarys_own`.

    **Asked of the registered limits rather than of a sitting**, which is what
    makes it a `--check` and not a verdict: an axis whose block-path limits
    afford fewer than `RESERVE_FIT_MIN_COUNTS` distinct reader counts can only
    ever publish a secant, and a later axis edit that quietly makes that true
    should fail before a sitting is spent rather than after. It is a
    **necessary** condition and not a sufficient one: the resolved count is
    `min(recommendation, fit)`, so a host with fewer cores than there are
    distinct fits collapses two of them onto one count. That residue is a
    per-sitting property and is what the secant covers; the static check
    is not where the guarantee comes from.
    """
    bad = []
    for _name, label, unit in shapes.RESERVE_FLAGLESS_INPUTS:
        counts = sorted(
            {memory_model.afforded_readers(unit, memory_model.discovered_budget(limit)) for _, limit in shapes.RESERVE_LIMITS}
            - {0}
        )
        if len(counts) < shapes.RESERVE_FIT_MIN_COUNTS:
            bad.append(
                f"{label}: the registered limits afford {len(counts)} distinct reader "
                f"count(s) — {counts or 'none'} — under the {shapes.RESERVE_FIT_MIN_COUNTS} a "
                "two-term fit needs, so this family publishes a secant however the sitting "
                "goes; register a limit affording a count none of the others does"
            )
    return bad


# -- what a scan holds above the budget it was given ------------------------


def _fmt_budget(n: int) -> str:
    """A stated budget, in MiB.

    Not `_fmt_bytes`, which labels a mebibyte-scale value `MB`: that reads
    correctly for a file whose size nobody chose and wrongly for a number
    someone typed on a command line as `--memory 67108864`. Every
    registered budget is a whole mebibyte, which a test holds."""
    if n % config.MIB:
        raise ValueError(f"budget {n} is not a whole number of MiB")
    return f"{n >> 20} MiB"


def _reserve_specs() -> list[runspec.RunSpec]:
    """One spec per source, arena setting and stated budget.

    A leg is identified by its command shape, which carries both the arena
    token and the budget, so two legs differing only in the words the table
    prints cannot share a reading -- the failure `_attribution_specs` names."""
    return [
        runspec.RunSpec(
            "pgdt",
            name,
            f"{shapes.RESERVE_FAMILY}{token}-{budget}",
            "warm-parallel",
            f"{source} {arena}, {_fmt_budget(budget)} stated",
        )
        for name, source in shapes.RESERVE_INPUTS
        for token, _, arena in shapes.RESERVE_ARENAS
        for budget in shapes.RESERVE_BUDGETS
    ]

#: The arena token that sets nothing. Every reading outside the arena legs
#: themselves is taken under it, because the shipped constant has to survive the
#: operator who capped nothing.
RESERVE_UNCAPPED = shapes.RESERVE_ARENAS[0][0]

#: The arena token that caps, read off the registry rather than written again.
RESERVE_CAPPED = next(token for token, value, _ in shapes.RESERVE_ARENAS if value)


def _flagless_shape(arena: str = "") -> str:
    """The flagless command shape, under one arena setting."""
    return f"{shapes.RESERVE_FLAGLESS}{arena or RESERVE_UNCAPPED}"


def _reserve_flagless_specs() -> list[runspec.RunSpec]:
    """One spec per compressed input and container limit, stating no flags.

    **The limit is on the spec, not in the shape**, because it is a `nerdctl
    run` argument: these legs share one command shape and are told apart by
    `RunSpec.memory`, which `RunSpec.key` carries for exactly this reason."""
    return [
        runspec.RunSpec(
            "pgdt",
            name,
            _flagless_shape(),
            "warm-parallel",
            f"{label}, flagless in {token}",
            memory=token,
        )
        for name, label, _ in shapes.RESERVE_FLAGLESS_INPUTS
        for token, _ in shapes.RESERVE_LIMITS
    ]


def _reserve_gate_specs() -> list[runspec.RunSpec]:
    """Every flagless leg's `gate_twin`: the gate's reading of the shipped
    default's own arrangement on the platform allocator, at every limit and
    both block sizes.

    **The whole flagless axis, and nothing else of this figure**: a kill is a
    reading on these legs alone (`KILL_TOLERANT`), and a kill on any other is
    already read on its twin when it raises (`Session.time_run`). The twins are
    the gate's and never this figure's fit, check or account, which are the
    shipped build's."""
    return [gating.gate_twin(spec) for spec in _reserve_flagless_specs()]


def _reserve_mechanism_specs() -> list[tuple[str, runspec.RunSpec]]:
    """The mechanism leg, and what the table calls it.

    **The reference is not here.** It is the flagless leg at the same input and
    the same limit, which the axis above already measures — so this is one
    reading rather than a pair, and the comparison is against a number no leg of
    this block had to re-take.

    **One leg, where three were registered.** The arena cap is the *same
    flagless arrangement* with one mechanism changed, and it is the only one of
    the three that bears on glibc's dynamic mmap threshold, which retains a
    block-sized buffer in the arena of every thread that ever decoded one:
    capping the arenas bounds how many can hold one. At `RESERVE_MECHANISM_LIMIT`
    the uncapped process already runs no more arenas than the cap allows, so the
    leg is inert there (`RESERVE_MECHANISM_LIMIT`). The two allocator legs are
    **dropped rather than re-aimed** — jemalloc and mimalloc do not have that
    threshold, so swapping them removes the mechanism instead of measuring it.
    On the default build, whose Rust heap is mimalloc's, the threshold reaches
    only what C allocates, and the rendered paragraph says so wherever the
    instrument reports two heaps.
    What replaced them is `_reserve_instrument_specs`, which asks the process
    rather than subtracting two of them.

    The path step is the other exception and states a budget, because the thing
    under test is a comparison the environment cannot express:
    `BlockCache::affordable` is read off the budget, so one byte either side of
    the one-reader charge it compares against (`charge_bytes(unit, 1)`, which is
    `reader_bytes` plus the pool's retention list) is the only way to change the
    path and nothing else.
    """
    arena = next(label for token, _, label in shapes.RESERVE_ARENAS if token == RESERVE_CAPPED)
    return [
        (
            arena,
            runspec.RunSpec(
                "pgdt",
                shapes.RESERVE_MECHANISM_INPUT,
                _flagless_shape(RESERVE_CAPPED),
                "warm-parallel",
                f"flagless in {RESERVE_MECHANISM_LIMIT}, {arena}",
                memory=RESERVE_MECHANISM_LIMIT,
            ),
        )
    ]


def _reserve_instrument_specs() -> list[runspec.RunSpec]:
    """The flagless arrangement on the introspection build, which is where the
    attribution comes from.

    **The same command shape as the black-box flagless legs**, so the two are
    the same arrangement measured two ways and the `getrusage` axis is the check
    on this one: a term the process names has to show up in the sum the wrapper
    measures. They differ by `RunSpec.binary` alone, which `key` carries, so
    neither can be read as a rep of the other.

    **The uncapped axis, plus the capped leg the mechanism row also takes.** The
    axis is what decomposes — every limit resolves its own reader count, so the
    program's own high-water can be read against that count — and the capped leg
    is what says *where* an arena cap's megabytes go, which the black-box delta
    beside it can only say *whether*; its `Arenas` column is also what says
    whether the cap had any arena to remove.

    These legs are readings of this figure, declared here; the build is never
    the timed binary: it takes atomics on every allocation and
    `binary_allocator` refuses it."""
    return [
        runspec.RunSpec(
            "introspect",
            shapes.RESERVE_MECHANISM_INPUT,
            _flagless_shape(),
            "warm-parallel",
            f"instrument, flagless in {token}",
            memory=token,
            instrument=True,
        )
        for token in RESERVE_INSTRUMENT_LIMITS
    ] + [
        runspec.RunSpec(
            "introspect",
            shapes.RESERVE_MECHANISM_INPUT,
            _flagless_shape(RESERVE_CAPPED),
            "warm-parallel",
            f"instrument, flagless in {RESERVE_MECHANISM_LIMIT}, "
            + next(label for token, _, label in shapes.RESERVE_ARENAS if token == RESERVE_CAPPED),
            memory=RESERVE_MECHANISM_LIMIT,
            instrument=True,
        )
    ]


def _reserve_diagnostic_specs() -> list[runspec.RunSpec]:
    """The flagless arrangement on the introspection build at
    `RESERVE_DIAGNOSTIC_INPUT`'s block size, one leg a limit of
    `RESERVE_DIAGNOSTIC_LIMITS`.

    The same command shape as the black-box flagless legs, differing by
    `RunSpec.binary` alone, as `_reserve_instrument_specs`' do; the label names
    the block size, which the 24 MiB family's leaves implicit."""
    label = next(
        label for name, label, _ in shapes.RESERVE_FLAGLESS_INPUTS if name == RESERVE_DIAGNOSTIC_INPUT
    )
    return [
        runspec.RunSpec(
            "introspect",
            RESERVE_DIAGNOSTIC_INPUT,
            _flagless_shape(),
            "warm-parallel",
            f"instrument, {label}, flagless in {token}",
            memory=token,
            instrument=True,
        )
        for token in RESERVE_DIAGNOSTIC_LIMITS
    ]


def _reserve_step_specs() -> list[runspec.RunSpec]:
    """The path step's pair, in table order: the budget that affords one
    block-decoding reader, then the one a byte below it."""
    return [
        runspec.RunSpec(
            "pgdt",
            shapes.RESERVE_MECHANISM_INPUT,
            f"{shapes.RESERVE_STEP_FAMILY}{budget}",
            "warm-parallel",
            f"`--memory {memory_model.stated_allowance(budget)}`, block decode "
            + (
                "afforded"
                if memory_model.block_path_afforded(shapes.RESERVE_MECHANISM_UNIT, budget)
                else "declined"
            ),
            memory=RESERVE_MECHANISM_LIMIT,
        )
        for budget in shapes.RESERVE_STEP_BUDGETS
    ]


def _least_squares(points: Sequence[tuple[float, float]]) -> tuple[float, float]:
    """The intercept and slope of the line best fitting `points`.

    `resident = fixed + readers × per_reader`, which is the pair this figure
    publishes. Written out rather than taken from a library because the harness
    depends on nothing but the standard library, and because three lines of
    arithmetic are easier to check than an import is to justify.

    **It is never given a resident set.** The mechanism is
    piecewise linear where this model is straight: the pool holds `workers`
    blocks in flight plus `max(POOL_DEPTH, workers) − 1` retained
    (`WorkerMemory::pool_bytes`), so held memory rises by one unit a reader
    below `POOL_DEPTH` and by two above it, with a real kink at four readers
    that is in the measured resident and not only in the charge. Every caller
    therefore hands this function a **remainder** — `_depooled` has already
    taken that term off each ordinate — and what comes back are the two terms
    the sitting does not know in advance.

    Raises on fewer than two distinct abscissae: a "fit" through one point is an
    intercept asserted as a measurement, which is the extrapolation mistake
    `.claude/skills/evidence/SKILL.md`'s second rule names.
    """
    xs = [x for x, _ in points]
    if len(set(xs)) < 2:
        raise ValueError(f"a fit needs two distinct reader counts, got {sorted(set(xs))}")
    n = len(points)
    mean_x, mean_y = sum(xs) / n, sum(y for _, y in points) / n
    cov = sum((x - mean_x) * (y - mean_y) for x, y in points)
    var = sum((x - mean_x) ** 2 for x in xs)
    slope = cov / var
    return mean_y - slope * mean_x, slope


def _depooled(
    points: Sequence[tuple[float, float]], unit: int
) -> list[tuple[float, float]]:
    """`points` — `(readers, MiB)` — with the block pool's retention list taken
    off each ordinate, leaving the remainder a line may honestly be fitted to.

    **The charge is piecewise linear and a straight line across its kink is a
    biased line**. `pool_bytes` is `(POOL_DEPTH.max(workers) − 1) ×
    unit`: constant below `POOL_DEPTH` and growing by a unit a reader above it,
    so what a leg holds rises by one unit a reader at the bottom of the axis and
    by two at the top, and both registered families straddle the bend
    (`RESERVE_LIMITS`). Fitting one line across it puts the bend into the
    intercept — over the charge's own held-unit values ≈49 MiB of intercept bias
    at 24 MiB blocks and ≈421 MiB at 128, where the slope reads 1.37 units
    against a true 2.0.

    **Subtracting rather than fitting a second regime is `charge_model`'s
    principle one table over**: every quantity in the term is known before the sitting and
    already mirror-checked against the library's own constants, so taking it off
    is arithmetic and not a degree of freedom. What is left — a fixed cost and a
    per-reader cost outside the pools the charge bills — is regime-free and is
    the part the sitting genuinely measures, which is why
    `RESERVE_FIT_MIN_COUNTS` is unchanged: the remainder still has two terms.

    **Only a block-path leg may be handed here.** A leg on the streaming
    fallback holds no block pool at all, so subtracting a retention list from it
    would invent a negative term; callers drop those legs from the line by name
    (`block_path_afforded`), the way the flagless family always has.
    """
    return [(x, y - memory_model.pool_bytes(unit, int(x)) / config.MIB) for x, y in points]


def _fit_or_secant(
    points: Sequence[tuple[float, float]], unit: int
) -> tuple[float | None, float]:
    """`(fixed, per_reader)` where the points cover enough of the axis to
    publish an intercept, and `(None, per_reader)` where they do not.

    **The publication guard, at the boundary every fitted line crosses.**
    `RESERVE_FIT_MIN_COUNTS` says why three; what this function adds is that the
    rule is enforced once, in front of `_least_squares`, rather than in whichever
    renderer remembered it — the harness has three call sites across two
    families, and a guard carried by one of them lets the instrument account
    publish an intercept in the same sitting the flagless axis refuses one.

    **The slope is the same number either way, which is why it survives.** With
    exactly two distinct abscissae the least-squares slope *is* the secant
    between the two groups' means — `(ȳ₂ − ȳ₁) / (x₂ − x₁)`, whatever the group
    sizes — so the caller printing a secant is printing a reading of the axis,
    not a second arithmetic path that has to be kept in step with the first.
    What is withheld is the intercept and, with it, the residual, both of which
    are the two-term model's claims rather than the data's.

    **It is also where the pool term comes off**, for the same reason: the
    de-pooling is a property of the mechanism rather than of one table, and a
    renderer that forgot it would publish an intercept carrying the charge's own
    bend (`_depooled`). So both terms this returns are the remainder's —
    the cost *outside* the block pool's retention list — and a caller computing
    residuals against them measures its ordinates through `_depooled` too.

    Raises below two distinct abscissae, where `_least_squares` does: a secant
    needs two points as much as a fit does.
    """
    remainder = _depooled(points, unit)
    fixed, slope = _least_squares(remainder)
    if len({x for x, _ in remainder}) < shapes.RESERVE_FIT_MIN_COUNTS:
        return None, slope
    return fixed, slope


def _censored_constraint(
    session: Session, figure: str, spec: runspec.RunSpec, label: str, token: str, limit: int
) -> str:
    """One OOM-killed leg, written as the constraint it still proves.

    **It is stated as "did not fit", not as `peak > limit`.** What the kill
    reads off the apparatus is that the kernel reaped a process in that cgroup
    ([`../../../docs/design/runtime-invariants.md`](../../../docs/design/runtime-invariants.md),
    `RT9`), which says the arrangement did not run inside the allocation — a
    fact about the *arrangement*, needing no argument about what the cgroup
    charged to whom. The register's fourth scope limit does license the step to
    "something reclaim could not free hit the ceiling", clean page cache being
    reclaimed rather than killed for; what it does not license is the last step
    to `peak > limit`, because the charge is the **cgroup's** and the fit is
    over one process's `ru_maxrss`. "The rule's own arrangement did not fit its
    allocation" is already the thing a reserve constant is chosen against, so
    the stronger sentence would buy precision in a quantity the line is not
    about.

    **It bounds the arrangement, not the fitted terms.** It prints under the
    fits because that is where a reader looks for what happened at the limits,
    and the window sentence above — each fit naming the legs it covers — is
    only legible with the legs it does *not* cover adjacent to it. That
    placement is not a claim that this is a point the fit should pass near:
    the reserve constant is chosen from a headroom criterion over surviving
    reps, and a killed leg has no headroom to report.

    **The number beside it is a floor, and says so.** `maxrss_bound_kib` is
    where the wrapper's reading had got to when the process was reaped, so it
    is a lower bound on a peak that was never reached; `seconds_to_kill` is how
    far in, which separates a leg that died opening its first block from one
    that ran most of a scan. The worst rep's pair is printed, since a censored
    leg's reps are not samples of one distribution and their median means
    nothing.

    **Not fitted, deliberately.** Interval censoring is the statistically right
    treatment of a bound like this and the wrong size for a family of
    `RESERVE_LIMITS`' size — see `run_reserve`, which is where that refusal is
    argued. A constraint line is what carries the reading without a second
    fitting technique in the harness.
    """
    killed = session.kills(figure, spec)
    bounds = session.censored_bounds(figure, spec)
    report = session.reported.get(spec.key(figure), {})
    where = (
        f"{report['resolved_jobs']} reader(s) inside "
        f"{formatting._fmt_budget_bytes(int(report['resolved_budget']))}"
        if "resolved_jobs" in report and "resolved_budget" in report
        else "an arrangement it never lived to report"
    )
    # The *worst* rep, as one rep: a peak from one and a duration from another
    # would describe a run that did not happen.
    worst = max((b for b in bounds if b[0] is not None), key=lambda b: b[0], default=None)
    floor = (
        f" The wrapper had reached **{formatting.fmt_mib(worst[0])}** when it was reaped"
        + (f", {worst[1]:.1f} s in" if worst[1] is not None else "")
        + " — a floor on the peak, not the peak."
        if worst
        else " The run reported no bound before it died, so the kill is all there is."
    )
    return (
        f"- **{label}** at `-m {token}`: **did not fit {limit / config.MIB:,.0f} MiB** with {where} "
        f"({killed} rep(s) killed)." + floor
    )


def run_reserve(session: Session) -> str:
    """What a scan holds resident **above** the budget it was told it could have.

    The budget rule is `limit - reserve`, and this is the one number in it.
    Neither other resident figure answers it: `peak-rss` measures the whole
    against nothing, and `rss-attribution` decomposes growth per `COPY` block.

    **It checks a model rather than searching for a constant.**
    `MEMORY_RESERVE` was chosen off five builds' flagless legs against a
    headroom criterion and `MEMORY_UNPOOLED_BOUND` by arithmetic over the same
    readings, so what a sitting can do is fault (`charge_model_problem`). The
    fitted **fixed and per-reader terms** are published beside that check, each
    with its spread, because a table reporting only resident leaves the
    decomposition unstated; neither constant is read off them. This is the
    opposite end of `rss-attribution`, which holds block count as its axis and
    publishes a *slope* against an intercept that is the allocator's baseline.

    **The attribution is introspective, and the black-box legs are its check.**
    A subtraction between whole runs cannot name a term that no leg removes. So
    the account comes from the process reporting its own live bytes and its
    allocator's retention (`_reserve_instrument_specs`), and the `getrusage` axis
    stays as the independent reading it has to agree with — two instruments
    sharing no mechanism, which is the independence
    `.claude/skills/evidence/SKILL.md`'s third rule asks for. The instrument
    build is never the timed binary: it takes atomics on every allocation, and
    `binary_allocator` refuses it (`docs/design/roadmap.md`, "Attribution is
    introspective; only the gate is blind").

    **Two families, because one arrangement cannot answer both questions.** The
    flagless legs run what a person who states nothing gets, which is the
    arrangement the constant is for and the only one that can say what the
    shipped default holds; the stated legs pin the count and vary the budget,
    which is the only arrangement that can tell a reader's cost from a budget
    byte's -- under discovery `budget = jobs x per_worker` exactly, so there the
    two axes are one axis. Dropping either leaves a figure that cannot answer
    one of the two questions asked of it.

    **The uncapped legs are the ones the shipped default answers to**, because
    the default has to survive the operator who did not set `MALLOC_ARENA_MAX` --
    that being the case that kills the process.

    **A leg the kernel killed is a third cell state, not a missing number**
    (`KILL_TOLERANT`). Its reading is a bound on a peak the process never
    reached, so the cell says so and carries no headroom; and the leg leaves the
    fit **whether or not a rep survived**. `emit` is what then bars the figure
    from publication.

    **What that exclusion is for is enforcement, not the survivorship it looks
    like.** Survivorship is real — the reps that survived are the ones that
    stayed under the ceiling, so a line through them reads low — but it is not
    what earns the rule its place, because two other rules already close the
    path it protects: the constant is not read off a censored table at all, and
    it is chosen from a measured headroom rather than off this fit. Both of
    those are prose. This exclusion is the only *mechanical* thing between a
    censored sitting and a fitted number, which is why it stays. Stating it the
    other way round is how the rule gets reversed by a later session that
    correctly observes the bias argument is not load-bearing.

    *Rejected:* fitting the censored point as an interval-censored observation.
    It is the statistically right answer and the wrong size for a family of
    `RESERVE_LIMITS`' size — it buys precision this table cannot support and puts
    a second fitting technique in the harness.

    **Each fit states the window it covers, in the legs that are in it.** Every
    other clause names what *left* — declined, censored — and a reader holding
    only those has to subtract them from a tuple the table never prints. It
    matters here more than it would elsewhere, because a leg is censored exactly
    when its resident ran closest to its ceiling: dropping every such leg leaves
    a line through the legs that had room, which is
    [`.claude/skills/evidence/SKILL.md`](../.claude/skills/evidence/SKILL.md)
    rule 2's window trap with the sign flipped. Both branches say it — the
    no-fit one too, since "no fit" is a claim about a window as much as a fit
    is.

    **A killed leg is then printed as a constraint, under the fits**
    (`_censored_constraint`). It is the one reading in the table that says
    resident is *above* a number rather than at one, which is worth more to a
    reserve than another interior point; leaving it in `raw.json` unread was
    `KILL_TOLERANT` recording a reading and the renderer discarding it, two
    policies for one number.
    """
    figure = "reserve"
    stated = _reserve_specs()
    flagless = _reserve_flagless_specs()
    gate = _reserve_gate_specs()
    mechanism = _reserve_mechanism_specs()
    instrument = _reserve_instrument_specs()
    diagnostic = _reserve_diagnostic_specs()
    steps = _reserve_step_specs()
    # Before the first reading, as `rss-attribution` and the `allocator` figure
    # build theirs: a leg discovered missing at rep two has already spent the
    # session's first rep under a different machine state. The instrument build
    # is the one whose absence is silent — a default binary writes no report and
    # the leg fails at the rep, not at the build — so it is made and
    # interrogated here.
    if instrument or diagnostic:
        leg_binaries.ensure_instrument_binary(session.cfg, session.log)
    # The serial baseline, from `peak-rss` where this sitting took it, and
    # dropped from the interleave below rather than left in it — a spec the
    # sitting already holds would spend a reading to overwrite one.
    note = borrowing.share_readings(session, figure)
    specs = [
        # Each flagless leg beside its gate twin, so the pair is read under
        # one machine state.
        *(leg for pair in zip(flagless, gate) for leg in pair),
        *instrument,
        *diagnostic,
        *(spec for _, spec in mechanism),
        *steps,
        *stated,
        register._RESERVE_BASELINE,
    ]
    to_sweep = [s for s in specs if not session.has(figure, s)]
    session.build_before_sweep(to_sweep)
    session.sweep(figure, to_sweep, session.cfg.reps(3))
    # A sweep takes every leg, so this keeps them all; a re-render of a
    # sitting taken before the diagnostic legs existed renders without their
    # table rather than raising on readings it never took.
    diagnostic = [s for s in diagnostic if session.has(figure, s)]

    per_rep: list[str] = []

    def rss(spec: runspec.RunSpec) -> list[float]:
        readings = session.get_rss(figure, spec)
        killed = session.kills(figure, spec)
        per_rep.append(
            f"- {spec.label}: "
            + (", ".join(formatting.fmt_mib(v) for v in readings) or "no surviving rep")
            + (f" · **{killed} rep(s) OOM-killed**" if killed else "")
        )
        return readings

    def arrangement(spec: runspec.RunSpec) -> tuple[int, int] | None:
        """The worker count and budget this leg's own run reported, or `None`
        where no rep of it got as far as reporting one."""
        report = session.reported.get(spec.key(figure), {})
        if "resolved_jobs" not in report or "resolved_budget" not in report:
            return None
        return int(report["resolved_jobs"]), int(report["resolved_budget"])

    def resolved(spec: runspec.RunSpec) -> tuple[int, int]:
        """The worker count and budget this leg's own run reported.

        Read back rather than computed: the count a flagless run resolves is
        `Discovered::resolve`'s answer to the allocation, and a harness that
        predicted it would be a second authority on the rule under test.

        A leg with a surviving rep that reported nothing is an error; a leg with
        none at all is a censored cell and its caller asks `arrangement`
        instead, because the mode report is printed before the scan opens and a
        run killed early enough may never have reached it."""
        got = arrangement(spec)
        if got is None:
            raise RuntimeError(
                f"{spec.label}: the run reported no resolved arrangement, so the fit has no "
                "reader count — `scan started` is where it comes from (`parse_resolution`)"
            )
        return got

    # -- the flagless axis: what the shipped default resolves and holds -----
    limits = dict(shapes.RESERVE_LIMITS)
    by_flagless = {(spec.input, spec.memory): spec for spec in flagless}
    flagless_rows, fits = [], []
    for token, limit in shapes.RESERVE_LIMITS:
        cells = [f"`-m {token}`"]
        for name, _, unit in shapes.RESERVE_FLAGLESS_INPUTS:
            spec = by_flagless[(name, token)]
            readings = rss(spec)
            killed = session.kills(figure, spec)
            if not readings:
                # The third cell state: every rep censored, so there is no
                # number to print. What the leg still says is the arrangement
                # it resolved before it died, which is the half of the reading
                # the kill did not destroy.
                got = arrangement(spec)
                cells.append(
                    f"**OOM-killed**, {killed} rep(s) · "
                    + (
                        f"{got[0]}r, {formatting._fmt_budget_bytes(got[1])}"
                        if got
                        else "arrangement never reported"
                    )
                    + " · head —"
                )
                continue
            jobs, budget = resolved(spec)
            worst = max(readings) * 1024
            head = (limit - worst) / limit * 100
            # Whether this leg took the block path at all is the budget
            # against what one reader of *this file* costs, which is
            # `block_path_afforded` — `BlockCache::affordable` exactly. A
            # declined leg is not on the line the fit is over, and the fit
            # leaves it out by name.
            #
            # **Every cell names its path, not only the declined ones.** The two
            # arrangements hold different things — the streaming fallback keeps
            # no block slots at all — so a column mixing them is two series
            # printed as one, and an unmarked cell cannot be told from a cell
            # nobody checked.
            block_path = memory_model.block_path_afforded(unit, budget)
            cells.append(
                f"{formatting.fmt_mib_median_spread(readings)} · {jobs}r, {formatting._fmt_budget_bytes(budget)} · "
                f"head {head:.1f}%"
                + (" · *block path*" if block_path else " · *streaming*")
                # A partly-censored leg's surviving reps are the ones that did
                # not reach the ceiling, so its median understates and its worst
                # is not the worst. Said in the cell, and kept out of the fit.
                + (f" · **{killed} rep(s) OOM-killed**" if killed else "")
            )
        flagless_rows.append(cells)
    flagless_table = formatting.md_table(
        ["Allocation", *(label for _, label, _ in shapes.RESERVE_FLAGLESS_INPUTS)], flagless_rows
    )
    gate_legs = [
        (f"{label}, `-m {token}`", by_flagless[(name, token)], limit)
        for name, label, _ in shapes.RESERVE_FLAGLESS_INPUTS
        for token, limit in shapes.RESERVE_LIMITS
    ]
    gate_text = gate_table.gate_section(session, figure, gate_legs)
    for twin in gate:
        if session.has(figure, twin):
            rss(twin)

    constraints: list[str] = []
    for name, label, unit in shapes.RESERVE_FLAGLESS_INPUTS:
        points, declined, censored_legs = [], [], []
        for token, limit in shapes.RESERVE_LIMITS:
            spec = by_flagless[(name, token)]
            readings = session.get_rss(figure, spec)
            # A censored leg is out of the fit whether or not a rep survived:
            # that exclusion is the one mechanical thing between a censored
            # sitting and a fitted number (`run_reserve`). It is not out of the
            # *figure* — `_censored_constraint` is what it still says, printed
            # under the fits.
            if session.kills(figure, spec):
                censored_legs.append(token)
                constraints.append(
                    _censored_constraint(session, figure, spec, label, token, limit)
                )
                continue
            jobs, budget = resolved(spec)
            if not memory_model.block_path_afforded(unit, budget):
                declined.append(token)
                continue
            points.append((token, jobs, readings))
        censored_tail = (
            f". `{'`, `'.join(censored_legs)}` was OOM-killed and is out of the fit — its "
            "reading is a bound on a peak the process never reached, not a point on the line; "
            "what it still proves is below"
            if censored_legs
            else ""
        )
        # The window, said as the legs that are *in*. Every other clause here
        # names what left — declined, censored — and a reader who has only
        # those has to subtract them from a tuple they cannot see.
        window = ", ".join(f"`{t}` at {j}r" for t, j, _ in points) or "no leg at all"
        declined_tail = (
            f". `{'`, `'.join(declined)}` declined the block path and is not in the line"
            if declined
            else ""
        )
        counts = {jobs for _, jobs, _ in points}
        if len(counts) < 2:
            # Not even a secant: one abscissa is a point, and a line through a
            # point is an intercept asserted as a measurement.
            fits.append(
                f"- **{label}**: no line — "
                + (
                    f"the block path is declined at {', '.join(f'`{t}`' for t in declined)} and "
                    if declined
                    else ""
                )
                + f"what is left is {window}, which is {len(counts)} distinct reader count(s), "
                "under the 2 any line through them needs"
                + censored_tail
                + "."
            )
            continue
        # Every line here is over the **remainder**: `_fit_or_secant` takes the
        # block pool's retention list off each ordinate first, that term being
        # known before the sitting and the reason a single straight line across
        # the axis reads its intercept ≈49 MiB high at 24 MiB blocks and
        # ≈421 MiB high at 128 (`_depooled`). Only block-path legs reach
        # here, which is what makes the subtraction well defined.
        fixed, per_reader = _fit_or_secant(
            [(j, formatting.median(r) / 1024) for _, j, r in points], unit
        )
        # The band is the same line taken over the per-rep extremes rather than
        # the medians: a term's spread is what the reps permit it to be, and a
        # single residual says nothing about which of the two terms moved.
        band = [
            _fit_or_secant([(j, pick(r) / 1024) for _, j, r in points], unit)
            for pick in (min, max)
        ]
        if fixed is None:
            # The secant. `RESERVE_FIT_MIN_COUNTS` is what withholds the
            # intercept, and the residual goes with it: a two-term model has
            # none at two points, so printing one would be printing zero as
            # though it were a reading.
            ends = " → ".join(
                ", ".join(f"`{t}`" for t, j, _ in points if j == count) + f" at {count}r"
                for count in (min(counts), max(counts))
            )
            fits.append(
                f"- **{label}**: no intercept — a **secant**, not a fit: a reader "
                f"**{per_reader:,.1f} MiB** outside the pool "
                f"({band[0][1]:,.1f}–{band[1][1]:,.1f}) over "
                f"{ends}. The {len(points)} leg(s) left cover {len(counts)} distinct reader "
                f"count(s), under the {shapes.RESERVE_FIT_MIN_COUNTS} a two-term model needs before "
                "its intercept is a reading, so none is published and no residual with it"
                + declined_tail
                + censored_tail
            )
            continue
        # Measured through `_depooled` as well, so the residual is a residual of
        # the line that was actually fitted rather than of a line over a
        # quantity nobody fitted.
        residual = max(
            abs(y - (fixed + per_reader * j))
            for (j, y) in _depooled([(j, formatting.median(r) / 1024) for _, j, r in points], unit)
        )
        fits.append(
            f"- **{label}**: outside the pool — fixed **{fixed:,.0f} MiB** "
            f"({band[0][0]:,.0f}–{band[1][0]:,.0f}), a reader **{per_reader:,.1f} MiB** "
            f"({band[0][1]:,.1f}–{band[1][1]:,.1f}), over "
            f"the {len(points)} leg(s) it covers — {window}; residuals "
            f"reach ±{residual:,.0f} MiB"
            + declined_tail
            + censored_tail
        )

    # -- the charge against what was held -----------------------------------
    #
    # The model check, per cell. Every other reading in this figure is a number
    # somebody then has to reason about; this is the arithmetic done in the
    # renderer, at the cell, against a criterion registered before the sitting
    # (`charge_model_problem`). Done by hand over a sitting's readings, it dies
    # with the session that does it, which is why it is here.
    #
    # **It reports; it neither raises nor bars publication — two levers, not
    # one.** `KILL_TOLERANT` pulls both: the sitting survives a kill *and*
    # `emit` refuses to publish the figure that lost the leg. So the precedent
    # settles the first half here and says nothing about the second, which
    # turns on what a cell is. A censored cell is a bound on a peak the process
    # never reached, so the table above it describes an arrangement nobody
    # measured; every cell here is a real reading of a real run, and what a
    # refutation falsifies is the library's claim about those readings rather
    # than the readings. Barring publication on it would leave
    # `measurements.md` able to carry only tables that agree with the library,
    # which is the opposite of what it is for. So the verdict is written in
    # bands (`BAND_STANCE`): an over-bill or a cell above the reserve refutes
    # the model and says why, and a cell between the bound and the reserve is a
    # finding that re-derives the bound from the sitting's own remainders.
    model_rows, model_faults, model_unnamed = [], [], []
    for name, label, unit in shapes.RESERVE_FLAGLESS_INPUTS:
        for token, _limit in shapes.RESERVE_LIMITS:
            spec = by_flagless[(name, token)]
            readings = session.get_rss(figure, spec)
            # A censored leg's reading is a bound on a peak never reached, so it
            # is not a `held` this model may be evaluated at — the same
            # exclusion the fit above makes, for the same reason.
            if session.kills(figure, spec) or not readings:
                continue
            jobs, budget = resolved(spec)
            # A declined leg ran the streaming fallback, which holds none of
            # these terms. Excluded by what the leg *did*, read off its own
            # reported budget, rather than by which token it carries.
            if not memory_model.block_path_afforded(unit, budget):
                continue
            held = max(readings) * 1024
            billed, floor, unnamed = memory_model.charge_model(unit, jobs, held)
            fault = memory_model.charge_model_problem(unit, jobs, held)
            model_unnamed.append(unnamed)
            if fault:
                model_faults.append((fault, f"- **{label}** at `-m {token}`: {fault.text}"))
            model_rows.append(
                [
                    f"{label}, `-m {token}`",
                    f"{jobs}r",
                    formatting._fmt_budget_bytes(billed),
                    formatting._fmt_budget_bytes(floor) if floor else "—",
                    formatting.fmt_mib(held / 1024),
                    formatting._fmt_budget_bytes(unnamed),
                    # The band, not a bare "refuted": which line a cell crossed
                    # is what says whether it refutes the model.
                    "met" if fault is None else f"**{fault.band}**",
                ]
            )
    model_table = formatting.md_table(
        [
            "Leg",
            "Readers",
            "Billed",
            "of which pool list",
            "Worst rep held",
            "Unnamed",
            "Criterion",
        ],
        model_rows,
    )
    model_verdict = memory_model.charge_verdict(
        model_faults,
        model_unnamed,
        len(model_rows),
        nowhere=(
            "**The model was evaluated at no cell**: every flagless leg either declined the "
            "block path or was censored, so nothing here bears on the charge."
        ),
    )

    # -- the attribution: what the process says it held ---------------------
    #
    # This is the half no subtraction between whole runs produces. Every column
    # below is a number the process reported about itself, and the two families
    # do not cover the same memory: `live_*` is what passed through Rust's
    # `GlobalAlloc`, `mallinfo_*`/`malloc_*` are glibc's view of the whole
    # process, C included. The renderer keeps them apart and adds the one term
    # that sits between them — `liblzma`'s per-reader dictionary — by hand.
    def reported_median(spec: runspec.RunSpec, key: str) -> float | None:
        """One instrument field's median over this leg's surviving reps, or
        `None` where the leg reported none — which is the kill, since the report
        is written at exit."""
        values = [float(r[key]) for r in session.instrument_reports(figure, spec) if key in r]
        return formatting.median(values) if values else None

    def reported_bytes(spec: runspec.RunSpec, key: str) -> str:
        """A reading the report may lack, as bytes or a dash: a report
        from before mimalloc's statistics carries none of its keys."""
        value = reported_median(spec, key)
        return "—" if value is None else formatting._fmt_budget_bytes(value)

    def heap_cells(spec: runspec.RunSpec) -> list[str]:
        """One surviving leg's readings of each heap, in `two_heap_table`'s
        column order after `Peak RSS`: none subtracted from another."""
        return [
            reported_bytes(spec, "live_peak_bytes"),
            reported_bytes(spec, "mimalloc_committed_peak_bytes"),
            reported_bytes(spec, "mimalloc_reserved_peak_bytes"),
            reported_bytes(spec, "malloc_system_max"),
            f"{reported_median(spec, 'malloc_heaps') or 0.0:.0f}",
            formatting._fmt_budget_bytes(reported_median(spec, "mallinfo_fordblks") or 0.0),
            formatting._fmt_budget_bytes(reported_median(spec, "mallinfo_hblkhd") or 0.0),
        ]

    def instrument_check(
        spec: runspec.RunSpec, readings: Sequence[float], got: tuple[int, int] | None, where: str
    ) -> str | None:
        """The check: the same arrangement measured black-box. A term the
        instrument names has to show up in the sum the wrapper measures, and
        the two instruments share no mechanism — which is the independence
        two black-box sittings agreeing with each other never have.

        **The arrangement is the exact half and resident is the approximate
        one.** Whether the instrument build resolved the same reader count and
        budget is a yes or no, and a no means the attribution describes a run
        the shipped build does not make. Resident cannot be held to the
        shipped leg's own spread — this is a different binary, so its text and
        its allocator bookkeeping are its own — so the tolerance is stated
        rather than read off three reps of something else."""
        black_box = by_flagless.get((spec.input, spec.memory))
        if black_box is None or spec.command != black_box.command:
            return None
        theirs = session.get_rss(figure, black_box)
        if not theirs:
            return None
        readers = got[0] if got else 0
        delta = formatting.median(readings) - formatting.median(theirs)
        off = abs(delta) / formatting.median(theirs) * 100
        same = arrangement(black_box) == got
        return (
            f"- {where}: the instrument build resolved "
            + (
                f"the shipped build's arrangement, {readers} reader(s)"
                if same
                else "**a different arrangement** from the shipped build's, so what it "
                "attributes is not the run beside it"
            )
            + f", and held {formatting.fmt_mib(formatting.median(readings))} against "
            f"{formatting.fmt_mib_median_spread(theirs)} — {formatting.fmt_rss_delta(delta)}, {off:.0f}% "
            + (
                f"apart, inside the {INSTRUMENT_TOLERANCE_PCT}% a second build of the "
                "same source is allowed"
                if off <= INSTRUMENT_TOLERANCE_PCT
                else f"apart, **outside** the {INSTRUMENT_TOLERANCE_PCT}% a second "
                "build of the same source is allowed, so the sum the instrument "
                "decomposes is not the sum the gate measured"
            )
            + "."
        )

    instrument_rows, account_rows, account_points, checks = [], [], [], []
    # The same legs' readings for a two-heap report, one column a reading, each
    # headed with the heap it covers and none subtracted from another: readings
    # are not a model, so the reason the account is withheld does not reach
    # them (`docs/status/history/2026-10-05.md`).
    two_heap_rows = []
    for spec in instrument:
        readings = rss(spec)
        killed = session.kills(figure, spec)
        got = arrangement(spec)
        heap_max = reported_median(spec, "malloc_system_max")
        live_peak = reported_median(spec, "live_peak_bytes")
        if heap_max is None or live_peak is None or not readings:
            # A censored leg, or one whose reps all died before exit. It carries
            # the same third cell state the flagless axis uses, and enters
            # neither table's arithmetic.
            instrument_rows.append(
                [spec.label, "—", f"**OOM-killed**, {killed} rep(s)", "—", "—", "—", "—", "—"]
            )
            two_heap_rows.append(
                [spec.label, "—", f"**OOM-killed**, {killed} rep(s)", *(["—"] * 7)]
            )
            continue
        readers = got[0] if got else 0
        resident = formatting.median(readings) * 1024
        fordblks = reported_median(spec, "mallinfo_fordblks") or 0.0
        hblkhd = reported_median(spec, "mallinfo_hblkhd") or 0.0
        heaps = reported_median(spec, "malloc_heaps") or 0.0
        instrument_rows.append(
            [
                spec.label,
                f"{readers}r" if got else "—",
                formatting.fmt_mib_median_spread(readings),
                formatting._fmt_budget_bytes(heap_max),
                formatting._fmt_budget_bytes(live_peak),
                f"{heaps:.0f}",
                formatting._fmt_budget_bytes(fordblks),
                formatting._fmt_budget_bytes(hblkhd),
            ]
        )
        two_heap_rows.append(
            [
                spec.label,
                f"{readers}r" if got else "—",
                formatting.fmt_mib_median_spread(readings),
                *heap_cells(spec),
            ]
        )
        # The account, term by term. Each term is a high-water **of its own**,
        # so the sum bounds any single instant rather than describing one, and
        # the remainder is a residual of maxima. Said in the prose below, and
        # the reason no line here claims an identity.
        dictionaries = readers * memory_model.XZ_DICT_BYTES
        unattributed = heap_max - live_peak - dictionaries
        account_rows.append(
            [
                spec.label,
                formatting.fmt_mib(resident / 1024),
                formatting._fmt_budget_bytes(resident - heap_max),
                formatting._fmt_budget_bytes(live_peak),
                formatting._fmt_budget_bytes(dictionaries),
                formatting._fmt_budget_bytes(unattributed),
                (
                    f"{fordblks / unattributed * 100:.0f}%"
                    if unattributed > 0
                    else "—"
                ),
            ]
        )
        if not killed and got:
            account_points.append(
                (readers, live_peak, unattributed, fordblks, spec.label, got[1])
            )
        # The check (`instrument_check`).
        check = instrument_check(spec, readings, got, f"`-m {spec.memory}`")
        if check:
            checks.append(check)

    # -- the diagnostic legs (`RESERVE_DIAGNOSTIC_LIMITS`) --------------------
    # Each beside the charge its own run was billed, so the excess a heap
    # carries is read off the row; in no fit and no account.
    diagnostic_label, diagnostic_unit = next(
        (label, unit)
        for name, label, unit in shapes.RESERVE_FLAGLESS_INPUTS
        if name == RESERVE_DIAGNOSTIC_INPUT
    )
    diagnostic_rows, diagnostic_checks = [], []
    for spec in diagnostic:
        readings = rss(spec)
        killed = session.kills(figure, spec)
        got = arrangement(spec)
        if not readings or reported_median(spec, "live_peak_bytes") is None:
            diagnostic_rows.append(
                [spec.label, "—", "—", f"**OOM-killed**, {killed} rep(s)", *(["—"] * 7)]
            )
            continue
        diagnostic_rows.append(
            [
                spec.label,
                f"{got[0]}r" if got else "—",
                formatting._fmt_budget_bytes(memory_model.charge_bytes(diagnostic_unit, got[0])) if got else "—",
                formatting.fmt_mib_median_spread(readings)
                + (f" · **{killed} rep(s) OOM-killed**" if killed else ""),
                *heap_cells(spec),
            ]
        )
        check = instrument_check(
            spec, readings, got, f"{diagnostic_label}, `-m {spec.memory}`"
        )
        if check:
            diagnostic_checks.append(check)
    diagnostic_table = formatting.md_table(
        [
            "Leg",
            "Readers",
            "Billed — the charge for the count it resolved",
            "Peak RSS — the process",
            "Live high-water — Rust, what passed through `GlobalAlloc`",
            "mimalloc committed high-water — the Rust heap",
            "mimalloc reserved high-water — the Rust heap's address space",
            "glibc heap high-water — C `malloc` alone",
            "glibc arenas — C",
            "Freed and held at exit — glibc, C",
            "mmap-backed at exit — glibc, C",
        ],
        diagnostic_rows,
    )
    diagnostic_text = (
        (
            f"\n\n**What the shipped build holds above its charge at {diagnostic_label}**, "
            "said by the process: the flagless arrangement on the introspection build at the "
            "limits where this block size's unnamed remainder left `MEMORY_UNPOOLED_BOUND` on "
            "the shipped build alone, in no fit and no account above. `Billed` is "
            "`charge_bytes` for the count the leg itself resolved — every reader's block slot, "
            "chunk buffer and decoder retention, and the pool's list — and every other column "
            "is a reading of the heap its heading names, none subtracted from another. Rust's "
            "live high-water at or under `Billed` with mimalloc's committed high-water above "
            "it is mimalloc keeping what the program freed; a live high-water itself above "
            "`Billed` is the program; glibc's carrying the excess is C. A row where none of "
            "the three does attributes nothing, and is read so:\n\n"
            + diagnostic_table
            + (
                "\n\nThe same check against the shipped leg of each:\n\n"
                + "\n".join(diagnostic_checks)
                if diagnostic_checks
                else ""
            )
        )
        if diagnostic
        else ""
    )

    instrument_table = formatting.md_table(
        [
            "Leg",
            "Readers",
            "Peak RSS",
            "glibc heap high-water",
            "Rust live high-water",
            "Arenas",
            "Freed and held at exit",
            "mmap-backed at exit",
        ],
        instrument_rows,
    )
    two_heap_table = formatting.md_table(
        [
            "Leg",
            "Readers",
            "Peak RSS — the process",
            "Live high-water — Rust, what passed through `GlobalAlloc`",
            "mimalloc committed high-water — the Rust heap",
            "mimalloc reserved high-water — the Rust heap's address space",
            "glibc heap high-water — C `malloc` alone",
            "glibc arenas — C",
            "Freed and held at exit — glibc, C",
            "mmap-backed at exit — glibc, C",
        ],
        two_heap_rows,
    )
    account_table = formatting.md_table(
        [
            "Leg",
            "Peak RSS",
            "RSS − heap high-water",
            "Rust live high-water",
            "Decoder dictionaries",
            "Unattributed",
            "Covered by `fordblks`",
        ],
        account_rows,
    )

    # The program's own two terms, read off the counter rather than off a
    # difference of resident sets: `live_peak = fixed + readers x per_reader`,
    # **outside the block pool's retention list**, which `_fit_or_secant` takes
    # off first — the counter sees those buffers like any other Rust allocation,
    # so the charge's kink at `POOL_DEPTH` is in this series exactly as it is in
    # the resident one (`_depooled`). Evaluated inside its own window, at
    # the smallest arrangement, because an intercept is a physical quantity only
    # where the fit still holds where the mechanism is simplest.
    #
    # **It crosses the same publication guard the flagless axis does**, through
    # `_fit_or_secant`. This family's coverage is a *per-sitting* property where
    # the other's is a property of the axis: `RESERVE_INSTRUMENT_LIMITS` is
    # derived from `RESERVE_LIMITS` and cannot be narrowed on its own, but
    # `account_points` drops every killed leg and a kill takes the high-memory
    # end, so two kills leave two counts.
    #
    # **A leg that declined the block path is dropped from the line by name**,
    # as the flagless family drops one: it holds no block pool, so there is no
    # retention list to subtract and it is not on the same mechanism's line. No
    # registered limit declines at this block size today, which is what makes
    # this a guard rather than a filter.
    line_points = [
        pt for pt in account_points if memory_model.block_path_afforded(shapes.RESERVE_MECHANISM_UNIT, pt[5])
    ]
    live_counts = {r for r, *_ in line_points}
    if len(live_counts) >= 2:
        live_fixed, live_per_reader = _fit_or_secant(
            [(r, p / config.MIB) for r, p, _, _, _, _ in line_points], shapes.RESERVE_MECHANISM_UNIT
        )
        if live_fixed is None:
            # Named legs, not bare counts: the secant is a difference between
            # two runs, and a reader who cannot see which two cannot re-take it.
            ends = " → ".join(
                ", ".join(f"`{lab}`" for r, _, _, _, lab, _ in line_points if r == count)
                + f" at {count} reader(s)"
                for count in (min(live_counts), max(live_counts))
            )
            live_line = (
                f"**What a reader costs the program**, as a **secant** and not a fit: "
                f"**{live_per_reader:,.1f} MiB** a reader outside the pool over {ends}, "
                f"across the {len(line_points)} leg(s) that survived. Those cover "
                f"{len(live_counts)} distinct reader count(s), under the "
                f"{shapes.RESERVE_FIT_MIN_COUNTS} a two-term model needs before its intercept is a "
                "reading, so no fixed term and no residual are published."
            )
        else:
            smallest = min(line_points)
            at_smallest = live_fixed + live_per_reader * smallest[0]
            # The check `evidence` rule 2 asks for, and it is made against the
            # **remainder** at that leg rather than against its raw high-water,
            # because the remainder is what was fitted.
            held_smallest = _depooled(
                [(smallest[0], smallest[1] / config.MIB)], shapes.RESERVE_MECHANISM_UNIT
            )[0][1]
            live_line = (
                f"**What the program itself held outside the block pool**, least squares over "
                f"the {len(line_points)} leg(s) that survived, the pool's retention list "
                f"subtracted first because it is known before the sitting and bends the line "
                f"at `POOL_DEPTH`: fixed "
                f"**{live_fixed:,.0f} MiB**, a reader **{live_per_reader:,.1f} MiB**. At the "
                f"smallest arrangement in its own window — {smallest[0]} reader(s) — it "
                f"predicts {at_smallest:,.0f} MiB against {held_smallest:,.0f} MiB "
                f"measured, a residual of {abs(at_smallest - held_smallest):,.0f} MiB."
            )
        # The account against the model, which is the point of taking it: what a
        # reader costs the program, plus the C dictionary the counter is blind
        # to, against what `CompressedFileSource` bills a sub-stream. Computed here rather
        # than left to a reader, because it is the one comparison that says
        # whether the charge the budget rule divides by is the charge a reader
        # actually is. **It reads the slope alone**, which is why a secant keeps
        # it: refusing the whole line would withdraw this comparison in exactly
        # the censored sitting that needs it.
        #
        # **The two sides are commensurable only because the slope is the
        # remainder's.** `reader_bytes` is the per-worker term *without* the
        # pool's retention list — that list is `charge_bytes`' second term —
        # so a slope still carrying the list would be compared against a bill
        # that does not, which is a unit a reader of disagreement above
        # `POOL_DEPTH` (`_depooled`).
        measured_reader = live_per_reader * config.MIB + memory_model.XZ_DICT_BYTES
        billed = memory_model.reader_bytes(shapes.RESERVE_MECHANISM_UNIT)
        live_line += (
            f" Against the charge: {live_per_reader:,.1f} MiB of Rust outside the pool plus "
            f"the {formatting._fmt_budget_bytes(memory_model.XZ_DICT_BYTES)} dictionary is "
            f"**{formatting._fmt_budget_bytes(measured_reader)}** a reader, where "
            f"`BlockCache::reader_bytes` bills {formatting._fmt_budget_bytes(billed)} a reader — "
            f"{measured_reader / billed * 100:.0f}% of it."
        )
    else:
        live_line = (
            "**No line over the program's own high-water**: the surviving block-path legs "
            f"resolved {len(live_counts)} distinct reader count(s), and a line "
            "through one point is an intercept asserted as a measurement."
        )

    # A name, or an explicit no-name with the follow-up that would supply one.
    # The criterion is stated with the answer rather than applied silently: the
    # account's `Unattributed` column is *named* where glibc's own
    # freed-and-held figure covers at least half of it at every surviving leg.
    # A leg whose column is not positive has nothing to cover and counts as
    # covered, which is why the sentence qualifies "every leg" by sign.
    #
    # **It names that column and no other.** The charge table's `Unnamed` is
    # the bill subtracted from a black-box worst rep, a different subtraction on
    # a different build, and `fordblks` is not a coverage of it.
    covered = [
        (f / u if u > 0 else 1.0) for _, _, u, f, _, _ in account_points
    ]
    if covered and min(covered) >= 0.5:
        verdict = (
            "**The remainder has a name**: glibc's own `fordblks` — bytes the program freed, "
            "the allocator kept and the kernel still counts resident — covers at least half of "
            "the account's `Unattributed` column at every surviving leg where that column is "
            f"positive (worst {min(covered) * 100:.0f}%). That is the dynamic "
            "mmap threshold's signature and not program structure: a block-sized buffer stops "
            "being mmap-backed after the first one is freed, and the arena it lands in never "
            "returns it, which is why `hblkhd` reads zero on a run that decoded blocks "
            "throughout. It is not a coverage of the charge table's `Unnamed` column, which "
            "subtracts the bill from a black-box worst rep."
        )
    elif covered:
        verdict = (
            "**The remainder has no name here**, in those words: `fordblks` covers as little as "
            f"{min(covered) * 100:.0f}% of the account's `Unattributed` column, so what is left is neither the program's own live "
            "bytes, the decoder's dictionaries, nor allocator retention as glibc reports it. "
            "What would name it is `cd scripts && uv run measure.py --heaptrack-recipe`, which "
            "attributes every `malloc` to a call stack — C and Rust alike, on the `system` build it records — and a `--diff` "
            "between two of these arrangements would name the site rather than the term."
        )
    else:
        verdict = (
            "**The remainder has no name here**, in those words: every instrument leg was "
            "censored, so nothing was reported to attribute."
        )

    # -- the mechanism legs, against the flagless leg they differ from one ---
    reference = by_flagless[(shapes.RESERVE_MECHANISM_INPUT, RESERVE_MECHANISM_LIMIT)]
    ref_readings = session.get_rss(figure, reference)
    ref_killed = session.kills(figure, reference)
    ref_arrangement = arrangement(reference)
    ref_jobs = ref_arrangement[0] if ref_arrangement else 0

    def mech_cell(readings: Sequence[float], killed: int) -> str:
        """One mechanism leg's reading, or the fact that it was killed."""
        if not readings:
            return f"**OOM-killed**, {killed} rep(s)"
        return formatting.fmt_mib_median_spread(readings) + (
            f" · **{killed} rep(s) OOM-killed**" if killed else ""
        )

    mech_rows = [
        [
            "the reference — arenas uncapped",
            mech_cell(ref_readings, ref_killed),
            "—",
        ]
    ]
    for label, spec in mechanism:
        readings = rss(spec)
        killed = session.kills(figure, spec)
        # A delta against a censored reference, or from a censored leg, is a
        # difference of two things at least one of which is a bound. Said,
        # never printed as a number.
        if not readings or not ref_readings:
            against = "— · a censored leg is a bound, not a number"
        else:
            delta = formatting.median(readings) - formatting.median(ref_readings)
            against = f"{formatting.fmt_rss_delta(delta)} ({delta / formatting.median(ref_readings) * 100:+.0f}%)"
        mech_rows.append([label, mech_cell(readings, killed), against])
    mech_table = formatting.md_table(["Leg", "Peak RSS", "Against the reference"], mech_rows)

    # -- the path step ------------------------------------------------------
    step_rows, step_medians = [], []
    for spec in steps:
        readings = rss(spec)
        step_medians.append(formatting.median(readings))
        budget = int(spec.command.removeprefix(shapes.RESERVE_STEP_FAMILY))
        step_rows.append(
            [
                f"`--memory {memory_model.stated_allowance(budget)}` — {formatting._fmt_budget_bytes(budget)} of buffers"
                + (
                    " — one reader afforded"
                    if memory_model.block_path_afforded(shapes.RESERVE_MECHANISM_UNIT, budget)
                    else " — **one byte short**, block decode declined"
                ),
                formatting.fmt_mib_median_spread(readings),
            ]
        )
    step_table = formatting.md_table(["Leg", "Peak RSS"], step_rows)

    # -- the stated-budget axis ---------------------------------------------
    by_key = {
        (spec.input, spec.command.removeprefix(shapes.RESERVE_FAMILY).rpartition("-")[0],
         int(spec.command.rpartition("-")[2])): spec
        for spec in stated
    }
    rows, worst_stated = [], {}
    for name, source in shapes.RESERVE_INPUTS:
        for token, _, arena in shapes.RESERVE_ARENAS:
            cells = [f"{source}, {arena}"]
            for budget in shapes.RESERVE_BUDGETS:
                spec = by_key[(name, token, budget)]
                readings = rss(spec)
                reserve = formatting.median(readings) - budget / 1024
                worst_stated[token] = max(worst_stated.get(token, reserve), reserve)
                cells.append(f"{formatting.fmt_mib_median_spread(readings)} · {formatting.fmt_rss_delta(reserve)}")
            rows.append(cells)
    table = formatting.md_table(
        ["Leg", *(_fmt_budget(b) + " stated" for b in shapes.RESERVE_BUDGETS)], rows
    )

    base = rss(register._RESERVE_BASELINE)
    base_reserve = formatting.median(base) - (memory_model.LIBRARY_DEFAULT_BUDGET / 1024)
    # **The account above models one heap**: glibc's, holding the counter's
    # live bytes and `liblzma`'s dictionaries alike, which is what an
    # instrument counting in front of glibc reported. One counting in front of
    # mimalloc reports two (`mimalloc_scope`) — the Rust heap apart, glibc's
    # holding only what reached C `malloc` — so every subtraction above takes
    # the counter's bytes out of a heap that never held them. Withheld rather
    # than printed, and said in those words; the counter's own line stands,
    # being no allocator's. The two-heap account is `KD34`'s owner's to draw.
    two_heaps = any(
        "mimalloc_scope" in report
        for spec in instrument
        for report in session.instrument_reports(figure, spec)
    )
    if two_heaps:
        attribution = (
            "**What the process says it held**, on the introspection build running the "
            "same flagless shape as the axis above. These are this figure's instrument legs, "
            "declared in its register entry; the build takes atomics on every allocation and "
            "`pgdt --version` names it, so it is never timed. **It counted in front of "
            "mimalloc, so the process held two heaps**: the Rust heap is mimalloc's, and "
            "glibc's holds only what reached C `malloc`. The account this figure draws — the "
            "counter's high-water and the decoder dictionaries subtracted from glibc's — "
            "subtracts the Rust heap from a heap that never held it, so it is **withheld** "
            "here rather than printed; the two-heap account is `KD34`'s owner's to draw. "
            "**The readings stand**, a reading being no model: each column below is headed "
            "with the memory it covers, and none is subtracted from another. mimalloc's "
            "`committed` counts an arena's slices from when it first hands them out, touched "
            "or not, so it is no resident reading, and `reserved` is address space:\n\n"
            + two_heap_table
            + "\n\nWhat stands of the account is the counter's own line, which no allocator "
            "moves:\n\n"
            + live_line
        )
    else:
        attribution = (
            "**What the process says it held**, on the introspection build running the "
            "same flagless shape as the axis above. These are this figure's instrument legs, "
            "declared in its register entry; the build takes atomics on every allocation and "
            "`pgdt --version` names it, so it is never timed. The "
            "two families do not cover the same memory — the Rust column is what passed through "
            "`GlobalAlloc`, every glibc column is the whole process, C included — and the gap "
            "between them is decoder working set plus bookkeeping plus retention, never retention "
            "alone:\n\n"
            + instrument_table
            + "\n\n**The account, term by term.** Each term is a high-water *of its own*, so the "
            "row bounds any single instant rather than describing one, and the last column is a "
            "residual of maxima. `liblzma` allocates through C `malloc`, so its per-reader "
            f"dictionary — {memory_model.XZ_DICT_BYTES:,} bytes, read off a stack rather than modelled — is "
            "added back by hand: the counter cannot see it and glibc cannot separate it, and a "
            "decomposition that subtracted the two families would charge it to retention. Two "
            "columns can leave the range a resident term would keep, and both say the same thing: "
            "`RSS − heap high-water` goes **negative** where the arenas' summed high-water exceeds "
            "peak RSS, which it may, because `system max` is address space each arena obtained and "
            "no two arenas reach their maxima at once; and `fordblks` covers **more** than the "
            "remainder where retention at exit is larger than the gap between two maxima taken at "
            "different instants. Neither is an error in the reading — both are what "
            "non-simultaneity looks like, and they are why the last column is a share rather than "
            "a subtraction anyone should carry forward:\n\n"
            + account_table
            + "\n\n"
            + live_line
            + "\n\n"
            + verdict
        )

    return (
        "**What a flagless scan resolves, and what it then holds.** Each cell is peak resident "
        "set, the worker count and budget the run itself reported, and what the *worst* rep left "
        "of the allocation — which is the number a cgroup's killer reads, where the median is "
        "context. Nothing is stated on these legs: the container's limit is the whole input, "
        "which is why it is the axis.\n\n**Each cell also names the path it ran**, because the "
        "two hold different things and a column mixing them is two series printed as one: a leg "
        "takes the block path only where the budget it resolved affords one block-decoding "
        "reader of that file — `BlockCache::affordable`, which charges that reader's share "
        "of the pool's retention list with it, so the line is "
        f"{formatting._fmt_budget_bytes(memory_model.charge_bytes(shapes.RESERVE_FLAGLESS_INPUTS[0][2], 1))} at "
        f"{shapes.RESERVE_FLAGLESS_INPUTS[0][1]} and "
        f"{formatting._fmt_budget_bytes(memory_model.charge_bytes(shapes.RESERVE_FLAGLESS_INPUTS[-1][2], 1))} at "
        f"{shapes.RESERVE_FLAGLESS_INPUTS[-1][1]}. A *streaming* cell is a reading of the fallback "
        "decoder and belongs to no fit and no charge below.\n\n"
        + flagless_table
        + "\n\n"
        + gate_text
        + "\n**Resident against the reader count**, least squares over the legs that took the "
        "block path, the band being the same line over the per-rep extremes — a check on the "
        "charge's shape, not what either constant is read off. **Both terms are "
        "what a leg held *outside* the block pool's retention list**: that term is "
        "`(POOL_DEPTH.max(jobs) − 1) × unit`, known before the sitting and mirror-checked "
        "against the library's own constants, and it is constant below "
        f"`POOL_DEPTH` = {memory_model.LIBRARY_POOL_DEPTH} and grows by a unit a reader above — so a leg's "
        "resident rises by one unit a reader at the bottom of this axis and by two at the top, "
        "and both families straddle the bend. Subtracting it and fitting the remainder is how a "
        "known term stays out of the intercept; fitting one straight line across the kink "
        "instead reads the intercept ≈49 MiB high at 24 MiB blocks and ≈421 MiB high at 128, "
        "and the slope 31% low. Each line names the "
        "window it covers, since a leg is censored exactly when its resident ran closest to its "
        "ceiling and a fit over what survives is a fit over the legs that had room. A family "
        f"covering fewer than {shapes.RESERVE_FIT_MIN_COUNTS} distinct reader counts publishes a "
        "**secant** instead — the slope between its two ends, with no fixed term and no "
        "residual: the remainder still has two terms, so below that the residual printed beside "
        "it is zero by construction rather than a reading, while the slope is a difference the "
        "axis measured:\n\n"
        + "\n".join(fits)
        + (
            "\n\n**What the killed legs still prove**, stated as constraints and **not** fitted: "
            "a kill is the one reading here that says resident is *above* a number rather than "
            "at one, and interval censoring is the right treatment of it at the wrong size "
            f"for {len(shapes.RESERVE_LIMITS)} legs. This is the end of the axis the fit above does "
            "not cover:\n\n"
            + "\n".join(constraints)
            if constraints
            else ""
        )
        + "\n\n**The charge against what was held**, cell by cell, which is the check this "
        "figure runs rather than a constant it searches for. The criterion has three lines and "
        "all of them are registered before the sitting: the unnamed remainder must be "
        "**non-negative**, a negative one being an over-bill — bytes the rule charged that "
        "nothing holds, and so a reader the allocation would have afforded — and it is read "
        "against two ceilings, which say different things. Above "
        f"**`MEMORY_UNPOOLED_BOUND`** ({formatting._fmt_budget_bytes(memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND)}), the "
        "number `margin_allowance` predicts a count's resident with, the allocation still holds "
        "and the **bound** is wrong; above "
        f"**`MEMORY_RESERVE`** ({formatting._fmt_budget_bytes(memory_model.LIBRARY_MEMORY_RESERVE)}), which is by "
        "construction what covers everything the charge does not bill, the **rule** is. The "
        "`Criterion` column names the band rather than saying only that a cell faulted, because "
        f"the three are read differently: a cell in the `{memory_model.BAND_BOUND}` band is a finding the "
        "sitting discharges itself, re-deriving the bound from these same remainders, while a "
        "cell above the reserve is an arrangement the discovery cannot keep inside its "
        "allocation and an over-bill is a charge nothing holds — neither of those two is "
        "apparatus scatter, and neither leaves the sitting anything to repair, so both refute "
        "the model. None of the three keeps the table out of this document. The "
        "middle column is the "
        "second term of that bill, reported apart because it is the one unbounded in the block "
        "size: `BufferPool::slots` clamps the block pool at `POOL_DEPTH.max(jobs)` with "
        f"`POOL_DEPTH` = {memory_model.LIBRARY_POOL_DEPTH} and `BlockCache::slot` drains to one below it "
        "before taking the buffer `Decoding::land` pushes back, so the pool holds "
        "`(POOL_DEPTH.max(jobs) − 1) × unit` on top of the block each reader has in flight — "
        "billed at **every** count (`parallelism.rs`, `WorkerMemory`). What is left is what the charge "
        "does not bill, and this column does not name it: the instrument legs below attribute "
        "a remainder of their own, which is a different subtraction on a different build. A "
        "leg that declined the block path is absent, holding none of these "
        "terms; a censored one is absent too, its reading being a bound:\n\n"
        + model_table
        + "\n\n"
        + model_verdict
        + "\n\n"
        + attribution
        + (
            "\n\n**The check, which is what makes the two instruments independent**: the "
            "black-box legs measure the same arrangement through `getrusage`, sharing no "
            "mechanism with the report above, so a term the process names has to show up in "
            "the sum the wrapper measures.\n\n" + "\n".join(checks)
            if checks
            else ""
        )
        + diagnostic_text
        + f"\n\n**What each mechanism moves**, at one block size and one allocation — "
        f"`{shapes.RESERVE_MECHANISM_INPUT}` flagless in `-m {RESERVE_MECHANISM_LIMIT}`, which resolved "
        + (f"{ref_jobs} readers" if ref_arrangement else "a count it never lived to report")
        + ". The reference is the axis row above, not a re-take. **One leg, where three were "
        "registered**: the arena cap bounds how many arenas can hold a retained block, and "
        "at a cell whose uncapped process already runs no more arenas than the cap allows it "
        "cannot move one — the instrument's `Arenas` column says which this cell is."
        + (
            " **On this build the cap reaches C alone**: the instrument counted in front of "
            "mimalloc, which it is refused beside any other allocator, so the shipped build's "
            "Rust heap — the block buffers included — is mimalloc's, and glibc's arenas hold "
            "only what C allocates, `liblzma`'s decoder state among it."
            if two_heaps
            else ""
        )
        + " The two allocator legs are dropped rather than re-aimed "
        "because jemalloc and mimalloc do not have glibc's dynamic mmap threshold — swapping "
        "them removes the mechanism instead of measuring it. What replaced them is the "
        "instrument above, which reports the retention rather than differencing two runs:\n\n"
        + mech_table
        + "\n\n**What the block path costs against the streaming fallback**, one byte of budget "
        f"apart in the same {RESERVE_MECHANISM_LIMIT} allocation at `--jobs {shapes.RESERVE_JOBS}`: "
        f"{formatting.fmt_rss_delta(step_medians[0] - step_medians[-1])} between the two, "
        f"{step_medians[0] / max(step_medians[-1], 1):.1f}×. `BlockCache::affordable` compares "
        "the budget against what **one** reader costs — the per-reader term plus that one "
        "reader's share of the pool's retention list — so the pair straddles that "
        f"comparison at {formatting._fmt_budget_bytes(memory_model.charge_bytes(shapes.RESERVE_MECHANISM_UNIT, 1))} and differs "
        "in nothing else. It is what an operator deciding whether to set `--memory` "
        "needs, and no other figure states it:\n\n"
        + step_table
        + f"\n\n**The stated-budget axis.** Each cell "
        f"is peak resident set, then that reading **minus the read-buffer budget the run's "
        f"`--memory` states** — what the reserve has to cover. Where the margin lowers a typed "
        f"count's budget below the one stated, the cell understates what the run holds above "
        f"the budget it resolved, which the run reports. Every row states `--jobs {shapes.RESERVE_JOBS}` in a {shapes.PARALLEL_MEMORY} container, an "
        f"apparatus departure from the register's 512 MB, which is smaller than the largest "
        f"budget under test; the flagless legs above each carry their own allocation instead.\n\n"
        + table
        + f"\n\n**The worst cell of each arena leg**, which is what a `--jobs {shapes.RESERVE_JOBS}` scan "
        "holds above the budget its `--memory` states, the count typed so that the margin "
        "lowers only the budget — not the flagless arrangement `MEMORY_RESERVE` was read "
        "off, which the legs above are: "
        + ", ".join(
            f"{arena} **{formatting.fmt_rss_delta(worst_stated[token])}**"
            for token, _, arena in shapes.RESERVE_ARENAS
        )
        + ".\n\n"
        f"**The shipped serial arrangement is read beside it and not on either axis**: "
        f"`control` at `--jobs 1` with no budget stated runs at the library's "
        f"{_fmt_budget(memory_model.LIBRARY_DEFAULT_BUDGET)} default and holds "
        f"{formatting.fmt_mib_median_spread(base)}, a reserve of **{formatting.fmt_rss_delta(base_reserve)}**. That "
        "run is `peak-rss`'s own `control` row — the same binary, command, input and regime — "
        "so it is one reading this table shares with that one rather than a second "
        "measurement of it.\n"
        + note
        + "\nPer-rep readings (peak RSS):\n"
        + "\n".join(per_rep)
        + "\n"
    )
