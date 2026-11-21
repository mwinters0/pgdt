"""`--count-pair` and `--count-span`: one change, or a run of commits, priced in `instructions:u`."""

from __future__ import annotations

import hashlib
import json
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable, Mapping, Sequence
from typing import TYPE_CHECKING

import release

from measurement import config
from measurement import formatting
from measurement import git
from measurement import host
from measurement import inputs
from measurement import parsing
from measurement import preflight
from measurement import shapes
from measurement import staging
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import release_profile

if TYPE_CHECKING:
    from measurement.config import Config


# -- the count pair ---------------------------------------------------------

#: The input a shape reads unless it names another: the register's control
#: file, at `Config.size_gib`, off the SSD cache. A count is user space alone,
#: so the device it is read from moves nothing it reports, and nothing is
#: staged.
COUNT_PAIR_INPUT = "control"

#: The long-field rows' inputs: `long-line-resident`'s escaped text and `bytea`
#: at its shortest length, a 32 MiB line each, past a read chunk and under
#: the 64 MiB line limit the bound replaced, so every commit a pair is taken on
#: reads them unflagged.
COUNT_PAIR_LONG_INPUTS: tuple[str, ...] = tuple(
    inputs.long_line_input(kind, "32") for kind in ("escaped", "bytea")
)

#: `--count-pair`'s shapes, in its comparison's row order, each with what the
#: row is called, the container limit it runs under (`None`: the recorded
#: 512 MB) and the input it reads. The hot paths a reshape can move:
#: discovery; the data level, the only shape reaching statistics gathering,
#: under `statistics-gathering`'s limit, since the allowance it gathers within
#: is discovered from the container (D85); both `query` modes; the provider's
#: typed scan; and the typed `query` over the `arrays` input, the one row
#: whose batch decodes a nested column, the control holding none, and
#: `pgdt sql -c` over it counting `perf`'s scalar columns alone. The same
#: shapes the register times, so a count here and a figure there are one
#: workload. `pgdt sql`'s runs fall on two counts, so a rise smaller than their
#: step is unresolved there, and the typed `query` counts the same decode finer.
#:
#: **A long field's parse has four rows of its own** (`COUNT_PAIR_LONG_INPUTS`):
#: the data-level parse and the strict one (`STRICT_PARSE`) over a cluster of
#: escaped text and of `bytea` lines, each a `long-line-resident` input. Only
#: a field past its chunk reaches the path a parse reads a long field on
#: (`docs/design/decisions.md`, "D138"), and the control holds none; each runs
#: under the data level's limit, for the same reason. **The ledger, parking,
#: the floor and the bound have no row**: each adds work per chunk or per
#: plan, and D108's trigger is per row or byte; a per-row change is priced on
#: the control rows, the four long rows holding too few rows to resolve one.
COUNT_PAIR_SHAPES: tuple[tuple[str, str, str | None, str], ...] = (
    ("parse", "`pgdt parse` at the metadata level", None, COUNT_PAIR_INPUT),
    (f"{shapes.STATISTICS_FAMILY}data", "`pgdt parse` at the data level", shapes.STATISTICS_MEMORY, COUNT_PAIR_INPUT),
    ("query-strings", "`query --schema-mode strings`", None, COUNT_PAIR_INPUT),
    ("query-typed", "`query --schema-mode typed`", None, COUNT_PAIR_INPUT),
    (shapes.PARALLEL_SCAN, "`pgdt sql -c` — a typed DataFusion scan", None, COUNT_PAIR_INPUT),
    ("query-typed", "`query --schema-mode typed` over `arrays`", None, "arrays"),
    *(
        (command, f"{label} over `{name}`", shapes.STATISTICS_MEMORY, name)
        for name in COUNT_PAIR_LONG_INPUTS
        for command, label in (
            (f"{shapes.STATISTICS_FAMILY}data", "`pgdt parse` at the data level"),
            (shapes.STRICT_PARSE, "a strict `pgdt parse`"),
        )
    ),
)


def count_pair_row(command: str, name: str) -> str:
    """A row's name in a file name: its shape and its input, since one shape
    reads two inputs."""
    return f"{command}-{name}"

#: The pair's two legs, in each row's column order.
COUNT_PAIR_LEGS: tuple[str, ...] = ("parent", "tree")

