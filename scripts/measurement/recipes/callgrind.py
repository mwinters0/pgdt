"""The callgrind recipe: a callgrind count is no figure, so printed."""

from __future__ import annotations

from pathlib import Path

import contained

from measurement import config
from measurement import count_pair
from measurement import inputs
from measurement import shapes
from measurement.recipes import profile


#: The counter: the release image's `valgrind`, run under `contained.py` as
#: every recipe line running a binary is.
CALLGRIND = "valgrind"

#: Each callgrind run's container limit: a guard on the machine, sized to hold
#: valgrind beside the run. **The run is not given it**: each states the limit
#: its row's pair container had as its allowance (`callgrind_invocation`), so it
#: carves what the counted run discovered rather than this.
CALLGRIND_MEMORY = "4g"

#: What a counted run is started under, so `pgdt` is not its PID namespace's
#: init: valgrind runs its guest in its own process, and as init `pgdt` installs
#: a handler on every ending signal (`namespace-init`), the real-time one valgrind
#: reserves among them, which it refuses. Not `exec`'d, so `sh` stays the init
#: and `pgdt` its child, as `pgdt` is a child in the pair's container.
CALLGRIND_NOT_INIT = "sh -c '\"$@\"; exit $?' sh"


def callgrind_prefix(cfg: config.Config) -> str:
    """The head of each callgrind recipe line: its limit, and the input
    cache mounted, which the linked input leads to without a line naming it."""
    return profile.contained_prefix(cfg, CALLGRIND_MEMORY, cfg.cache_dir)

#: Runs per leg. valgrind serializes a process's threads, so two runs of one
#: binary differ by scheduling alone, and a rise smaller than that spread is not
#: placed.
CALLGRIND_REPS = 2

#: How many of the largest per-function changes each placement keeps.
CALLGRIND_TOP = 60

#: Each function's self `Ir` change between two `callgrind_annotate` listings,
#: parent's first, as `|change| TAB change TAB function`, for `sort -rn`. A count
#: is right-aligned to the widest, so it may follow blanks. The object in
#: brackets is dropped, since each leg's binary has its own path, and a name met
#: twice is summed. No single quote, as it is printed inside them.
CALLGRIND_DIFF_AWK = (
    "FNR == 1 { leg++ } / PROGRAM TOTALS$/ { next } "
    "/^ *[0-9][0-9,]*  / { n = $1; gsub(/,/, \"\", n); sub(/^ *[0-9][0-9,]*  /, \"\"); "
    "sub(/ \\[[^]]*\\]$/, \"\"); ir[leg, $0] += n; seen[$0] = 1 } "
    "END { for (f in seen) { d = ir[2, f] - ir[1, f]; "
    "if (d != 0) printf \"%.0f\\t%+.0f\\t%s\\n\", (d < 0 ? -d : d), d, f } }"
)


def callgrind_invocation(
    command: str, source: Path | str, cache: Path | str, memory: int
) -> tuple[list[str], list[str], list[str] | None]:
    """The environment, the arguments (subcommand first) and the untimed
    builder one `COUNT_PAIR_SHAPES` row runs under callgrind, each stating
    `memory` as its allowance.

    `_script`'s timed line and builder flag for flag, with one addition:
    **the allowance the pair's container gave the row, stated**, `--memory` on
    a `pgdt` command and `SET pgdump.memory` ahead of the provider's query,
    since callgrind's container is larger and a discovered allowance would
    carve something else. `test_callgrind.py`'s `CallgrindRecipe` holds the rest
    to `_script`."""
    stated = ["--memory", str(memory)]
    if command == shapes.PARALLEL_SCAN:
        env, argv, builder = profile.release_diff_invocation(command, source, cache)
        query = argv.index("-c")
        argv = [*argv[:query], "-c", f"SET pgdump.memory = {memory}", *argv[query:]]
        return env, argv, [*(builder or []), *stated]
    if command.startswith(shapes.STATISTICS_FAMILY):
        flags = dict(shapes.STATISTICS_LEGS)
        leg = command.removeprefix(shapes.STATISTICS_FAMILY)
        if leg not in flags:
            raise ValueError(f"{command!r} is not a `--count-pair` shape")
        argv = [
            "parse",
            "--source", str(source),
            "--cache", str(cache),
            "--jobs", str(shapes.SWEEP_JOBS),
            *flags[leg].split(),
        ]  # fmt: skip
        return [], [*argv, *stated], None
    if command == shapes.STRICT_PARSE:
        argv = [
            "parse",
            "--source", str(source),
            "--cache", str(cache),
            "--jobs", str(shapes.SWEEP_JOBS),
            *shapes.GATHER_STATISTICS.split(),
            *shapes.STRICT_FLAG.split(),
        ]  # fmt: skip
        return [], [*argv, *stated], None
    if command not in {c for c, *_ in count_pair.COUNT_PAIR_SHAPES}:
        raise ValueError(f"{command!r} is not a `--count-pair` shape")
    builder = profile.profile_builder_argv(command, source, cache)
    return (
        [],
        [*profile.profile_argv(command, source, cache), *stated],
        [*builder, *stated] if builder else None,
    )


