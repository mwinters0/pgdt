"""The heaptrack recipe: a heap recording is no figure, so printed."""

from __future__ import annotations

import shlex
from pathlib import Path
from typing import Sequence
from typing import TYPE_CHECKING

import contained

import release

from measurement import config
from measurement import inputs
from measurement import memory_model
from measurement import shapes
from measurement.builds import image
from measurement.figures import gzip_resident
from measurement.figures import register
from measurement.figures import reserve
from measurement.recipes import profile

if TYPE_CHECKING:
    from measurement.runspec import RunSpec


#: The recorder: the release image's, run in a container (`contained.py`).
#: `heaptrack_print` is derived from it rather than named beside it: the two
#: ship in one package, so a printer never reads another version's format.
HEAPTRACK = "heaptrack"

#: What a recording's container adds to the one the sweep times its shape
#: under: heaptrack's term, **one stated GiB whatever the shape**. heaptrack's
#: bookkeeping grows with live allocations and distinct backtraces, not with
#: what the run holds, and `heaptrack_interpret` runs beside the recorder in the
#: same cgroup, so no multiple of a held bound prices it; each recording's
#: `memory.peak` (`HEAPTRACK_PEAK_SUFFIX`) is what re-sizes it.
HEAPTRACK_TERM = config.GIB

#: The file each recording's container writes its cgroup's `memory.peak` to,
#: beside the recording: `-o`'s stem plus this. Read from inside the
#: recording's own `sh -c`, so it is that container's peak, recorder and
#: interpreter included, and one build's — never a timed cell less a recording,
#: which would price two builds.
HEAPTRACK_PEAK_SUFFIX = ".memory.peak"

#: Where a container states its own peak: the cgroup v2 file a container sees
#: as its own cgroup's.
CGROUP_MEMORY_PEAK = "/sys/fs/cgroup/memory.peak"

#: The suffix the recorder writes its file under, `-o`'s stem plus this:
#: **the image's heaptrack compresses with gzip**, being built without zstd,
#: so a recipe naming `.zst` reads a file that was never written and prints
#: an empty report (observed: `runs/p15.18/heaptrack/`).
HEAPTRACK_SUFFIX = ".gz"

#: The demangler the report is piped through.
#:
#: **Rust symbols reach `heaptrack_print` mangled, and they are v0.** This
#: workspace's binaries carry 7,351 `_R`-prefixed symbols and no `_ZN` ones, and
#: heaptrack's own Rust demangling is *post*-1.5.0 and absent from the build
#: installed here — no binary of it references `rustc_demangle`, and
#: `heaptrack_interpret` resolves only `__cxa_demangle`. `c++filt` demangles v0
#: natively, so one pipe buys back every Rust frame in the report; without it
#: the C frames read fine and the Rust ones above them are noise, which is a
#: report that looks half-broken rather than one that looks wrong.
HEAPTRACK_DEMANGLE = "c++filt"

#: The allocator the recorded build links, and its target directory's name
#: under `Config.alloc_build_root`.
#:
#: **`system`, because heaptrack counts requests at `malloc`**, never what an
#: allocator kept, and the shipped build's mimalloc is linked without
#: `override`: a recording of the default build carries `liblzma`'s and
#: `aws-lc`'s allocations and none of Rust's, which on `HEAPTRACK_AXIS`'s pair
#: leaves the block slot and the chunk buffer unnamed. The two builds run the
#: same code, so a `system` recording holds every request a default one does
#: plus Rust's. **Its own target directory**, because a `--features` build
#: under `--profile profiling` in another overwrites that one's binary.
HEAPTRACK_LEG = "system"

#: The recorded build: `HEAPTRACK_LEG`'s allocator, `profiling`'s line tables.
HEAPTRACK_BUILD = release.Build(
    profile="profiling", features=(HEAPTRACK_LEG,), default_features=False
)

