"""`long-line-resident`: what a pass holds resident over long lines, and the factors its allowance
is sized from."""

from __future__ import annotations

from fractions import Fraction
from typing import Sequence
from typing import TYPE_CHECKING

import generate_long_lines as long_lines

from measurement import formatting
from measurement import inputs
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.timing import Session


def fmt_factor(factor: Fraction) -> str:
    """A planned factor as the instrument writes it: `held/per`, a whole one
    bare, `none` for zero."""
    if not factor:
        return "none"
    return str(factor.numerator) if factor.denominator == 1 else f"{factor.numerator}/{factor.denominator}"


def parse_planned_factor(text: str) -> Fraction:
    """A report's `planned_factor`, `held/per` or `none`, as bytes held beside
    a line per byte of it, `none` being zero."""
    if text == "none":
        return Fraction(0)
    held, sep, per = text.partition("/")
    if not sep or not held.isdigit() or not per.isdigit() or int(per) == 0:
        raise ValueError(f"{text!r} is no planned factor")
    return Fraction(int(held), int(per))

#: The length each kind's instrument leg reads its plan's factor over: the
#: ladder's shortest, a plan's factor being the schema's and its filter's,
#: whatever the lines' length.
LONG_LINE_FACTOR_LENGTH = inputs.LONG_LINE_LENGTHS[0][0]


def _long_factor_specs() -> list[runspec.RunSpec]:
    """Each kind's instrument leg: its planned pass (`LONG_LINE_PLANNED`) on
    the introspection build over its `LONG_LINE_FACTOR_LENGTH` input, its
    report naming the factor the plan settled. **Swept in its kind's ladder
    sweep, ahead of every timed leg**, whose allowance it sizes: a sweep of
    their own would stage each input twice with no subtraction pairing it."""
    labels = dict(shapes.LONG_LINE_PASSES)
    return [
        runspec.RunSpec(
            "introspect",
            inputs.long_line_input(kind, LONG_LINE_FACTOR_LENGTH),
            f"{shapes.LONG_FACTOR_FAMILY}{name}",
            "warm-parallel",
            f"{kind}, the factor its {labels[name]} planned",
            instrument=True,
        )
        for kind in long_lines.KINDS
        for name in shapes.long_line_passes(kind)
        if name in shapes.LONG_LINE_PLANNED
    ]


def _long_line_specs(allowance: int) -> list[runspec.RunSpec]:
    """Every timed leg, by kind then sweep, each stating `allowance`: each
    length of a kind's first sweep at each of its counts, then its second
    sweep's — the serial-only lengths and their anchor, at one worker, the
    anchor's legs naming the sweep (`in_sweep`)."""
    figure = "long-line-resident"
    labels = dict(shapes.LONG_LINE_PASSES)
    specs = []
    gi = 0
    for kind in long_lines.KINDS:
        for k, group in enumerate(register.long_line_groups(kind)):
            for length in group:
                anchor = k == 1 and length not in inputs.LONG_LINE_SERIAL_ONLY
                jobs_read = (1,) if anchor else shapes.long_line_jobs(kind, length)
                where = (
                    f"{length} MiB lines" if length.isdigit() else "lines at the bound"
                ) + (" beside the bound" if anchor else "")
                for name in shapes.long_line_passes(kind):
                    for jobs in jobs_read:
                        spec = runspec.RunSpec(
                            "dfcli" if name == "query" else "pgdt",
                            inputs.long_line_input(kind, length),
                            f"{shapes.LONG_RSS_FAMILY}{name}-{jobs}",
                            "warm-parallel",
                            f"{kind}, {where}, {labels[name]}, {jobs}{long_line_unit(name)}",
                            allowance=allowance,
                        )
                        specs.append(staging.in_sweep(figure, spec, gi) if anchor else spec)
            gi += 1
    return specs


def long_line_unit(name: str) -> str:
    """What a pass's count is of: the provider's partitions, or workers."""
    return "p" if name == "query" else "j"


def long_line_secant(lo: Sequence[float], hi: Sequence[float], span: int) -> float:
    """The resident each line byte added between two lengths `span` bytes
    apart, from one pass's readings in KiB at each: their medians' difference
    over the span, in bytes a line byte. **A secant over two lengths, not a
    fit**, read at every other length by `long_line_residual`."""
    return (formatting.median(hi) - formatting.median(lo)) * 1024 / span


