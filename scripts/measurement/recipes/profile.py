"""The profiling recipe: a sampling profile is no figure, so printed."""

from __future__ import annotations

import shlex
from pathlib import Path

import release

from measurement import config
from measurement import parsing
from measurement import shapes
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import release_profile


#: The profiler: the release image's `perf`, run as the invoking user in a
#: container whose seccomp lets `perf_event_open` reach the kernel, where this
#: machine's `perf_event_paranoid = 2` permits user-space sampling of one's own
#: processes (`contained.py`) -- and user space is what this phase is about.
#:
#: **libc's frames are named by the image's `libc6-dbg`**, which apt holds to
#: its `libc6`'s version, so the symbols match the libc every profiled binary
#: runs on by construction. This is not a nicety: a stripped libc puts ~48% of a
#: warm `parse` profile into bare addresses in `libc.so.6` -- and those
#: addresses are `__memmove_avx_unaligned_erms` and
#: `__memset_avx2_unaligned_erms`, the two functions a phase about zero-copy most
#: needs to see.
PERF = "perf"

#: The limit each profiled run's container is given (`contained.py`): a guard
#: on the machine, not a figure's budget. A shape stating no allowance
#: discovers this one rather than its figure's container's -- a different
#: budget, under which it runs the same code -- and `perf` is charged to it.
PROFILE_MEMORY = "4g"

#: The profiled build: `profiling`'s line tables, and the frame pointers no
#: Cargo profile key states, as `RUSTFLAGS` for this build alone.
PROFILE_BUILD = release.Build(profile="profiling", rustflags="-C force-frame-pointers=yes")

#: The introspection build the `pgdt sql` pair's reading runs, never timed.
DFCLI_INTROSPECT_BUILD = release.Build(features=("introspect",))


def contained_prefix(cfg: config.Config, memory: str, *mount: Path) -> str:
    """The head of a printed recipe line run in a container under `memory`
    (`contained.py`), `Config.alloc_build_root` mounted for the binaries built
    there, which a report reads without its command naming them, and each of
    `mount`, read the same way."""
    mounts = "".join(f"--mount {m} " for m in (cfg.alloc_build_root, *mount))
    return f"python3 {config.REPO / 'scripts/contained.py'} --memory {memory} {mounts}--"


def recipe_build_line(build: release.Build, target_dir: Path) -> str:
    """A printed recipe's build of `build` in the release image, held to its
    glibc there (`release.py build`), into `target_dir`."""
    opts = " ".join(shlex.quote(o) for o in build.options() if o != "--package=pgdt")
    return (
        f"(cd scripts && uv run release.py build --target {build.target} {opts} \\\n"
        f"  --target-dir {target_dir})"
    )

#: Sampling frequency, in Hz. Prime, so it cannot fall into lockstep with a
#: periodic phase of the thing being sampled.
#:
#: **4999 rather than the customary 997, and that is measured rather than
#: preferred.** `parse` is the thin shape -- ~0.85 s warm on the control, so
#: 997 Hz yields ~850 samples -- and over three reps at 997 the `memmove`
#: bucket read 25.8 / 33.6 / 30.9 % and the two `memchr` kernels swapped rank.
#: At 4999 the same three reps read 30.9 / 30.6 / 30.2 and the ranks hold. What
#: does *not* tighten is the worker thread's `memset`, 29.2 / 21.3 / 18.9, so
#: that spread is the workload rather than the instrument -- which is worth
#: knowing before a profile is read as though every bucket were equally solid.
#: The cost is data-file size (~0.7 MB for `parse`, ~19 MB for the longest
#: shape) on a gitignored directory, and the kernel's own ceiling here is
#: `perf_event_max_sample_rate` = 49000.
PERF_FREQ = 4999