#: `CLAUDE.md`'s "Long-running processes" line, past which a run is launched
#: detached rather than waited on. `--count-pair` says which side of it a run
#: fell, which decides how a hot-path slice takes its count.
DETACH_SECONDS = 10 * 60


def count_pair_parent() -> tuple[str, bool]:
    """The commit the working tree is one change on, and whether it carries
    that change uncommitted: `HEAD` where `git status` names anything,
    untracked files included, since a slice landed unattended adds its new
    files untracked; `HEAD`'s first parent where the tree is clean, the change
    then being `HEAD` itself."""
    dirty = bool(host.run(["git", "status", "--porcelain"], cwd=config.REPO, capture=True).strip())
    rev = "HEAD" if dirty else "HEAD^"
    return host.run(["git", "rev-parse", "--verify", rev], cwd=config.REPO, capture=True).strip(), dirty


def count_pair_refusal(rev: str, parent: str, dirty: bool) -> str | None:
    """Why `rev` cannot be the pair's parent leg, or `None`.

    **The parent and nothing else** (`.claude/skills/evidence/SKILL.md`, rule
    7): two adjacent trees differ by one change, so the difference is that
    change's. Any older commit prices every commit between, under the name of
    the last one, and no reading would look wrong."""
    resolved = git.resolve_commit(rev)
    if resolved is None:
        return f"{rev!r} names no commit"
    if resolved == parent:
        return None
    tree = "the uncommitted tree" if dirty else "HEAD, the tree being clean"
    return (
        f"{rev} is {resolved[:10]}, but the working tree's parent is {parent[:10]} "
        f"(the change measured is {tree}): a pair across more than one change prices every "
        "commit between under the last one's name"
    )


def count_pair_named(parent_rev: str, change_rev: str) -> tuple[str, str, str | None]:
    """`--count-pair <parent>..<change>`'s two commits, and why they cannot be
    the pair's legs, or `None`: a change committed before the instrument
    pricing it, taken from a checkout of each.

    **The parent and nothing else**, as `count_pair_refusal` holds the tree
    to: `change`'s first parent must be `parent`, so the pair is one
    commit's."""
    parent, change = git.resolve_commit(parent_rev), git.resolve_commit(change_rev)
    for rev, resolved in ((parent_rev, parent), (change_rev, change)):
        if resolved is None:
            return "", "", f"{rev!r} names no commit"
    assert parent is not None and change is not None
    first = git.resolve_commit(f"{change}^")
    if first == parent:
        return parent, change, None
    return parent, change, (
        f"{change_rev} is {change[:10]}, whose parent is "
        f"{first[:10] if first else 'none'}, not {parent_rev}'s {parent[:10]}: a pair across "
        "more than one change prices every commit between under the last one's name"
    )


def count_span_named(base_rev: str, tip_rev: str) -> tuple[str, str, int, str | None]:
    """`--count-span <base>..<tip>`'s two commits, how many commits the span
    lands, and why they cannot be its legs, or `None`.

    **An account, never a price**: a span differences every commit between,
    so it answers only what the pairs taken on its commits must sum to
    (`.claude/skills/evidence/SKILL.md`, rules 1 and 7), and says so. Refused
    unless `base` is an older commit on `tip`'s first-parent chain, so the
    commits it sums are the ones that chain landed and each could be priced
    by a pair of its own."""
    base, tip = git.resolve_commit(base_rev), git.resolve_commit(tip_rev)
    for rev, resolved in ((base_rev, base), (tip_rev, tip)):
        if resolved is None:
            return "", "", 0, f"{rev!r} names no commit"
    assert base is not None and tip is not None
    chain = host.run(["git", "rev-list", "--first-parent", tip], cwd=config.REPO, capture=True).split()
    if base == tip or base not in chain:
        return base, tip, 0, (
            f"{base_rev}'s {base[:10]} is no older commit on {tip_rev}'s first-parent chain, "
            "so the span is no run of landed commits a pair could each price"
        )
    return base, tip, chain.index(base), None


def image_inputs_at(rev: str) -> dict[str, bytes]:
    """`release.image_inputs` as `rev` has them, read out of git."""
    paths = {"Dockerfile": "release/Dockerfile", **{n: n for n in release.CONTEXT_FILES}}
    return {
        name: subprocess.run(
            ["git", "show", f"{rev}:{path}"], cwd=config.REPO, capture_output=True, check=True
        ).stdout
        for name, path in paths.items()
    }


