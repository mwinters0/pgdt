"""What each figure family's legs report in a dry run (`Figure.stand_in`): a stand-in a renderer can
read, so a dry run reaches every division and refusal a table performs. Each is plausible and
never the library's arithmetic, so nothing here grows into a second authority on what a run
resolves; where a table holds a report to the harness's own mirror of it, the stand-in reports
the mirror's answer. A leg outside the family a function names reports nothing."""

from __future__ import annotations

import hashlib
from typing import TYPE_CHECKING

from measurement import config
from measurement import gzip_charge
from measurement import inputs
from measurement import shapes
from measurement.figures.declaration import StandIn

if TYPE_CHECKING:
    from measurement.runspec import RunSpec

NOTHING = StandIn({})


def decode(spec: RunSpec, digest: bytes) -> StandIn:
    """The `xz_decode` example's counts, which the decode table divides by."""
    if not spec.command.startswith("decode-"):
        return NOTHING
    return StandIn(
        {
            "plaintext": str(3 * config.GIB),
            "delivered": str(3 * config.GIB),
            "workers": spec.command.rpartition("-")[2],
            "blocks": str(shapes.DECODE_WORKERS[-1] * 8),
        }
    )


def dynamic_filter(spec: RunSpec, digest: bytes) -> StandIn:
    """One answer per query, alike in both legs, and the startup leg's,
    which the table refuses to render otherwise."""
    if spec.command == shapes.DYNFILTER_STARTUP:
        return StandIn({"startup_answer": "1"})
    if not spec.command.startswith(shapes.DYNFILTER_FAMILY):
        return NOTHING
    query = spec.command.rpartition("-")[0]
    return StandIn(
        {
            "result_rows": "1",
            "result_first": "100",
            "result_digest": hashlib.sha256(query.encode()).hexdigest(),
        }
    )


def parallel_scan(spec: RunSpec, digest: bytes) -> StandIn:
    """One answer for every provider leg (`PARALLEL_SCAN`, whose prefix the
    prebuilt family's shares), which `parallel_answer_problems` refuses
    otherwise."""
    if not spec.command.startswith(shapes.PARALLEL_SCAN):
        return NOTHING
    return StandIn(
        {
            "result_rows": "1",
            "result_first": "100",
            "result_digest": hashlib.sha256(shapes.PARALLEL_SCAN_SQL.encode()).hexdigest(),
        }
    )


def pruning(spec: RunSpec, digest: bytes) -> StandIn:
    """What the query's own notes say: the pruning table divides by them and
    refuses a pruned leg that skipped nothing — or, for the filter its
    statistics cannot narrow, one that skipped anything, and a leg over a
    cache without statistics that consulted any."""
    if not spec.command.startswith(shapes.PRUNING_FAMILY):
        return NOTHING
    pruned = spec.command.endswith("-all")
    skipped = (
        0
        if spec.command.startswith(f"{shapes.PRUNING_FAMILY}{shapes.PRUNING_UNNARROWED}-")
        else 3060
    )
    return StandIn(
        {
            "rows_returned": "3000",
            **(
                {
                    "skipped_groups": str(skipped),
                    "groups": "3072",
                    "skipped_bytes": str(skipped * config.MIB),
                    "bytes": str(3072 * config.MIB),
                }
                if pruned
                else {}
            ),
        }
    )


def gzip_first_pass(spec: RunSpec, digest: bytes) -> StandIn:
    """A resolution: the first-pass table refuses a leg whose run resolved
    another count, or a gzip leg that opened no first pass and a plain one
    that did."""
    if not spec.command.startswith(shapes.GZPASS_FAMILY):
        return NOTHING
    jobs = spec.command.removeprefix(shapes.GZPASS_FAMILY)
    return StandIn(
        {
            "resolved_jobs": jobs,
            **({} if spec.input == "control" else {"first_pass_workers": jobs}),
        }
    )


def gzip_resident(spec: RunSpec, digest: bytes) -> StandIn:
    """A resolution and its bill: the resident table refuses a leg whose run
    resolved another count or billed other than the mirror charges
    (`gzip_charge.gzip_leg_charge`), a first-pass leg that opened no first
    pass, and a block-path leg that did. Billed over the leg's nominal size,
    which a first pass's charge reads only for whether it is past one chunk."""
    if not spec.command.startswith(shapes.GZRSS_FAMILY):
        return NOTHING
    kind = {leg: kind for leg, _, kind in shapes.GZIP_RSS_LEGS}[spec.input]
    jobs = spec.command.removeprefix(shapes.GZRSS_FAMILY)
    nominal = inputs.INPUTS[spec.input].nominal_bytes
    if nominal is None:
        raise ValueError(f"{spec.input} declares no nominal size to bill a dry run over")
    billed, _ = gzip_charge.gzip_leg_charge(kind, int(jobs), nominal)
    return StandIn(
        {
            "resolved_jobs": jobs,
            "resolved_budget": str(billed),
            **({"first_pass_workers": jobs} if kind == "pass" else {}),
        }
    )

def long_line(spec: RunSpec, digest: bytes) -> StandIn:
    """An answer or a resolution: the figure refuses a query or filter leg
    answering other than its input's rows, and a parse leg resolving another
    count. A factor leg's report names a plan's factor, varied by the rep's
    bytes so the figure's largest is one kind's."""
    if not spec.command.startswith((shapes.LONG_RSS_FAMILY, shapes.LONG_FACTOR_FAMILY)):
        return NOTHING
    rest = spec.command.removeprefix(shapes.LONG_RSS_FAMILY).removeprefix(shapes.LONG_FACTOR_FAMILY)
    name, _, jobs = rest.rpartition("-") if "-" in rest else (rest, "", "")
    answer = str(inputs.long_line_answer(spec.input))
    reported = (
        {"result_rows": "1", "result_first": answer}
        if name == "query"
        else {"rows_returned": answer}
        if name == "filter"
        else {"resolved_jobs": jobs}
    )
    planned = (
        {"planned_factor": f"{1 + digest[4] % 9}/{2}"}
        if spec.command.startswith(shapes.LONG_FACTOR_FAMILY)
        else {}
    )
    return StandIn(reported, planned)


def gzip_spacing(spec: RunSpec, digest: bytes) -> StandIn:
    """An index: the spacing table divides by the unit count, and refuses
    stages that disagree on it or a `starts` leg that began fewer units than
    it holds."""
    if not spec.command.startswith(shapes.GZSPACING_FAMILY):
        return NOTHING
    stage, _, mib = spec.command.removeprefix(shapes.GZSPACING_FAMILY).partition("-")
    units = -(-3 * config.GIB // (int(mib) * config.MIB))
    return StandIn(
        {
            "units": str(units),
            "windows": str((units - 1) * 32 * 1024),
            **({"started": str(units)} if stage == "starts" else {}),
        }
    )


def reserve(spec: RunSpec, digest: bytes) -> StandIn:
    """The flagless legs' resolution, whose limit is their axis: the count
    the flagless table fits over exists only in what a real run reports.
    Monotone in the limit, so the fit is well conditioned."""
    tokens = [token for token, _ in shapes.RESERVE_LIMITS]
    if spec.memory not in tokens:
        return NOTHING
    rank = 1 + tokens.index(spec.memory)
    return StandIn({"resolved_jobs": str(3 * rank), "resolved_budget": str(3 * rank * (65 << 20))})