#: The command shapes profiled. `parse` is discovery at the metadata level,
#: read against the scan-throughput tables and `statistics-gathering`'s
#: `metadata` leg; `query-strings` is row extraction
#: before a column is typed and `query-typed` the whole path, read against
#: `nested-end-to-end`, each over the data-level cache its figure reads
#: (`profile_builder_argv`). **The data level's `parse`**, `statistics-gathering`'s
#: `data` leg run without its resident wrapper, is what attributes that
#: figure's Δ among the census, the unrepresentable count and the statistics
#: (`docs/design/roadmap.md`, "Attribution is introspective; only the gate is
#: blind"): no figure differences a build without one of them. **Every `pgdt`
#: shape is profiled at one worker**, and the plain `parse`'s loss past four
#: (D25, unseparated) is not profiled: its suspect, a fused worker waiting on
#: a `POOL_DEPTH` slot, is off-CPU, where an on-CPU `perf record` sees
#: nothing, so its attribution would be an introspective counter of time
#: blocked on the pool, and nothing consumes one while D2 recommends one
#: plain worker.
PROFILE_SHAPES: tuple[str, ...] = (
    "parse",
    f"{shapes.STATISTICS_FAMILY}data-rss",
    "query-strings",
    "query-typed",
)

#: The two inputs. The brace-free control is the shape most dumps have; the
#: `--arrays --composite` file is where the nested path is reached at all, and
#: where the census inspects array shapes.
PROFILE_INPUTS: tuple[str, ...] = ("control", "arrays")

#: The row of a `pgdt sql` figure whose legs are profiled as a
#: pair and read by the introspection build, as `(figure, query)`:
#: `dynamic-filter-join`'s costing row, where evaluating rows rejects none.
#: **A pair because the row is a difference**: what evaluating rows adds over
#: the default leg is read bucket by bucket, one leg's profile against the
#: other's, and a profile of either alone answers nothing. Its input is
#: `dynfilter`, its cache the figure's own `GATHER_STATISTICS`, and each leg's
#: command `dfcli_invocation`'s, so the profiled run is the timed one.
DFCLI_ACCOUNT: tuple[str, str] = ("join", "costing")

#: The pair's legs: the filter on at the provider's default against rows
#: evaluated, so the setting alone separates them — the comparison
#: `decisions.md`, "D93" is refused and reopened on.
DFCLI_ACCOUNT_LEGS: tuple[str, str] = ("on", shapes.DYNFILTER_ROWS_LEG)

#: How many times the introspection build runs each of those legs. Its
#: spans are summed within a run; three runs say how far one moves.
DFCLI_INTROSPECT_REPS = 3


def profile_argv(command: str, source: Path | str, cache: Path | str) -> list[str]:
    """The `pgdt` arguments one profiled shape runs.

    Deliberately the same flags `_script` hands the sweep, because a profile is
    only readable against the figure it explains -- and a profile of a shape no
    figure times answers a question nobody asked. The two are separate
    functions because `_script` builds a *container* command line with a `time`
    builtin in front of it and this one builds a host argv;
    `test_profile.py`'s `ProfileRecipe` reconciles them shape by shape, so a
    change to a measured invocation that misses this one fails there rather
    than being discovered in a profile that quietly measured something else."""
    if command == "parse":
        return [
            "parse",
            "--source", str(source),
            "--cache", str(cache),
            "--jobs", str(shapes.SWEEP_JOBS),
            *shapes.NO_STATISTICS.split(),
        ]
    if command.startswith(shapes.STATISTICS_FAMILY):
        # The leg's own `parse`, the resident wrapper left off: a profile is
        # the process's proportions, and the wrapper is a second process.
        leg, _, suffix = command.removeprefix(shapes.STATISTICS_FAMILY).partition("-")
        flags = dict(shapes.STATISTICS_LEGS)
        if leg not in flags or suffix != "rss":
            raise ValueError(f"unknown profile shape {command!r}")
        return [
            "parse",
            "--source", str(source),
            "--cache", str(cache),
            "--jobs", str(shapes.SWEEP_JOBS),
            *flags[leg].split(),
        ]
    if command in ("query-strings", "query-typed"):
        # Over the cache `profile_builder_argv` writes first, unprofiled.
        mode = command.split("-")[1]
        return [
            "query",
            "--source", str(source),
            "--table", "public.perf",
            "--cache", str(cache),
            "--statistics", "none",
            "--schema-mode", mode,
            "--jobs", str(shapes.SWEEP_JOBS),
        ]
    raise ValueError(f"unknown profile shape {command!r}")