def count_pair_image_refusal(rev: str) -> str | None:
    """Why the two legs cannot be built in one image, or `None`.

    Both legs are built in the image `release.ensure_image` makes of the
    tree's toolchain files, and the parent's own `release.py --inside`
    refuses an image made from other copies than its checkout's. Asked here
    first, so a change that moves the toolchain or the image is named as such
    before a build is spent: a compiler's difference is not a slice's, and is
    priced by `release-profile`'s re-take on the new toolchain."""
    here, there = release.image_inputs(), image_inputs_at(rev)
    moved = sorted(name for name in here if here[name] != there.get(name))
    if not moved:
        return None
    return (
        f"{', '.join(moved)} differ(s) between {rev} and the tree, so the two legs would be "
        "built by different toolchains or images and the count would price that too"
    )


def count_pair_leg_binary(cfg: Config, leg: str) -> Path:
    """Where `--count-pair` leaves a leg's binary, overwritten by the next pair
    and read by `callgrind_recipe` as the binary counted."""
    return cfg.out_dir / f"pgdt-count-pair-{leg}"


def count_pair_binary(
    cfg: Config, leg: str, tree: Path, log: Callable[[str], None], target: str | None = None
) -> Path:
    """One leg of the pair: the shipped build — `release`, at the default
    features — of `tree`, in the release image, in a cargo target directory
    of its own (`image_target_dir`, `count-pair-<leg>` unless `target` names
    another), so neither leg nor the register's own build ever finds
    another's output fresh. Copied into `Config.out_dir` and asked which
    profile built it, for `binary_profile`'s reason."""
    target_dir = image.image_target_dir(cfg, target or f"count-pair-{leg}")
    out = count_pair_leg_binary(cfg, leg)
    if cfg.dry_run:
        log(f"  [dry-run] would build the {leg} leg from {tree} into {target_dir}")
        return out
    log(f"  building the {leg} leg from {tree} into {target_dir}")
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    built = image.build_in_image(cfg, release.Build(), target_dir=target_dir, tree=tree)
    shutil.copyfile(built, out)
    out.chmod(0o755)
    got = leg_binaries.binary_profile(out)
    if got != "release":
        out.unlink()
        raise RuntimeError(
            f"the {leg} leg reports profile `{got}`: the pair is two shipped builds, "
            "or it counts something else"
        )
    return out

#: The target directory a named change's leg builds in: never the tree leg's,
#: which only the checkout builds in.
COUNT_PAIR_CHANGE_TARGET = "count-pair-change"


def count_pair_checkout_binary(
    cfg: Config, leg: str, commit: str, log: Callable[[str], None], target: str | None = None
) -> Path:
    """One leg built from a detached checkout of `commit` under
    `Config.out_dir`, added just before its build and removed after it, in
    `target` (`count_pair_binary`). Its files are written then, so cargo,
    which reads a source file's mtime against its last build in the kept
    target directory, every checkout being mounted at one path, never finds
    a build of another tree fresh. That holds only in a directory no older
    files build in, so a named change's leg is never built where the
    checkout's tree leg is (`COUNT_PAIR_CHANGE_TARGET`)."""
    work = cfg.out_dir / f"count-pair-worktree-{commit[:10]}"
    if cfg.dry_run:
        log(f"  [dry-run] would check {commit[:10]} out at {work}")
        return count_pair_binary(cfg, leg, work, log, target)
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    if work.exists():
        host.run(["git", "worktree", "remove", "--force", str(work)], cwd=config.REPO, quiet=True)
    host.run(["git", "worktree", "add", "--detach", str(work), commit], cwd=config.REPO, quiet=True)
    try:
        return count_pair_binary(cfg, leg, work, log, target)
    finally:
        host.run(["git", "worktree", "remove", "--force", str(work)], cwd=config.REPO, quiet=True)


def count_pair_argv(
    cfg: Config, binary: Path, counter: Path, dump: Path, command: str, memory: str | None
) -> list[str]:
    """One counted run of `command`: `time_run`'s container, `_script`'s
    shape under the counter (`counted_script`), and nothing staged — a count
    is not a wall clock, so no binary is read ahead of it and no cache is
    dropped."""
    limit = memory or cfg.memory
    return [
        *cfg.container_argv(), "run", "--rm",
        "-m", limit, "--memory-swap", limit,
        *(a for cap in parsing.COUNT_INSTRUCTIONS_CAPS for a in ("--cap-add", cap)),
        "-v", f"{binary}:/pgdt:ro",
        "-v", f"{counter}:{parsing.COUNT_INSTRUCTIONS}:ro",
        "-v", f"{dump}:/dump.sql:ro",
        cfg.image, "bash", "-c", parsing.counted_script(shapes._script(command)),
    ]  # fmt: skip


