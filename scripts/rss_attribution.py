#!/usr/bin/env python3
"""What the per-block resident growth is made of — `KD14`'s attribution.

**A diagnostic, not a figure.** It answers *which of several mechanisms*, read
as a proportion; its legs are ones no sweep would take (a second and a third
allocator, a scan stopped at the preamble, an index merely loaded); and a
resident set is not a timing, so none of the apparatus control a figure exists
for is doing anything here. That is the standing the profiling recipe has —
see `measurements.md`, "What the per-block resident growth is made of", which is
declared outside the register in `measure.NOT_OURS` and is where its numbers are
published.

**The instrument is `peak-rss`'s**, imported rather than copied: the same
`getrusage` wrapper, container, image and memory limit as `measure.py`'s
`parse-rss` shape, over the same staged inputs. That is what lets the first leg
be read against that figure's `blocks4000` row.

Every leg is taken at **two** block counts, so each reading is a slope in blocks
rather than one absolute at 4,000 with a fixed baseline folded into it — at 500
blocks the three allocators' baselines differ by 110 MiB, and jemalloc would
read as catastrophic on absolutes when its *slope* is within 35% of glibc's.

It prints its table; it writes no artifact. Paste the table into the doc's
section, which is the only place these numbers live.

    cd scripts && uv run rss_attribution.py
"""

from __future__ import annotations

import os
import platform
import re
import shutil
import subprocess
from pathlib import Path

import measure

CFG = measure.Config()

#: Where this diagnostic stages its inputs. Not `measure.Config.warm_dir`: a
#: sweep evicts that directory as it moves between figures, and running this
#: beside one should not fight it for the same paths.
STAGE = Path(os.environ.get("PGDQ_RSS_STAGE", "/dev/shm/pgdq-rss"))

#: The two block counts every leg is taken at, and the `measure` input each
#: names. Both are `generate_block_count_bench.py` outputs, so the pair differs
#: in block count and in nothing else.
INPUTS: tuple[str, ...] = ("blocks500", "blocks4000")
REPS = 3

BINARIES: dict[str, Path] = {
    "system": CFG.bin_pgdq,
    "jemalloc": CFG.out_dir / "pgdq-alloc-jemalloc",
    "mimalloc": CFG.out_dir / "pgdq-alloc-mimalloc",
}

#: The worker count every leg states, which is the harness's — a resident set
#: is exactly the quantity a worker count moves, since each worker holds read
#: buffers of its own, so a leg inheriting the CLI's `available_parallelism()`
#: default would attribute a growth this apparatus never measured
#: (`measure.SWEEP_JOBS`; `measurements.md`, "The apparatus"). `info` takes no
#: such flag and states none.
JOBS = f"--jobs {measure.SWEEP_JOBS}"

PARSE = f"parse --source /dump.sql --dqcache /tmp/x.dqcache {JOBS}"
#: `parse` refuses `--dqcache none` outright — "cache is disabled, but `parse`
#: requires a cache file" — so the un-throttled splice, one whole-list rebuild
#: per block, is reachable only through `query`. Its cached twin differs in that
#: one flag and in nothing else, which is what isolates the clone.
NOMATCH = "query --source /dump.sql --table public.nosuchtable --dqcache"

#: leg -> (allocator, the pgdq arguments). `CACHE` is the input's own name.
LEGS: dict[str, tuple[str, str]] = {
    "parse": ("system", PARSE),
    "parse, jemalloc": ("jemalloc", PARSE),
    "parse, mimalloc": ("mimalloc", PARSE),
    "parse --preamble-only": (
        "system",
        f"parse --preamble-only --source /dump.sql --dqcache /tmp/x.dqcache {JOBS}",
    ),
    "info over the finished cache": ("system", "info --dqcache /out/CACHE.dqcache"),
    "no match, cached": ("system", f"{NOMATCH} /tmp/x.dqcache {JOBS}"),
    "no match, none": ("system", f"{NOMATCH} none {JOBS}"),
    "no match, none, jemalloc": ("jemalloc", f"{NOMATCH} none {JOBS}"),
    "no match, none, mimalloc": ("mimalloc", f"{NOMATCH} none {JOBS}"),
}