#: The first pair recorded, the step: a **pair** rather than a survey.
#:
#: `reserve`'s path step: a stated budget either side of the one-reader charge
#: `charge_bytes(unit, 1)` — `reader_bytes` plus the pool's retention list — one
#: byte apart, so the two runs differ by whether `BlockCache::affordable` admits
#: a block-decoding reader and by nothing else (`RESERVE_STEP_BUDGETS`). Read
#: as a difference — `heaptrack_print --diff` — that pair names the whole block
#: path's allocation at the `malloc` boundary, decoder included, C and Rust
#: alike on the `HEAPTRACK_LEG` build, which is the measure-change-measure loop
#: no single recording gives.
#:
#: **Why this pair and not the flagless family.** Every other `reserve` leg
#: differs from its neighbour by a container limit, which a recording, its
#: container carrying `HEAPTRACK_TERM`, does not reproduce; the step is the one axis that lives
#: entirely in the argv, so it is the one a recording can reproduce exactly. The input is `RESERVE_MECHANISM_INPUT` for the same reason that
#: figure's mechanism legs use it — the budgets are computed from its block
#: size, so a second input would price a step nobody measured.
HEAPTRACK_AXIS: tuple[tuple[str, str], ...] = tuple(
    (f"{shapes.RESERVE_STEP_FAMILY}{budget}", shapes.RESERVE_MECHANISM_INPUT)
    for budget in shapes.RESERVE_STEP_BUDGETS
)

#: The second pair: a single member's first pass at the limit leg, at the top
#: count and at `POOL_DEPTH`'s, `gzip-resident`'s two upper rows.
#:
#: **Read as a difference, it names what twenty added workers asked for**, by
#: call stack: the secant that figure reads against `Chunking::task_bytes`,
#: attributed term by term. The limit leg, because it is the one whose chunks
#: fill a task's plaintext terms, so a remainder past them is a worker's own.
#: The twenty-four-job shape is first, so the diff is it less the four-job one.
HEAPTRACK_FIRST_PASS_AXIS: tuple[tuple[str, str], ...] = tuple(
    (f"{shapes.GZRSS_FAMILY}{jobs}", gzip_resident.GZIP_LIMIT_LEG) for jobs in (shapes.GZIP_RSS_JOBS[-1], shapes.GZIP_RSS_JOBS[1])
)


def heaptrack_memory(
    figure: str, specs: Sequence[RunSpec], axis: tuple[tuple[str, str], ...]
) -> str:
    """A recorded pair's container limit: the one the sweep times its shapes
    under in `figure` — each spec's own `RunSpec.memory`, else the figure's,
    else the recorded default, as `Session` resolves it — plus
    `HEAPTRACK_TERM`.

    **The figure's container, because a recorded shape is a timed one**
    (`heaptrack_argv`), so the container it already works in on any machine is
    the one to record it in, and heaptrack is the only thing a recording adds.
    It is a guard on the machine and moves no budget: every recorded shape
    states its allowance."""
    timed = {
        (spec.command, spec.input): spec.memory or register.FIGURES_BY_ID[figure].memory or config.Config.memory
        for spec in specs
    }
    missing = [shape for shape in axis if shape not in timed]
    if missing:
        raise ValueError(f"{figure} times no {missing}")
    limits = {timed[shape] for shape in axis}
    if len(limits) != 1:
        raise ValueError(f"{figure} times one pair under {sorted(limits)}")
    return f"{(contained.size_bytes(limits.pop()) + HEAPTRACK_TERM) // config.MIB}m"

#: Each recorded pair: its axis, its container limit, and the stem its diff
#: is written under.
HEAPTRACK_PAIRS: tuple[tuple[tuple[tuple[str, str], ...], str, str], ...] = (
    (
        HEAPTRACK_AXIS,
        heaptrack_memory("reserve", reserve._reserve_step_specs(), HEAPTRACK_AXIS),
        "heaptrack-step-diff",
    ),
    (
        HEAPTRACK_FIRST_PASS_AXIS,
        heaptrack_memory("gzip-resident", gzip_resident._gzip_rss_specs(), HEAPTRACK_FIRST_PASS_AXIS),
        "heaptrack-first-pass-diff",
    ),
)