def count_pair_resolution(parent: Sequence[int], tree: Sequence[int]) -> str:
    """Whether a row's two legs' counts meet: **resolved** where one leg's
    fewest lies above the other's most (`resolved_above`), said with the
    direction, and **unresolved** otherwise. A fact, not a verdict: a
    resolved rise is what refuses a hot-path slice, and on a pair of
    identical sources any resolved row, either way, falsifies the instrument
    (`docs/design/decisions.md`, "D108")."""
    if formatting.resolved_above(tree, parent):
        return "resolved, a rise"
    if formatting.resolved_above(parent, tree):
        return "resolved, a fall"
    return "unresolved"


def count_pair_table(counts: Mapping[str, Mapping[str, Sequence[int]]]) -> str:
    """The comparison: one row per shape, keyed by its label, each leg's
    fewest and most `instructions:u`, the tree's median against the
    parent's, and whether the two legs' counts meet
    (`count_pair_resolution`)."""
    rows = []
    for _, label, _, _ in COUNT_PAIR_SHAPES:
        parent, tree = (counts[label][leg] for leg in COUNT_PAIR_LEGS)
        delta = formatting.median(tree) - formatting.median(parent)
        rows.append(
            [
                label,
                *(f"{lo:,}–{hi:,}" for lo, hi in (formatting.spread(parent), formatting.spread(tree))),
                f"{delta:+,.0f}",
                f"{delta / formatting.median(parent) * 100:+.3f}%",
                count_pair_resolution(parent, tree),
            ]
        )
    return formatting.md_table(
        ["`instructions:u`", "parent, fewest–most", "tree, fewest–most", "median change", "%", "the legs"],
        rows,
    )