def long_line_residual(
    per_byte: float, anchor: Sequence[float], at: Sequence[float], span: int
) -> float:
    """What a secant of `per_byte` misses a length's median by, measured less
    predicted, in KiB: the prediction is `anchor`'s median, in the same sweep,
    plus the secant over the `span` bytes from the anchor's length to this
    one's, negative below it.

    **Read inside the data, where the mechanism is simplest, and past it**
    (`.claude/skills/evidence/SKILL.md`, rule 2): a residual larger than the
    shortest line says resident is not linear in the line there, and no factor
    describes it."""
    return formatting.median(at) - (formatting.median(anchor) + per_byte * span / 1024)


def long_line_factors(
    session: Session, specs: Sequence[runspec.RunSpec], figure: str = "long-line-resident"
) -> dict[str, Fraction]:
    """Each kind's planned factor, off its instrument leg's reports
    (`_long_factor_specs`) as `figure` swept them. **Refused, naming each,
    where a leg's reports name none, or disagree**: the factor is a plan's,
    the same every rep, and a leg that planned nothing, or answered other
    than its input's rows, read no factor of the figure's query."""
    factors: dict[str, Fraction] = {}
    problems = []
    for spec in specs:
        kind = spec.input.split("_")[1]
        name = spec.command.removeprefix(shapes.LONG_FACTOR_FAMILY)
        planned = {report.get("planned_factor") for report in session.instrument_reports(figure, spec)}
        if len(planned) != 1 or None in planned:
            problems.append(f"{spec.label}: its reports name the factors {sorted(map(str, planned))}")
            continue
        report = session.reported.get(spec.key(figure), {})
        want = str(inputs.long_line_answer(spec.input))
        got = report.get("result_first") if name == "query" else report.get("rows_returned")
        if got != want:
            problems.append(f"{spec.label}: it answered {got or 'nothing'} where its table holds {want}")
            continue
        factors[kind] = parse_planned_factor(planned.pop())
    if problems:
        raise RuntimeError(f"{figure} cannot read its factors: " + "; ".join(problems))
    return factors


def long_line_leg_problems(session: Session, figure: str, specs: Sequence[runspec.RunSpec]) -> list[str]:
    """Why `figure`'s timed long-line legs (`LONG_RSS_FAMILY`) did not read
    the arrangement their rows name, one line each: a query or filter that
    did not answer its input's rows (`long_line_answer`), or a parse that
    resolved another worker count than its row states. A query leg's
    partition count is checked by nothing: the first `scan started` in its
    log is the untimed builder's."""
    problems = []
    for spec in specs:
        name, _, jobs = spec.command.removeprefix(shapes.LONG_RSS_FAMILY).rpartition("-")
        report = session.reported.get(spec.key(figure), {})
        want = str(inputs.long_line_answer(spec.input))
        if name == "query":
            if report.get("result_rows") != "1" or report.get("result_first") != want:
                problems.append(
                    f"{spec.label}: the query counted {report.get('result_first', 'nothing')} "
                    f"values where its table holds {want}"
                )
        elif name == "filter":
            if report.get("rows_returned") != want:
                problems.append(
                    f"{spec.label}: the filter kept {report.get('rows_returned', 'no')} rows "
                    f"where its table holds {want}, every one of which it keeps"
                )
        elif report.get("resolved_jobs") != jobs:
            problems.append(
                f"{spec.label}: the run resolved {report.get('resolved_jobs', 'no')} worker(s) "
                f"where the row states {jobs}"
            )
    return problems