def profile_builder_argv(command: str, source: Path | str, cache: Path | str) -> list[str] | None:
    """The `pgdt parse` a profiled shape's cache is written by first, outside
    the profile, or `None` for a shape that reads none.

    `DATA_LEVEL_BUILDER`'s flags, as `profile_argv` is `_script`'s: a query
    shape is timed over a data-level cache, so a profile of it over none would
    attribute a mapping pass the figure never times. `test_profile.py`'s
    `ProfileRecipe` holds the two builders together."""
    if not command.startswith(shapes.DATA_LEVEL_QUERIES):
        return None
    return [
        "parse",
        "--source", str(source),
        "--cache", str(cache),
        "--jobs", str(shapes.SWEEP_JOBS),
        *shapes.GATHER_STATISTICS.split(),
    ]

#: The `release-profile` legs the downstream-build rule's differential sets
#: side by side (`roadmap.md`, "A downstream build is within 3% of ours"), in
#: `perf diff`'s order: the shipped leg as the baseline, then the default leg
#: read against it, so a symbol the default leg alone leaves out of line is
#: one the baseline lacks.
RELEASE_DIFF_LEGS: tuple[str, str] = (leg_binaries.SHIPPED_PROFILE_LEG, leg_binaries.PROFILE_LEGS[0][0])


def release_diff_invocation(
    command: str, source: Path | str, cache: Path | str
) -> tuple[list[str], list[str], list[str] | None]:
    """The environment, the arguments (subcommand first) and the untimed
    builder one `RELEASE_PROFILE_SHAPES` shape runs on a leg's binary.

    A `pgdt` shape is `profile_argv`'s and `profile_builder_argv`'s, over
    `cache`; `PARALLEL_SCAN` is `typed_scan_invocation`'s over a cache
    `PROVIDER_BUILDER`'s flags write beside the dump, where `--source` looks, and
    `cache` goes unused. `test_profile.py`'s `ProfileRecipe` holds each to
    `_script`'s timed line, so the differential runs the workload the verdict
    was read on."""
    if command == shapes.PARALLEL_SCAN:
        env, argv = shapes.typed_scan_invocation(str(source))
        builder = [
            "parse",
            "--source", str(source),
            "--cache", f"{source}.pgdt",
            "--jobs", str(shapes.SWEEP_JOBS),
            *shapes.GATHER_STATISTICS.split(),
        ]
        return env, ["sql", *argv], builder
    if command not in dict(release_profile.RELEASE_PROFILE_SHAPES):
        raise ValueError(f"{command!r} is not a `release-profile` shape")
    return [], profile_argv(command, source, cache), profile_builder_argv(command, source, cache)


