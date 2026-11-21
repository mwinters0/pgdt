"""The measurement harness's command line: it runs the sweep behind
docs/design/measurements.md and emits that doc's tables. The harness itself is
the `measurement` package beside this file, whose doc says what it enforces.

Usage:

    cd scripts
    uv run measure.py --list
    uv run measure.py --figure map-only
    uv run measure.py --stage warm
    uv run measure.py --all
    uv run measure.py --stale --since <rev>
    uv run measure.py --count-pair HEAD~1
    uv run measure.py --count-pair <parent>..<change>
    uv run measure.py --count-span <base>..<tip>
    uv run measure.py --profile-recipe
    uv run measure.py --heaptrack-recipe
    uv run measure.py --callgrind-recipe
    uv run python -m unittest discover -s measurement -t . -v

Four invocations are printed rather than run, for two reasons: koji's because
it is an hour on another medium, and the three instrument recipes' because
none of a sampling profile, a heap recording or a callgrind count is a figure
at all. All four live here because a command kept in prose is a command that
stops running.
"""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import Sequence

from measurement import borrowing
from measurement import check
from measurement import config
from measurement import count_pair
from measurement import drift
from measurement import emit
from measurement import machine_state
from measurement import shapes
from measurement import verify_additive
from measurement.builds import image
from measurement.figures import register
from measurement.recipes import callgrind
from measurement.recipes import heaptrack
from measurement.recipes import koji
from measurement.recipes import profile
from measurement.staleness import outside_sections
from measurement.staleness import stale


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--list", action="store_true", help="name every figure and what invalidates it")
    parser.add_argument("--figure", action="append", default=[], help="figure id (repeatable, or comma-separated)")
    parser.add_argument(
        "--stage",
        choices=["cold", "cold-nvme", "warm", "warm-parallel", "criterion"],
        help="every figure of one stage",
    )
    parser.add_argument(
        "--alone",
        action="store_true",
        help="take exactly the figures named, borrowing nothing — a diagnostic sitting, "
        "whose tables carry the partial-sweep note and the NOT PUBLISHABLE banner",
    )
    parser.add_argument("--all", action="store_true", help="the whole sweep — what the doc's session stamp means")
    parser.add_argument("--stale", action="store_true", help="say which figures a diff has invalidated")
    parser.add_argument(
        "--render",
        metavar="RUN_DIR",
        help="rebuild a past sitting's tables.md from its raw.json, measuring nothing — "
        "how a presentation-only renderer change is folded in, instead of by hand",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="reconcile the register against measurements.md's figure markers and its "
        "outside-register declarations, and name the documents a moved figure invalidates",
    )
    parser.add_argument(
        "--verify-additive",
        action="store_true",
        help="regenerate every figure's inputs at --since and now, and compare bytes — the "
        "evidence an acknowledged commit carries",
    )
    parser.add_argument(
        "--since", help="revision for --stale/--verify-additive (default: the doc's session stamp)"
    )
    parser.add_argument(
        "--drift",
        nargs=2,
        metavar=("SWEEP", "SWEEP"),
        help="two runs/measure-* directories: emit the session-drift table across them, "
        "and each arm's drift where both took more than one",
    )
    parser.add_argument(
        "--arms",
        metavar="RUN_DIR",
        help="one runs/measure-* directory taken under more than one arm: set its arms' "
        "readings beside each other",
    )
    parser.add_argument(
        "--koji-recipe",
        action="store_true",
        help="print koji's detached invocation — the harness owns it but never runs it",
    )
    parser.add_argument(
        "--wrap",
        action="store_true",
        help="with --koji-recipe: the stop-report-resume-compare sequence instead",
    )
    parser.add_argument(
        "--koji-jobs",
        type=int,
        default=shapes.SWEEP_JOBS,
        metavar="N",
        help=f"with --koji-recipe: the worker count the scan states (default {shapes.SWEEP_JOBS}); "
        "koji's parallel leg is a leg at some count against a serial one, so the recipe "
        "takes it rather than pinning it",
    )
    parser.add_argument(
        "--profile-recipe",
        action="store_true",
        help="print the sampling-profile sequence — the harness owns it but never runs it",
    )
    parser.add_argument(
        "--heaptrack-recipe",
        action="store_true",
        help="print the libc-level heap-attribution sequence — the harness owns it but "
        "never runs it",
    )
    parser.add_argument(
        "--callgrind-recipe",
        action="store_true",
        help="print the sequence placing a rise --count-pair resolved, callgrind's Ir on "
        "the pair's own two binaries — the harness owns it but never runs it",
    )
    parser.add_argument(
        "--count-pair",
        metavar="REV",
        help="the shipped build at REV, which must be the working tree's parent, and at the "
        "tree, each hot-path shape's instructions:u counted release-profile's reps on each, "
        "each row resolved or not — a slice's price, never a figure; PARENT..CHANGE prices "
        "a committed CHANGE, whose parent PARENT must be, both built from checkouts",
    )
    parser.add_argument(
        "--count-span",
        metavar="BASE..TIP",
        help="--count-pair's rows over a run of landed commits, BASE on TIP's first-parent "
        "chain, both built from checkouts — an account the pairs on those commits must sum "
        "to, never one change's price",
    )
    parser.add_argument("--reps", type=int, help="override every figure's rep count (smoke runs only)")
    parser.add_argument("--dry-run", action="store_true", help="print what would run, measure nothing")
    parser.add_argument("--keep-warm", action="store_true", help="leave staged inputs on tmpfs")
    parser.add_argument(
        "--pin-governor",
        action="store_true",
        help=f"pin every CPU to the {machine_state.SWEEP_GOVERNOR} governor for the sweep and restore it "
        "afterwards; an apparatus change, and a measured no-op on amd-pstate-epp",
    )
    parser.add_argument(
        "--pin-cpus",
        choices=machine_state.ARM_CHOICES,
        help="place each leg on one L3 group, or two, by the workers it states, and the "
        "harness on the other die; `alternate` takes every leg unpinned and pinned in turn. "
        "Refuted, so anything but `off` is unpublishable",
    )
    parser.add_argument(
        "--stage-binaries",
        choices=machine_state.ARM_CHOICES,
        help="run the timed binaries from tmpfs, read once untimed before each run; "
        "`alternate` takes every leg staged and unstaged in turn. The recorded apparatus, "
        "so anything but `on` is unpublishable",
    )
    args = parser.parse_args(argv)

    if args.list:
        emit.cmd_list()
        return 0
    if args.drift:
        return drift.cmd_drift(*args.drift)
    if args.arms:
        return drift.cmd_arms(args.arms)
    if args.koji_recipe:
        return koji.cmd_koji(args.wrap, args.koji_jobs)
    if args.profile_recipe:
        return profile.cmd_profile()
    if args.heaptrack_recipe:
        return heaptrack.cmd_heaptrack()
    if args.callgrind_recipe:
        return callgrind.cmd_callgrind()
    if args.check:
        return check.cmd_check(config.REPO / "docs/design/measurements.md")
    if args.stale:
        return stale.cmd_stale(args.since)
    if args.render:
        # A default `Config`: a render reads no input and runs no container, so
        # nothing the sitting's own paths or sizes would have decided applies.
        return emit.render(config.Config(), Path(args.render))
    if args.verify_additive:
        return verify_additive.cmd_verify_additive(args.since)
    if args.count_pair:
        return count_pair.cmd_count_pair(args.count_pair, config.Config(dry_run=args.dry_run))
    if args.count_span:
        return count_pair.cmd_count_pair(args.count_span, config.Config(dry_run=args.dry_run), span=True)

    ids: list[str] = []
    for item in args.figure:
        ids += [p for p in item.split(",") if p]
    if args.stage:
        # Split on `+` rather than matching as a substring: a figure taken in
        # two regimes declares `cold+warm`, and both must select it, while
        # `cold` must *not* reach `cold-nvme` -- a different device, whose
        # absolutes belong to no cold-SSD sitting.
        ids += [f.id for f in register.FIGURES if args.stage in f.stage.split("+")]
    if args.all:
        ids = [f.id for f in register.FIGURES]
    if not ids:
        parser.error(
            "nothing selected: pass --figure, --stage, --all, --list, --stale or --verify-additive"
        )

    cfg = config.Config(
        reps_override=args.reps,
        dry_run=args.dry_run,
        keep_warm=args.keep_warm,
        alone=args.alone,
        pin_governor=args.pin_governor or config.Config().pin_governor,
        pin_cpus=args.pin_cpus or config.Config().pin_cpus,
        stage_binaries=args.stage_binaries or config.Config().stage_binaries,
    )
    try:
        arms = cfg.arms
    except ValueError as exc:
        parser.error(str(exc))
    if any(a.pinned for a in arms):
        problem = machine_state.pin_topology_problem(machine_state.l3_groups())
        if problem:
            parser.error(problem)
    figures = outside_sections.resolve_selection(ids, alone=args.alone)
    # A sitting short of the sweep publishes outside the session stamp, which
    # only a figure standing in no borrow edge may do. Asked here, before the
    # measurement is spent, rather than at `--check` after it -- and asked only
    # of a sitting that could be folded in at all, which is what
    # `cfg.publishable` answers: a smoke run and a `--alone` sitting both emit
    # their tables under the NOT PUBLISHABLE banner, so there is no publication
    # for this to refuse.
    if cfg.publishable and not {f.id for f in register.FIGURES} <= {f.id for f in figures}:
        refusals = borrowing.publication_refusals(figures)
        if refusals:
            parser.error(
                "this sitting would publish outside the document's session stamp: "
                + "; ".join(refusals)
                + ". Take the whole doc (`--all`), which re-stamps it, or `--alone` for a "
                "diagnostic sitting, whose tables come back marked NOT PUBLISHABLE."
            )
    # The shipped binary, built in the first second, before the run directory
    # exists, and about provenance rather than existence. `print` rather than
    # the sitting's log, which `emit` has not opened yet — a build that fails
    # here must not leave an empty run directory behind it.
    image.ensure_pgdt_binary(cfg, lambda msg: print(msg, flush=True))
    if not cfg.dry_run and not cfg.bin_pgdt.exists():
        parser.error(
            f"{cfg.bin_pgdt} is missing, and PGDT_MEASURE_BIN names a binary this harness does "
            f"not build. Point it at one that exists, or unset it and let the harness build "
            f"{config.REGISTER_BIN}."
        )
    return emit.emit(cfg, figures)

if __name__ == "__main__":
    raise SystemExit(main())