def callgrind_recipe(cfg: config.Config) -> str:
    """The whole sequence, with every path filled in.

    Each of these fails silently, returning a placement of some other
    workload, and `test_callgrind.py`'s `CallgrindRecipe` asserts each:

    * **the pair's own binaries, never rebuilt** (`count_pair_leg_binary`).
      The rise being placed was counted in those two files; a rebuild is
      another link, and each is asked that it is the shipped build first.
    * **the pair's own inputs, shapes and worker count**: each row's input
      off the SSD cache, unstaged as the pair reads it, and each row
      `_script`'s line (`callgrind_invocation`).
    * **the allowance the pair's container gave**, stated, since this
      container is larger: a discovered allowance moves the readers and the
      statistics a run holds (D85).
    * **a fresh cache before every run**, the pair's container starting from
      none, and a query row's builder ahead of it, uncounted.
    * **both legs and two runs each, the leg going first alternated** as the
      pair orders its reps, and the placement read on each rep pair.

    The reading is D108's: the totals first, and only where they reproduce
    the pair's median change are the functions read as its place.

    What the recipe leaves to whoever runs it:

    * **a commit other than the tree** is priced by `--count-pair
      <parent>..<change>`, never a pair from a second checkout, Cargo taking
      the shared `count-pair-tree` build as current by mtime;
    * **a span's residual** is placed on the gap between each pair of
      adjacent legs, each gap's candidates its own commits
      (`runs/count-pair-33.26-20261112/callgrind/{gen,analyze}.py` the
      pattern), and a job of several pairs keeps each step's legs, which the
      next pair overwrites (`runs/count-pair-33.26-20261112/run.sh`);
    * **placements run four or five at once**, an `Ir` count not moving with
      the machine's load; on `query --schema-mode typed` the gate can miss on
      one rep pair, mimalloc's paths moving, and the strings query's functions
      place the rise."""
    c = callgrind_prefix(cfg)
    work = cfg.out_dir / "callgrind-count-pair"
    legs = {leg: count_pair.count_pair_leg_binary(cfg, leg) for leg in count_pair.COUNT_PAIR_LEGS}
    names = list(dict.fromkeys(name for *_, name in count_pair.COUNT_PAIR_SHAPES))
    dumps = {name: work / inputs.input_file(name) for name in names}
    cache = work / "x.pgdt"

    lines: list[str] = []
    step = 0

    def head(*text: str) -> None:
        """A numbered step, numbered as emitted rather than by literal."""
        nonlocal step
        lines.extend([f"# {step}. {text[0]}", *(f"#    {t}" for t in text[1:])])
        step += 1

    head(
        "The tool: the release image's, in a container as every line below",
        "running a binary is (scripts/contained.py), under a limit sized to hold",
        "valgrind and given to no run as its allowance.",
    )
    lines += [f"{c} {CALLGRIND} --version", ""]

    head(
        "The legs: the two binaries `--count-pair` counted, where it left them.",
        "Never rebuilt, since the rise placed is the one counted in these files, and",
        "run before the next pair, which overwrites them. Each must say it is the",
        "shipped build; their digests are kept beside the reading.",
    )
    for leg, binary in legs.items():
        lines.append(
            f"{c} {binary} --version | grep -qF '(profile: release)' "
            f"|| echo '{binary} is not the shipped build: stop' >&2"
        )
    lines += [
        f"mkdir -p {work}",
        f"sha256sum {' '.join(str(b) for b in legs.values())} > {work / 'legs.sha256'}",
        "",
    ]

    head(
        "The inputs: the pair's, off the SSD cache and unstaged, as the pair reads",
        "them. Linked, so a cache the provider writes beside a dump lands here",
        "rather than in the input cache.",
    )
    lines += [f"ln -sf {cfg.cache_dir / inputs.input_file(name)} {dump}" for name, dump in dumps.items()]
    lines.append("")

    head(
        "One block per row; run only the block of a row the pair resolved. Each run",
        "states the allowance its pair container gave it, starts from no cache as",
        "the pair's container did, and writes pgdt's stderr beside its count: its",
        "arrangement line must read the same in all four. Then the gate, read",
        "before any function: each rep pair's total, tree less parent, against the",
        "pair's median change for the row; where neither reproduces it, the rise",
        "is layout (D108). Where both do, the place: each function's self `Ir`",
        "change on each rep pair, largest first, the two agreeing. Changed code",
        "at the top is the change's cost, reworked or accepted; identical code",
        "called as often is layout (D108).",
    )
    reps = CALLGRIND_REPS
    for command, label, memory, name in count_pair.COUNT_PAIR_SHAPES:
        allowance = contained.size_bytes(memory or cfg.memory)
        row_name = count_pair.count_pair_row(command, name)
        env, argv, builder = callgrind_invocation(command, dumps[name], cache, allowance)
        written = builder[builder.index("--cache") + 1] if builder else str(cache)
        lines += ["", f"# {label}"]
        for rep in range(1, reps + 1):
            order = count_pair.COUNT_PAIR_LEGS if rep - 1 < (reps + 1) // 2 else count_pair.COUNT_PAIR_LEGS[::-1]
            for leg in order:
                binary = legs[leg]
                run = work / f"{row_name}-{leg}-{rep}"
                lines += [
                    f"rm -f {written}",
                    *([f"{c} {binary} {' '.join(builder)} >/dev/null"] if builder else []),
                    f"{c} {CALLGRIND_NOT_INIT} {' '.join(['env', *env]) + ' ' if env else ''}"
                    f"{CALLGRIND} "
                    "--tool=callgrind --dump-instr=yes \\",
                    f"  --log-file={run}.log --callgrind-out-file={run}.out \\",
                    f"  {shapes.dfcli_shell([], str(binary), argv).lstrip()} >/dev/null 2>{run}.err",
                    f"{c} callgrind_annotate --inclusive=no --threshold=100 --show-percs=no "
                    f"--auto=no {run}.out > {run}.txt",
                ]
        runs = " ".join(
            str(work / f"{row_name}-{leg}-{rep}.txt")
            for rep in range(1, reps + 1)
            for leg in count_pair.COUNT_PAIR_LEGS
        )
        lines += [f"rm -f {written}", f"grep -H 'PROGRAM TOTALS' {runs}"]
        for rep in range(1, reps + 1):
            parent, tree = (work / f"{row_name}-{leg}-{rep}.txt" for leg in count_pair.COUNT_PAIR_LEGS)
            lines.append(
                f"awk '{CALLGRIND_DIFF_AWK}' \\\n  {parent} {tree} | sort -rn | cut -f2- "
                f"| head -n {CALLGRIND_TOP} > {work / f'{row_name}-place-{rep}.txt'}"
            )
    lines.append("")
    head("Tear down: the links, never the inputs they name.")
    lines += [f"rm -f {dump}" for dump in dumps.values()]
    return "\n".join(lines)


def cmd_callgrind() -> int:
    print(
        "# A callgrind count is not a figure: no medians, no apparatus gate, no\n"
        "# marker in measurements.md. It places a rise `--count-pair` resolved\n"
        "# (decisions.md, \"D108\"), in a runs/ directory. A row's four runs are\n"
        "# minutes each; past ten minutes in all, launch it detached (CLAUDE.md).\n"
    )
    print(callgrind_recipe(config.Config()))
    return 0