def cmd_count_pair(rev: str, cfg: Config, span: bool = False) -> int:
    """`--count-pair <rev>`: the shipped build at `rev` and at the working
    tree, each shape of `COUNT_PAIR_SHAPES` counted `RELEASE_PROFILE_REPS`
    times on each, and the comparison printed. **No figure**: nothing is
    written to `measurements.md` or to a sitting directory, and the binaries
    are overwritten by the next pair.

    **Reps on both legs**: a count holds still where a wall clock moves with
    the machine, though not to the instruction between runs of one build, so
    a change is read against the spread this pair takes at its own two trees,
    never against zero or another tree's. Each rep runs both legs, the leg
    going first reversed for the second half of the reps, as `_sweep_one`
    orders a figure's. A shape any of whose runs answers differently from
    another is refused, since its counts would be two workloads'. **Refused
    unless `rev` is the tree's parent** (`count_pair_refusal`), or where the
    two would build under different toolchains (`count_pair_image_refusal`).
    The parent is built from a worktree under `Config.out_dir`, removed after;
    the tree from the checkout itself, uncommitted files included.

    **`<parent>..<change>` prices a committed change instead**, the shapes
    being the tree's: both legs from checkouts, refused unless `parent` is
    `change`'s parent (`count_pair_named`) or either would build under
    another toolchain than the tree's. Ends by saying how many rows resolved
    and whether the run fit under `DETACH_SECONDS`.

    **`span` takes `<base>..<tip>` as an account instead** (`--count-span`):
    both legs from checkouts as a named change's are, refused unless `base`
    is on `tip`'s first-parent chain (`count_span_named`), and each row's
    change said to be every commit's between, summed — what the pairs on
    those commits must add up to, never one change's price (D155)."""
    started = time.monotonic()
    dry_run = cfg.dry_run
    named, _, change_rev = rev.partition("..")
    commits = 1
    if span:
        if not change_rev:
            print(f"--count-span refused: {rev!r} is no <base>..<tip>.", file=sys.stderr)
            return 2
        parent, change, commits, refusal = count_span_named(named, change_rev)
        refusals = [refusal] if refusal else [
            count_pair_image_refusal(parent), count_pair_image_refusal(change)
        ]
        described = f"{change[:10]}, an account over {commits} commits"
    elif change_rev:
        parent, change, refusal = count_pair_named(named, change_rev)
        refusals = [refusal] if refusal else [
            count_pair_image_refusal(parent), count_pair_image_refusal(change)
        ]
        described = change[:10]
    else:
        parent, dirty = count_pair_parent()
        change = None
        refusals = [count_pair_refusal(rev, parent, dirty), count_pair_image_refusal(parent)]
        described = "the uncommitted tree" if dirty else f"HEAD ({parent[:10]}..HEAD)"
    for refusal in refusals:
        if refusal:
            flag = "--count-span" if span else "--count-pair"
            print(f"{flag} refused: {refusal}.", file=sys.stderr)
            return 2

    def log(msg: str) -> None:
        print(msg, flush=True)

    log(f"count pair: {parent[:10]} against {described}")
    problems = preflight.counter_preflight(cfg, log)
    if problems:
        for problem in problems:
            log(f"!! {problem}")
        return 2
    counter = image.ensure_count_instructions_binary(cfg, log)
    stager = staging.Stager(cfg, log)
    dumps = {name: stager.cold_path(name) for *_, name in COUNT_PAIR_SHAPES}

    binaries = {"parent": count_pair_checkout_binary(cfg, "parent", parent, log)}
    binaries["tree"] = (
        count_pair_checkout_binary(cfg, "tree", change, log, COUNT_PAIR_CHANGE_TARGET)
        if change
        else count_pair_binary(cfg, "tree", config.REPO, log)
    )

    reps = cfg.reps(release_profile.RELEASE_PROFILE_REPS)
    counts: dict[str, dict[str, list[int]]] = {}
    for command, label, memory, name in COUNT_PAIR_SHAPES:
        answers: dict[str, dict[str, str]] = {}
        for rep in range(reps):
            legs = COUNT_PAIR_LEGS if rep < (reps + 1) // 2 else COUNT_PAIR_LEGS[::-1]
            for leg in legs:
                argv = count_pair_argv(cfg, binaries[leg], counter, dumps[name], command, memory)
                got = counts.setdefault(label, {}).setdefault(leg, [])
                if dry_run:
                    if rep == 0:
                        log("  [dry-run] " + shlex.join(argv))
                    digest = hashlib.sha256(f"{label}/{leg}/{rep}".encode()).digest()
                    got.append(3_000_000_000 + digest[0] * 1_000_000)
                    continue
                proc = subprocess.run(argv, text=True, capture_output=True)
                if proc.returncode != 0:
                    raise RuntimeError(
                        f"{label} on the {leg} leg, rep {rep + 1}, exited {proc.returncode}\n"
                        f"{proc.stderr[-2000:]}"
                    )
                got.append(parsing.parse_instructions(proc.stderr))
                answers[f"{leg} {rep + 1}"] = parsing.parse_reported(proc.stdout)
                log(f"  {label}, {leg}, rep {rep + 1}: {got[-1]:,} instructions:u")
        if len({json.dumps(a, sort_keys=True) for a in answers.values()}) > 1:
            log(f"!! {label}: the runs answered differently ({answers}), so no count is compared")
            return 1

    took = time.monotonic() - started
    log("\n" + count_pair_table(counts))
    resolved = [
        label
        for _, label, _, _ in COUNT_PAIR_SHAPES
        if count_pair_resolution(*(counts[label][leg] for leg in COUNT_PAIR_LEGS)) != "unresolved"
    ]
    log(
        f"\n{len(resolved)} of {len(COUNT_PAIR_SHAPES)} rows resolved"
        + (f": {', '.join(resolved)}." if resolved else ".")
    )
    if span:
        log(
            f"An account over {commits} commits, not one change's price: each row's change is "
            "every one of theirs, summed, and is read only against the pairs taken on them."
        )
    fits = took < DETACH_SECONDS
    log(
        f"The pair took {took / 60:.1f} min, "
        + (
            f"under the {DETACH_SECONDS // 60}-minute line: a hot-path slice runs it in the "
            "foreground."
            if fits
            else f"over the {DETACH_SECONDS // 60}-minute line: a hot-path slice launches it "
            "detached, logging under runs/, and the slice after it opens by reading the result."
        )
        + (" (A dry run: the time is the harness's, not a build's.)" if dry_run else "")
    )
    return 0