#: Every recorded shape, pair by pair.
HEAPTRACK_SHAPES: tuple[tuple[str, str], ...] = tuple(
    shape for axis, _, _ in HEAPTRACK_PAIRS for shape in axis
)


def heaptrack_argv(command: str, source: Path | str, cache: Path | str) -> list[str]:
    """The `pgdt` arguments one recorded shape runs.

    `profile_argv`'s sibling, and the same reconciliation applies: these are the
    flags `_script` hands the sweep, because an attribution is only readable
    against the figure it explains. `test_heaptrack.py`'s `HeaptrackRecipe`
    compares the two shape by shape, so a flag that moves in the timed shape and
    not here fails there rather than in a recording that quietly measured
    something else."""
    if command.startswith(shapes.RESERVE_STEP_FAMILY):
        budget = command.removeprefix(shapes.RESERVE_STEP_FAMILY)
        if not budget.isdigit() or int(budget) not in shapes.RESERVE_STEP_BUDGETS:
            raise ValueError(f"{command!r} names a budget the figure does not carry")
        return [
            "parse",
            "--source", str(source),
            "--cache", str(cache),
            "--jobs", str(shapes.RESERVE_JOBS),
            "--memory", str(memory_model.stated_allowance(int(budget))),
            *shapes.NO_STATISTICS.split(),
        ]
    if command.startswith(shapes.GZRSS_FAMILY):
        jobs = command.removeprefix(shapes.GZRSS_FAMILY)
        if not jobs.isdigit() or int(jobs) not in shapes.GZIP_RSS_JOBS:
            raise ValueError(f"{command!r} names a job count the figure does not carry")
        return [
            "parse",
            "--source", str(source),
            "--cache", str(cache),
            "--jobs", jobs,
            "--memory", str(memory_model.stated_allowance(shapes.GZPASS_BUDGET)),
            *shapes.NO_STATISTICS.split(),
        ]
    raise ValueError(f"unknown heaptrack shape {command!r}")