def run_long_line_resident(session: Session) -> str:
    """Peak resident set of each pass over a cluster of long lines, by value
    kind, line length and count: what a pass holds beside a line it holds,
    on code that charges none of it.

    **What it prices is the copies and the factors**, the phase's spec's
    account ("What a held line costs"): a parse streams its line, holding at
    most the scalar text it collects; a query holds its line and its decode,
    bounded by its plan's factor — the largest of its columns' decode factors
    and its terms' key factors (`docs/design/decisions.md`, "D154") — which
    each planned kind's instrument leg reads off the plan
    (`LONG_FACTOR_FAMILY`) and every leg's allowance is sized from
    (`long_line_allowance`). The term kinds (`LONG_LINE_TERM_KINDS`) read the
    terms no count of Arrow's buffers sees. This figure is read again once
    the ledger charges a held line, which confirms the factors or re-sets
    them.

    **What would falsify the account**, written before any reading:

    - a one-worker parse whose secant is past one: a parse streams its line
      (D138), holding no `text`, `bytea` or escaped field's bytes;
    - a one-partition query or filter whose secant is under one: its line is
      held whole while its row is decoded (`docs/design/decisions.md`, "D23")
      — an instrument not seeing the line, before it is a finding;
    - a one-partition query or filter whose secant is past one plus twice its
      planned factor: it then holds a term the factor misses beyond its
      builders' doubling, which the factor leaves uncharged (D141) and a
      reserved builder removes past a chunk term (D154); between one plus the factor and that, the
      doubling is the candidate, to be named rather than assumed;
    - a one-worker or one-partition cell whose residual at 32 MiB is larger
      than that line: resident is then not a factor times the line, and no
      charge factor is read off it;
    - a residual at 288 MiB differing from the 32 MiB one by more than a
      32 MiB line, at either count: past a chunk the peak then follows a
      buffer's doubling rather than the line, and a factor read off the
      power-of-two ladder is a factor of the growth at one phase of it. At
      four it reads how many lines the cut holds at once, which a factor,
      read at one, never answers: no factor is re-set on a four-reader fire;
    - a residual at the bound larger than a 32 MiB line: the ladder's secant
      does not reach the bound, and what one reader holds there, the floor's
      question, is that row's reading, not a factor times the line;
    - a four-worker secant past four times its one-worker one, more than one
      held line a worker. Between one and four times, the ratio is how many
      lines were held at once, which the leader's cut decides and nothing here
      assumes.

    **No gate twin**: the container is headroom (`LONG_LINE_MEMORY`). A kill
    is still a kill, run once on the gate build before the error is raised.
    **Three reps**, as the other resident figures take: a peak is steady
    across reps, and the reps catch an outlier. **One rep of each instrument
    leg**, a plan's factor being the same every rep.
    """
    figure = "long-line-resident"
    factor_specs = _long_factor_specs()
    to_read = [s for s in factor_specs if not session.has(figure, s)]
    session.build_before_sweep(to_read)
    session.sweep(figure, to_read, session.cfg.reps(1))
    factors = long_line_factors(session, factor_specs)
    allowance = shapes.long_line_allowance(factors)

    specs = _long_line_specs(allowance)
    to_sweep = [s for s in specs if not session.has(figure, s)]
    session.build_before_sweep(to_sweep)
    session.sweep(figure, to_sweep, session.cfg.reps(3))

    problems = long_line_leg_problems(session, figure, specs)
    if problems:
        raise RuntimeError(f"{figure} cannot read its legs: " + "; ".join(problems))
    # Keyed by kind, length, pass, count, and whether it is the anchor
    # measured beside the bound.
    readings: dict[tuple[str, str, str, int, bool], list[float]] = {}
    for spec in specs:
        name, _, jobs = spec.command.removeprefix(shapes.LONG_RSS_FAMILY).rpartition("-")
        _, kind, length = spec.input.split("_")
        readings[(kind, length, name, int(jobs), spec.sweep is not None)] = session.get_rss(
            figure, spec
        )

    def row_name(length: str) -> str:
        return f"{length} MiB" if length.isdigit() else f"the bound, {inputs.LONG_LINE_BOUND:,} B"

    lo, hi = inputs.LONG_LINE_SECANT
    read = {(name, jobs) for _, _, name, jobs, _ in readings}
    columns = [
        (name, label, jobs)
        for name, label in shapes.LONG_LINE_PASSES
        for jobs in shapes.LONG_LINE_JOBS
        if (name, jobs) in read
    ]
    headers = [
        "Lines",
        *(f"{label}, {jobs}{long_line_unit(name)}" for name, label, jobs in columns),
    ]

    def cell(key: tuple[str, str, str, int, bool]) -> str:
        return formatting.fmt_mib_median_spread(readings[key]) if key in readings else "—"

    rows = []
    secants = []
    for kind in long_lines.KINDS:
        ladder, serial = register.long_line_groups(kind)
        for length in ladder:
            rows.append(
                [f"{kind}, {row_name(length)}"]
                + [cell((kind, length, name, jobs, False)) for name, _, jobs in columns]
            )
        for length in serial:
            anchor = length not in inputs.LONG_LINE_SERIAL_ONLY
            rows.append(
                [f"{kind}, {row_name(length)}" + (", beside the bound" if anchor else "")]
                + [cell((kind, length, name, jobs, anchor)) for name, _, jobs in columns]
            )
        cells = [kind]
        for name, _, jobs in columns:
            r = {length: readings.get((kind, length, name, jobs, False)) for length in ladder}
            if r.get(lo) is None or r.get(hi) is None:
                cells.append("—")
                continue
            per_byte = long_line_secant(
                r[lo], r[hi], inputs.long_line_bytes(hi) - inputs.long_line_bytes(lo)
            )
            parts = [f"**×{per_byte:.2f}**"]
            if name in shapes.LONG_LINE_PLANNED:
                f = factors[kind]
                parts[0] += (
                    f" (planned ×{float(1 + f):.2f}, one and {fmt_factor(f)})"
                    if f
                    else " (planned ×1, no factor)"
                )
            parts += [
                f"{length}: "
                + formatting.fmt_rss_delta(
                    long_line_residual(
                        per_byte, r[lo], r[length], inputs.long_line_bytes(length) - inputs.long_line_bytes(lo)
                    )
                )
                for length in ladder
                if length not in inputs.LONG_LINE_SECANT
            ]
            for length in serial:
                at = readings.get((kind, length, name, jobs, False))
                if length in inputs.LONG_LINE_SERIAL_ONLY and at is not None:
                    anchor = readings[(kind, hi, name, jobs, True)]
                    span = inputs.long_line_bytes(length) - inputs.long_line_bytes(hi)
                    parts.append(
                        f"{length}: " + formatting.fmt_rss_delta(long_line_residual(per_byte, anchor, at, span))
                    )
            cells.append(" · ".join(parts))
        secants.append(cells)

    sizes = ", ".join(
        f"`{name}` " + formatting._fmt_bytes(session.input_size(name, "warm-parallel"))
        for name in inputs.LONG_LINE_INPUTS
    )
    planned = ", ".join(f"`{kind}` {fmt_factor(f)}" for kind, f in factors.items())
    notes = (
        "\n\nEach cell is peak resident set; a length at the bound is read at one worker or "
        "partition alone, and a term's kind at one alone. "
        f"Every input is one table of {long_lines.SHORT_ROWS} short rows, a cluster of {inputs.LONG_LINE_CLUSTER} "
        "consecutive rows each a line of the length its row names, terminator included, and "
        f"{long_lines.SHORT_ROWS} short rows (`generate_long_lines.py`): `text` decodes to "
        "itself, `escaped` to seven bytes in eight, `bytea` to half; `int8array` is a "
        "`bigint[]` of single digits, `textarray` a `text[]` of one-letter elements and `jsonb` "
        "an object of empty keys, each the densest text its term is held for. Every timed leg "
        f"states `--memory {allowance}`, `parallel-peak-rss`'s allowance and the widest leg's "
        "lines held at once at their charge — a parse's its line, a query's or filter's its line "
        "and its kind's planned factor — every line read to I71's bound, "
        f"`{inputs.LONG_LINE_BOUND}`, which no leg states, in a {shapes.LONG_LINE_MEMORY} container — "
        "an apparatus departure from the register's 512 MB, which one held line outgrows. A "
        f"parse states `{shapes.GATHER_STATISTICS}`, and the strict one "
        "`--postgres-invalid-values strict` beside it. The query is `pgdt sql`'s "
        f"`{shapes.LONG_LINE_SQL}` at the column's count of partitions, and the ordering filter "
        f"`pgdt query`'s `--filter '{shapes.LONG_LINE_FILTER}'` projecting `id`, each over a cache an "
        "untimed `parse` wrote in the same container, its answer held to the input's rows. Each "
        "kind is two sweeps: the lengths read at each count, and the bound "
        f"beside the {hi} MiB input again, its row marked so.\n\n"
        f"**Each planned factor**, read off its plan by the introspection build over the "
        f"{LONG_LINE_FACTOR_LENGTH} MiB input, in its kind's first sweep ahead of every "
        f"timed leg: {planned}.\n\n"
        f"Inputs: {sizes}.\n\n"
        f"**Each pass's secant**: the resident a line byte added between the {lo} MiB and "
        f"{hi} MiB rows, beside a query's or filter's one plus its planned factor, then what it "
        "misses each other length's median by, measured less predicted — the bound's from the "
        f"{hi} MiB row of its own sweep. At four workers or partitions it sums every line held "
        "at once.\n\n"
        + formatting.md_table(["Kind", *headers[1:]], secants)
        + "\n\nPer-rep readings (peak RSS):\n"
        + "\n".join(
            f"- {spec.label}: " + ", ".join(formatting.fmt_mib(v) for v in session.get_rss(figure, spec))
            for spec in specs
        )
        + "\n"
    )
    return formatting.md_table(headers, rows) + notes