MAXRSS = re.compile(r"^maxrss_kib=(\d+)$", re.M)


def container(binary: Path, dump: Path, script: str) -> list[str]:
    """One run of `script` in the `peak-rss` apparatus.

    `/out` is mounted for the `info` leg's cache, and is the staging directory
    rather than a container-local path so that the cache survives the run that
    built it."""
    return [
        *CFG.container_argv(), "run", "--rm",
        "-m", CFG.memory, "--memory-swap", CFG.memory,
        "-v", f"{binary}:/pgdq:ro",
        "-v", f"{dump}:/dump.sql:ro",
        "-v", f"{STAGE}:/out",
        CFG.image, "bash", "-c", script,
    ]


def run_leg(leg: str, name: str) -> int:
    allocator, args = LEGS[leg]
    binary = BINARIES[allocator]
    if not binary.exists():
        raise SystemExit(
            f"{binary} does not exist — build the allocator legs first:\n"
            f"  cargo build --release -p pgdump_query-cli --no-default-features "
            f"--features {allocator} --target-dir {CFG.alloc_build_root / allocator}"
        )
    # `rss_wrapper` already ends in the `--` that terminates perl's own option
    # parsing, so the command follows it directly; a second `--` makes `--` the
    # program perl execs, which fails looking exactly like a missing mount.
    wrapper = measure.rss_wrapper(platform.machine())
    script = f"{wrapper} /pgdq {args.replace('CACHE', name)} >/dev/null"
    proc = subprocess.run(
        container(binary, STAGE / f"{name}.sql", script),
        text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    if proc.returncode != 0:
        raise SystemExit(f"{leg} on {name} exited {proc.returncode}\n{proc.stderr[-2000:]}")
    found = MAXRSS.search(proc.stderr)
    if found is None:
        raise SystemExit(f"{leg} on {name}: no maxrss line in\n{proc.stderr[-2000:]}")
    return int(found.group(1))


def stage() -> None:
    """The inputs, and the `info` leg's cache — both outside the reps."""
    STAGE.mkdir(parents=True, exist_ok=True)
    for name in INPUTS:
        dump = STAGE / f"{name}.sql"
        if not dump.exists():
            print(f"staging {name}")
            shutil.copyfile(CFG.cache_dir / measure.input_file(name), dump)
        cache = STAGE / f"{name}.dqcache"
        if not cache.exists():
            print(f"building {name}.dqcache for the `info` leg")
            subprocess.run(
                container(
                    BINARIES["system"], dump,
                    f"/pgdq parse --source /dump.sql --dqcache /out/{name}.dqcache >/dev/null",
                ),
                check=True,
            )


def main() -> int:
    stage()
    readings: dict[tuple[str, str], list[float]] = {}
    for rep in range(REPS):
        # Reversed on alternate reps, for the reason `Session.sweep` reverses:
        # a session's own drift should not land on whichever leg went first.
        order = list(LEGS) if rep % 2 == 0 else list(reversed(list(LEGS)))
        for leg in order:
            for name in INPUTS:
                kib = run_leg(leg, name)
                readings.setdefault((leg, name), []).append(kib)
                print(f"  rep{rep + 1} {leg:<30} {name:<11} {kib / 1024:7.2f} MiB")

    small_n, big_n = (measure.input_block_count(n) for n in INPUTS)
    rows = []
    for leg in LEGS:
        small = measure.median(readings[(leg, INPUTS[0])])
        big = measure.median(readings[(leg, INPUTS[1])])
        rows.append([
            f"`{leg}`",
            measure.fmt_mib(small),
            measure.fmt_mib(big),
            f"{(big - small) * 1024 / (big_n - small_n):+,.0f} B",
        ])
    print("\n" + measure.md_table(
        ["Leg", f"{small_n:,} blocks", f"{big_n:,} blocks", "Per block"], rows
    ))
    print(f"\nPer-rep readings (MiB, {small_n:,} then {big_n:,}):")
    for leg in LEGS:
        legs = " · ".join(
            ", ".join(f"{v / 1024:.2f}" for v in readings[(leg, name)]) for name in INPUTS
        )
        print(f"- `{leg}`: {legs}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