def heaptrack_recipe(cfg: config.Config) -> str:
    """The whole sequence, with every path filled in.

    Eight things here decide whether the recording describes what it claims to,
    and each fails by returning a plausible report of something else.
    `test_heaptrack.py` asserts all eight:

    * **a `HEAPTRACK_LEG` build, in a target directory of its own.** The
      default build's Rust heap never reaches `malloc`, so its recording names
      C's allocations and reads as though Rust made none; and another
      directory's `profiling/pgdt` is the profile recipe's binary.
    * **the `profiling` binary, never a `release` one.** heaptrack
      resolves symbols from either, but `release` carries no line tables, so a
      report off it has no `.rs:` reference anywhere in it — 3,627 of them
      against 0, measured on the same recording — and every Rust frame is a bare
      name with no file behind it. The published figures stay on `release`,
      which is why this is the second binary the profile recipe already builds.
    * **no `-C force-frame-pointers=yes`, and that is a stated non-requirement
      rather than an omission.** heaptrack unwinds `.eh_frame`, where `perf`
      needs frame pointers, so the flag buys nothing here — verified by
      recording against a binary whose `main` opens `push %rax`, which resolved
      the decoder stack whole. Writing it anyway would fingerprint a second
      build of the same source for no reading.
    * **`--record-only`.** heaptrack otherwise hands the finished file to
      `heaptrack_gui`, which the image does not carry. That failure is
      cosmetic, the recording being already written, but a recipe that ends in an
      error message is a recipe a session stops trusting.
    * **`--merge-backtraces=0` on the report.** `heaptrack_print` merges by
      default and its own `--help` says the merged peak consumption is not
      correct: a merged frame states the *summed* peak of every backtrace under
      it beside a call count that belongs to the merge, so one decoder's 8.39 MB
      can be printed as "8.39M over 2 calls" and eight of them as 67.11 MB. That
      is the shape of the one unexplained reading this instrument has produced.
    * **`c++filt`.** See `HEAPTRACK_DEMANGLE`: the Rust frames arrive v0-mangled
      and this build of heaptrack cannot demangle them.
    * **the recorder's own suffix.** See `HEAPTRACK_SUFFIX`: a report of a
      file the recorder never wrote is empty.
    * **each report's pipe inside one container**, its file written there. A
      container's output reaches the host through the runtime's logger, which
      drops what is unwritten when the container exits, so a printer piped
      into a second container's demangler loses its report's tail.

    **Every line running a binary runs it in a container** (`contained.py`),
    as every run of a built binary is: a recording and its reports under their
    pair's limit, the figure's own container plus heaptrack's term
    (`heaptrack_memory`), and each recording writes that container's
    `memory.peak` beside it (`HEAPTRACK_PEAK_SUFFIX`), so one that came near
    its limit says so in its own artifact. A `--version` states no limit,
    taking `contained.py`'s."""
    warm = cfg.warm_dir
    target = image.image_target_dir(cfg, f"heaptrack-{HEAPTRACK_LEG}")
    binary = HEAPTRACK_BUILD.binary(target)
    c = profile.contained_prefix(cfg, contained.DEFAULT_MEMORY)
    cache = warm / "heaptrack.pgdt"
    out = cfg.out_dir
    printer = f"{HEAPTRACK}_print"

    lines: list[str] = []
    step = 0

    def head(*text: str) -> None:
        nonlocal step
        lines.extend([f"# {step}. {text[0]}", *(f"#    {t}" for t in text[1:])])
        step += 1

    head(
        "The tool: the release image's, run in a container as every line",
        "below that runs a binary is (scripts/contained.py), under a limit",
        "that guards the machine. It hooks malloc",
        "through LD_PRELOAD, so it sees liblzma's dictionary -- which is the",
        "whole reason for it: the counting global allocator sees only Rust's",
        "allocations. On the build below, whose Rust heap is the system one, it",
        "sees C and Rust alike; the shipped heap is mimalloc's, which never",
        "reaches malloc (KD109).",
    )
    lines += [f"{c} {HEAPTRACK} --version", ""]

    head(
        "The build, in the release image: the system allocator, so Rust's",
        "requests reach malloc, in a target directory of its own, so the",
        "profile recipe's binary is not overwritten; --version must say",
        f"(allocator: {HEAPTRACK_LEG}). No --rustflags: heaptrack unwinds .eh_frame,",
        "so frame pointers buy nothing and would fingerprint a second build.",
        "The `profiling` profile is what buys source lines; release has none.",
    )
    lines += [profile.recipe_build_line(HEAPTRACK_BUILD, target), f"{c} {binary} --version", ""]

    staged = sorted({name for _, name in HEAPTRACK_SHAPES})
    head(
        "Stage the inputs warm, on the host. Not for the reading's sake --",
        "heaptrack counts allocations, which a cold read does not change --",
        "but because its own overhead makes the recording the slow part.",
    )
    lines.append(f"mkdir -p {warm} {out}")
    for name in staged:
        lines.append(f"cp -n {cfg.cache_dir / inputs.input_file(name)} {warm / inputs.input_file(name)}")
    lines.append("")

    def stem(shape: str, name: str) -> str:
        return f"heaptrack-{shape}-{name}"

    head(
        "The recordings, each pair under its figure's own container plus",
        f"heaptrack's {HEAPTRACK_TERM // config.GIB} GiB. --record-only: nothing is handed to heaptrack_gui,",
        "which need not be installed and, where it is, may not start. Each",
        "starts from no cache -- a `parse` resumes from one, so the second run",
        "of a pair would otherwise scan nothing at all -- and writes its",
        f"container's memory.peak beside it, as <stem>{HEAPTRACK_PEAK_SUFFIX}: a recording",
        "near its limit says so there, and that number re-sizes the term.",
    )
    for axis, memory, _ in HEAPTRACK_PAIRS:
        pc = profile.contained_prefix(cfg, memory)
        for shape, name in axis:
            src = warm / inputs.input_file(name)
            argv = " ".join(heaptrack_argv(shape, src, cache))
            recording = out / stem(shape, name)
            peak = out / (stem(shape, name) + HEAPTRACK_PEAK_SUFFIX)
            inner = (
                f"{HEAPTRACK} --record-only -o {recording} {binary} {argv} >/dev/null; "
                f"s=$?; cat {CGROUP_MEMORY_PEAK} > {peak}; exit $s"
            )
            lines += [
                "",
                f"rm -f {cache} {out / (stem(shape, name) + HEAPTRACK_SUFFIX)} {peak}",
                f"{pc} sh -c {shlex.quote(inner)}",
            ]
    lines.append("")

    head(
        "The reports. --merge-backtraces=0, because a merged frame's peak is",
        "the sum over the backtraces merged into it and heaptrack_print's own",
        "--help says it is not correct; c++filt, because the Rust frames are",
        "v0-mangled and this heaptrack cannot demangle them. Both are the",
        "image's, so the printer reads its recorder's format. Each pipe runs",
        "inside one container, writing its file there: a container's own",
        "output reaches the host through the runtime's logger, which drops",
        "what is still unwritten when the container exits.",
    )
    def report(pc: str, recording: str, written: Path, diff: str = "") -> list[str]:
        against = f" --diff {diff}" if diff else ""
        inner = f"{printer} --merge-backtraces=0 {recording}{against} | {HEAPTRACK_DEMANGLE} > {written}"
        return [f"{pc} sh -c {shlex.quote(inner)}"]

    for axis, memory, _ in HEAPTRACK_PAIRS:
        pc = profile.contained_prefix(cfg, memory)
        for shape, name in axis:
            lines += report(
                pc, str(out / (stem(shape, name) + HEAPTRACK_SUFFIX)),
                out / (stem(shape, name) + ".txt"),
            )
    lines.append("")

    head(
        "Each pair read as a difference, its first recording less its second.",
        "The step pair: the block path against the streaming fallback, one",
        "byte of budget apart -- every term the admitted reader adds. The",
        "first-pass pair: twenty-four jobs against four at the limit leg --",
        "every term twenty added workers ask for. This is what a single",
        "recording cannot give, with no subtraction between sittings and no",
        "spread to carry.",
    )
    for axis, memory, diff in HEAPTRACK_PAIRS:
        (first, first_input), (second, second_input) = axis
        lines += report(
            profile.contained_prefix(cfg, memory),
            str(out / (stem(first, first_input) + HEAPTRACK_SUFFIX)),
            out / (diff + ".txt"),
            diff=str(out / (stem(second, second_input) + HEAPTRACK_SUFFIX)),
        )
    lines.append("")

    head("Tear down: the staged inputs, and the cache the recordings shared.")
    lines.append(
        "rm -f "
        + " ".join(str(warm / inputs.input_file(n)) for n in staged)
        + f" {cache}"
    )
    return "\n".join(lines)


def cmd_heaptrack() -> int:
    cfg = config.Config()
    print(
        "# A heaptrack recording is not a figure: no medians, no apparatus gate,\n"
        "# no marker in measurements.md. It is a runs/ artifact read for\n"
        "# attribution (measurements.md, \"What an instrument can see\"). The\n"
        "# sequence is minutes, so it is not a detached job.\n"
    )
    print(heaptrack_recipe(cfg))
    return 0