def profile_recipe(cfg: config.Config) -> str:
    """The whole sequence, with every path filled in.

    Six things here decide whether the profile is of the thing it claims to
    be, and each fails *silently* -- a profile comes back, it just describes
    something else. `test_profile.py` asserts all six:

    * **the `profiling` binary, never a `release` one.** `release`
      keeps its symbol table, so a flat profile of it names functions, but it
      carries no line tables and no frame pointers: no source lines for
      `perf annotate`, and every call chain the reports below print stops at
      the leaf. The published figures stay on `release`, which is why this
      is a second binary rather than a change to the one they use, built in
      the image (`PROFILE_BUILD`) in a target directory of its own.
    * **`-C force-frame-pointers=yes` on the build line.** Cargo has no profile
      key for frame pointers, so `[profile.profiling]` alone gives line tables
      and a call graph that stops at the leaf; `release.py build --rustflags`
      states them for this build alone.
    * **`--call-graph fp`, matching that build.** `dwarf` would need full debug
      info this profile does not carry, and its 8 KiB stack copies per sample
      would change the thing being measured.
    * **warm input, on tmpfs.** A profile taken off the SSD is a profile of
      `pread` waiting for a device; the proportions this phase reads are CPU
      proportions. Staging is a host copy for the same reason the sweep's is --
      writing 3 GiB of tmpfs from inside the 512 MB container charges those
      pages to its cgroup.
    * **every recording and report in the image**, whose `libc6-dbg` names
      libc's frames (`PERF`): a report read where the recording's libc is not
      resolves none of them, and ~48% of a warm `parse` profile is then bare
      addresses.
    * **the worker count, stated rather than inherited.** `profile_argv` states
      one for the same reason `_script` does, and here the consequence is
      sharper than a moved number: a sampling profile's buckets are per
      *thread*, so a profile taken at the machine's available parallelism
      attributes a scan among workers the figure it explains never ran. Every
      `pgdt` shape states `SWEEP_JOBS`, and `DFCLI_ACCOUNT`'s legs the
      partition count `dfcli_invocation` states. The shape-equality assertion
      is what holds this function and `_script` together, and it compares two
      shapes that each pin a count rather than two that each inherit one.

    **Every line running a binary runs it in a container** under
    `PROFILE_MEMORY` (`contained.py`), as every run of a built binary is: the
    limit guards the machine and leaves a profile's proportions as they were.

    **`DFCLI_ACCOUNT`'s pair runs `pgdt sql`**, off the same
    profiling build, over a cache the figure's own `pgdt parse` writes beside
    the dump where `--source` looks, each leg stating `dfcli_invocation`'s
    environment and arguments. Beside it, **the introspection build**
    (`--features introspect`, in a target directory of its own so no timed
    binary is overwritten) runs the same legs, each run writing what
    `pgdt_core::instrument` timed of its row evaluation to the file
    `INSTRUMENT_OUT_VAR` names: a `runs/` artifact, like the profiles, and
    never a figure.

    **The downstream-build rule's differential** profiles no build of its own:
    it records a `release-profile` sitting's `RELEASE_DIFF_LEGS` binaries
    where the sitting left them (`profile_leg_binary`), each asked first which
    leg it is, on every shape's own argv and builder
    (`release_diff_invocation`) over the figure's input, and prints `perf diff
    --sort symbol` of the pair. The legs are one tree's, the figure building
    every leg before its first reading, so nothing in it subtracts one commit
    from another; and since the next sitting overwrites them, a failing
    workload is profiled before it. Flat, with no `--call-graph`: the legs
    keep `.symtab` and carry no frame pointers, and `#[inline]` goes on the
    callee, whose name is the remedy's whole input. **It is read for names,
    never for a time** -- what the default leg alone leaves out of line --
    as any profile is read for proportions. Like every profile here a shape
    stating no allowance discovers `PROFILE_MEMORY` rather than the figure
    container's: a different budget, under which each leg runs the same code."""
    warm = cfg.warm_dir
    profile_target = image.image_target_dir(cfg, "profiling")
    binary = PROFILE_BUILD.binary(profile_target)
    sql = f"{binary} sql"
    introspect_target = image.image_target_dir(cfg, "dfcli-introspect")
    introspect = f"{DFCLI_INTROSPECT_BUILD.binary(introspect_target)} sql"
    c = contained_prefix(cfg, PROFILE_MEMORY)
    cache = warm / "profile.pgdt"
    out = cfg.out_dir
    figure, query = DFCLI_ACCOUNT
    account_input = "dynfilter"
    account_dump = warm / f"{account_input}.sql"
    account_cache = warm / f"{account_input}.sql.pgdt"

    lines: list[str] = []
    step = 0

    def head(*text: str) -> None:
        """A numbered step, numbered as emitted rather than by literal."""
        nonlocal step
        lines.extend([f"# {step}. {text[0]}", *(f"#    {t}" for t in text[1:])])
        step += 1

    head(
        "The tool: the release image's, run in a container as every line",
        "below that runs a binary is (scripts/contained.py), under a limit",
        "that guards the machine. `perf_event_paranoid = 2` already permits",
        "user-space sampling of one's own processes, so nothing needs a sysctl;",
        "the image's libc6-dbg names libc's frames.",
    )
    lines += [f"{c} {PERF} --version", ""]

    head(
        "The profiling build, in the release image and a target directory of",
        "its own. Frame pointers are not a Cargo profile key, so they come",
        "from --rustflags; no timed binary is rebuilt.",
    )
    lines += [recipe_build_line(PROFILE_BUILD, profile_target), ""]

    head(
        "The introspection build, whose `sql` the pair runs, in a target directory",
        "of its own: no timed binary is overwritten, and it is never timed.",
    )
    lines += [recipe_build_line(DFCLI_INTROSPECT_BUILD, introspect_target), ""]

    # Every input any profile below reads, in declaration order and without
    # repetition: the cross product's, then the `pgdt sql` pair's.
    (release_input,) = {
        spec.input for spec in release_profile._release_profile_specs() if spec.binary.startswith("profile:")
    }
    release_dump = warm / f"{release_input}.sql"
    staged = list(PROFILE_INPUTS)
    staged += [account_input] if account_input not in staged else []
    staged += [release_input] if release_input not in staged else []

    head("Stage the inputs warm, on the host.")
    lines.append(f"mkdir -p {warm} {out}")
    for name in staged:
        lines.append(f"cp -n {cfg.cache_dir / f'{name}.sql'} {warm / f'{name}.sql'}")
    lines.append("")

    def record(shape: str, name: str) -> None:
        stem = f"profile-{shape}-{name}"
        argv = " ".join(profile_argv(shape, warm / f"{name}.sql", cache))
        builder = profile_builder_argv(shape, warm / f"{name}.sql", cache)
        lines.extend(
            [
                "",
                f"rm -f {cache}",
                *([f"{c} {binary} {' '.join(builder)} >/dev/null"] if builder else []),
                f"{c} {PERF} record -F {PERF_FREQ} --call-graph fp "
                f"-o {out / (stem + '.data')} \\",
                f"  -- {binary} {argv} >/dev/null",
                f"{c} {PERF} report -i {out / (stem + '.data')} --stdio --no-children \\",
                f"  --percent-limit 0.5 > {out / (stem + '.txt')}",
            ]
        )

    head("The profiles. Each is a runs/ artifact, not a figure.")
    for name in PROFILE_INPUTS:
        for shape in PROFILE_SHAPES:
            record(shape, name)
    lines.append("")
    head(
        f"The pair `{figure}`'s `{query}` row is read as: its legs",
        f"{' and '.join(DFCLI_ACCOUNT_LEGS)}, the setting alone apart, over a cache the",
        "figure's own `pgdt parse` writes beside the dump, where `--source` looks.",
        "Read as a difference, bucket by bucket.",
    )
    lines += [
        f"rm -f {account_cache}",
        f"{c} {binary} parse --source {account_dump} --cache {account_cache} "
        f"--jobs {shapes.SWEEP_JOBS} {shapes.GATHER_STATISTICS} >/dev/null",
    ]
    for leg in DFCLI_ACCOUNT_LEGS:
        env, argv = shapes.dfcli_invocation(figure, query, leg, str(account_dump))
        stem = f"profile-dfcli-{figure}-{query}-{leg}"
        lines.extend(
            [
                "",
                f"{c} {' '.join(['env', *env]) + ' ' if env else ''}{PERF} record "
                f"-F {PERF_FREQ} --call-graph fp -o {out / (stem + '.data')} \\",
                f"  -- {shapes.dfcli_shell([], sql, argv).lstrip()} >/dev/null",
                f"{c} {PERF} report -i {out / (stem + '.data')} --stdio --no-children \\",
                f"  --percent-limit 0.5 > {out / (stem + '.txt')}",
            ]
        )
    lines.append("")
    head(
        "The per-term reading: the same legs on the introspection build, each run",
        "writing what it timed of its row evaluation. The default leg evaluates no",
        "row, so it times no span but `Chunk`: the control that only rows are timed.",
    )
    for leg in DFCLI_ACCOUNT_LEGS:
        env, argv = shapes.dfcli_invocation(figure, query, leg, str(account_dump))
        for rep in range(1, DFCLI_INTROSPECT_REPS + 1):
            report = out / f"introspect-dfcli-{figure}-{query}-{leg}-{rep}.txt"
            lines.append(
                f"{c} env {parsing.INSTRUMENT_OUT_VAR}={report} "
                f"{shapes.dfcli_shell(env, introspect, argv).lstrip()} >/dev/null"
            )
    lines.append("")
    head(
        "The downstream-build rule's attribution, for a workload a `release-profile`",
        "sitting's verdict reads as lost inlining: a flat profile of that sitting's own",
        f"{RELEASE_DIFF_LEGS[1]} and {RELEASE_DIFF_LEGS[0]} legs on the workload's own argv,",
        f"and `perf diff` of the two, {RELEASE_DIFF_LEGS[0]} the baseline. A hot symbol only",
        f"{RELEASE_DIFF_LEGS[1]} carries is a call it leaves out of line: `#[inline]` goes",
        "on it, or, where it is not ours, `rg` finds the calls crossing to it.",
        "Read for names, never for a time. Run only the failing workload's block, and",
        "before the next sitting, which overwrites the legs; each must name itself first.",
    )
    for leg in RELEASE_DIFF_LEGS:
        leg_binary = leg_binaries.profile_leg_binary(cfg, leg)
        lines.append(
            f"{c} {leg_binary} --version | grep -qF '(profile: {leg})' "
            f"|| echo '{leg_binary} is not the {leg} leg: stop' >&2"
        )
    for command, label in release_profile.RELEASE_PROFILE_SHAPES:
        env, argv, builder = release_diff_invocation(command, release_dump, cache)
        written = builder[builder.index("--cache") + 1] if builder else str(cache)
        lines += ["", f"# {label}"]
        for leg in RELEASE_DIFF_LEGS:
            leg_binary = leg_binaries.profile_leg_binary(cfg, leg)
            data = out / f"release-diff-{command}-{leg}.data"
            lines += [
                f"rm -f {written}",
                *([f"{c} {leg_binary} {' '.join(builder)} >/dev/null"] if builder else []),
                f"{c} {' '.join(['env', *env]) + ' ' if env else ''}{PERF} record "
                f"-F {PERF_FREQ} -o {data} \\",
                f"  -- {shapes.dfcli_shell([], str(leg_binary), argv).lstrip()} >/dev/null",
            ]
        baseline, read = (out / f"release-diff-{command}-{leg}.data" for leg in RELEASE_DIFF_LEGS)
        lines.append(
            f"{c} {PERF} diff --sort symbol {baseline} {read} "
            f"> {out / f'release-diff-{command}.txt'}"
        )
    lines.append("")
    head("Tear down: tmpfs is 16 G and these inputs do not fit beside a sweep's.")
    lines.append(
        f"rm -f {' '.join(str(warm / f'{n}.sql') for n in staged)} {cache} {account_cache} "
        f"{release_dump}.pgdt"
    )
    return "\n".join(lines)


def cmd_profile() -> int:
    cfg = config.Config()
    print(
        "# A profile is not a figure: no medians, no apparatus gate, no marker in\n"
        "# measurements.md. It is a runs/ artifact, read for proportions\n"
        "# (measurements.md, \"The apparatus\"). The whole\n"
        "# sequence is minutes, so it is not a detached job.\n"
    )
    print(profile_recipe(cfg))
    return 0
