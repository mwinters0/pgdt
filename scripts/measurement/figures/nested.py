"""`nested-end-to-end` and `cross-file-floor`: what a typed query over nested columns costs a row."""

from __future__ import annotations

from typing import TYPE_CHECKING

from measurement import formatting
from measurement import runspec
from measurement import staging
from measurement.figures import prose
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.timing import Session


_NESTED_FILES = {
    "control": "control — 16 scalar columns",
    "composite": "`--composite` — the same 16 plus one composite",
    "arrays": "`--arrays --composite` — the same 16 plus three nested",
}


def _nested_specs() -> list[runspec.RunSpec]:
    specs = []
    for gi, (_, nested) in enumerate(register._NESTED_SWEEPS):
        for name in register._NESTED_SWEEPS[gi]:
            beside = f" beside {nested}" if name == "control" else ""
            for mode in ("strings", "typed"):
                spec = runspec.RunSpec("pgdt", name, f"query-{mode}", "warm", f"{name} {mode}{beside}")
                specs.append(staging.in_sweep("nested-end-to-end", spec, gi))
    return specs


def run_nested_end_to_end(session: Session) -> str:
    figure = "nested-end-to-end"
    session.sweep(figure, _nested_specs(), session.cfg.reps(5))
    rows, per_rep = [], []
    for gi, (_, nested) in enumerate(register._NESTED_SWEEPS):
        for name in register._NESTED_SWEEPS[gi]:
            label = _NESTED_FILES[name]
            flag = _NESTED_FILES[nested].partition(" — ")[0]
            beside = f", in {flag}'s sweep" if name == "control" else ""
            sv, tv = (
                session.get(figure, staging.in_sweep(figure, runspec.RunSpec("pgdt", name, cmd, "warm", ""), gi))
                for cmd in ("query-strings", "query-typed")
            )
            profile = session.stager.profile(name)
            diff = formatting.median(tv) - formatting.median(sv)
            rows.append(
                [
                    label + beside,
                    f"{profile['rows']:,}",
                    f"{formatting.fmt_s(formatting.median(sv))} s",
                    f"{formatting.fmt_s(formatting.median(tv))} s",
                    f"**{diff / profile['rows'] * 1e6:.2f} µs/row**",
                    f"{formatting.median(tv) / formatting.median(sv):.2f}×",
                ]
            )
            per_rep.append(
                f"- {name}{beside} — `strings`: {formatting.fmt_readings(sv)}; `typed`: {formatting.fmt_readings(tv)}"
            )
    table = formatting.md_table(
        ["File", "Rows", "`strings`", "`typed`", "`typed` − `strings`", "Ratio"], rows
    )
    headline = _per_row_diffs(session, figure, "control", "arrays")
    baselines = []
    for _, nested in register._NESTED_SWEEPS:
        control, other = (
            session.get(figure, spec)
            for spec in _across(figure, "control", nested, "query-strings")
        )
        flag = _NESTED_FILES[nested].partition(" — ")[0]
        baselines.append(f"{flag} {formatting.median(other) / formatting.median(control):.3f}×")
    return (
        table
        + "\n\n**The three nested columns' cost**, the arrays file's `typed` − `strings` less "
        f"its own sweep's control's, paired rep by rep: **{formatting.median(headline):+.2f} µs/row** "
        f"({', '.join(f'{v:+.2f}' for v in sorted(headline))}).\n\n"
        "Each nested file's `strings` leg against its own sweep's control's: "
        + ", ".join(baselines)
        + ".\n\n"
        + prose._cached_query_note("Each `query`")
        + "\nPer-rep readings (s):\n"
        + "\n".join(per_rep)
        + "\n"
    )


# -- the cross-file floor ---------------------------------------------------


def _across(figure: str, a: str, b: str, command: str) -> tuple[runspec.RunSpec, runspec.RunSpec]:
    """`a`'s and `b`'s runs of `command` from the one sweep of `figure` a
    declared subtraction pairs them in (`subtraction_sweep`), which refuses a
    pair nothing declares."""
    gi = register.subtraction_sweep(figure, a, b)
    spec_a, spec_b = (
        staging.in_sweep(figure, runspec.RunSpec("pgdt", name, command, "warm", ""), gi) for name in (a, b)
    )
    return spec_a, spec_b


def _per_row_diffs(session: Session, figure: str, a: str, b: str) -> list[float]:
    """Per-rep µs/row differences between two files' own typed−strings costs,
    from the one sweep that declares them a pair.

    Each file's typed leg is differenced against *its own* strings leg first,
    which is what makes the subtraction legitimate: whatever the untyped
    baseline is worth on a given file cancels out of that file's own
    difference, and would not cancel out of a cross-file ratio."""
    strings, typed = (_across(figure, a, b, cmd) for cmd in ("query-strings", "query-typed"))
    out = []
    for s_spec, t_spec in zip(strings, typed):
        profile = session.stager.profile(s_spec.input)
        s, t = session.get(figure, s_spec), session.get(figure, t_spec)
        out.append([(tv - sv) / profile["rows"] * 1e6 for sv, tv in zip(s, t)])
    reps = min(len(out[0]), len(out[1]))
    return [out[1][i] - out[0][i] for i in range(reps)]


def run_cross_file_floor(session: Session) -> str:
    figure = "cross-file-floor"
    # Row 1 is read off the nested figure's first sweep's own reps -- the same
    # five runs, not a second pass over the same two files.
    share = _per_row_diffs(session, "nested-end-to-end", "control", "composite")
    # Row 2 is this figure's own: two files that differ only in their seed.
    specs = []
    for name in ("control", "control43"):
        for mode in ("strings", "typed"):
            specs.append(runspec.RunSpec("pgdt", name, f"query-{mode}", "warm", f"{name} {mode}"))
    session.sweep(figure, specs, session.cfg.reps(6))
    floor = _per_row_diffs(session, figure, "control", "control43")
    rows = [
        [
            "composite column's share — control against `--composite`",
            str(len(share)),
            f"**{formatting.median(share):+.2f} µs**",
            ", ".join(f"{v:+.2f}" for v in sorted(share)),
        ],
        [
            "**the instrument's own floor** — control against a second control (`--seed 43`, same 16 columns)",
            str(len(floor)),
            f"**{formatting.median(floor):+.2f} µs**",
            ", ".join(f"{v:+.2f}" for v in sorted(floor)),
        ],
    ]
    table = formatting.md_table(["Reading", "Reps", "Paired median", "Per-rep readings"], rows)
    return table + "\n\n" + prose._cached_query_note("Each `query`")
