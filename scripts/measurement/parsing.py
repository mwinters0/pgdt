"""Reading what a timed run printed: bash's `time`, the resident-set wrapper, the instruction
counter, an instrument build's report, the OOM killer's verdict, criterion's estimates."""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path


TIME_RE = re.compile(r"^real\s+(\d+)m([\d.]+)s\s*$", re.MULTILINE)

#: The CPU the timed command spent, beside its wall clock.
CPU_TIME_RE = re.compile(r"^(user|sys)\s+(\d+)m([\d.]+)s\s*$", re.MULTILINE)

#: What every in-container script is prefixed with, outside every command
#: shape as `OOM_ORACLE` is: bash's default report at six decimals rather than
#: three. **Bash 5.3 honours it and 5.2 clamps it to three**, so an image's
#: bash decides whether it reads to the microsecond or the millisecond; the
#: report's shape is the default's either way, which `TIME_RE` reads.
TIME_FORMAT = "TIMEFORMAT=$'\\nreal\\t%6lR\\nuser\\t%6lU\\nsys\\t%6lS'; "


def parse_bash_time(text: str) -> float:
    """Seconds off a `bash -c 'time ...'` report.

    bash writes `real\\t0m0.570s` to stderr. There is exactly one such line per
    timed command; more than one means two commands were timed and the caller
    built the wrong script."""
    matches = TIME_RE.findall(text)
    if not matches:
        raise ValueError(f"no `real` line in timed output:\n{text[-2000:]}")
    if len(matches) > 1:
        raise ValueError(f"{len(matches)} `real` lines in one timed run; the script times more than one command")
    minutes, seconds = matches[0]
    return int(minutes) * 60 + float(seconds)


def parse_bash_cpu(text: str) -> dict[str, float]:
    """The `user` and `sys` seconds beside a `real` line, keyed by name.

    Recorded per run rather than read: a CPU-bound reading's user time moves
    with the placement where its wall clock may not, which is what `M178`'s
    sittings want beside the drift."""
    return {name: int(m) * 60 + float(s) for name, m, s in CPU_TIME_RE.findall(text)}

#: What `peak-rss` prints, on stderr, beside bash's own `real` line.
MAXRSS_RE = re.compile(r"^maxrss_kib=(\d+)$", re.MULTILINE)


def parse_maxrss_kib(text: str) -> int:
    """The peak resident set of the timed process, in KiB.

    Exactly one line per wrapped command, for the same reason `parse_bash_time`
    insists on one `real` line: two would mean the script wrapped two."""
    matches = MAXRSS_RE.findall(text)
    if not matches:
        raise ValueError(f"no `maxrss_kib` line in wrapped output:\n{text[-2000:]}")
    if len(matches) > 1:
        raise ValueError(
            f"{len(matches)} `maxrss_kib` lines in one run; the script wraps more than one command"
        )
    return int(matches[0])

#: What the harness appends to every in-container script, after the timed
#: command and outside every timer, so a run can be asked whether the kernel
#: killed it.
#:
#: **The exit code cannot answer this.** `peak-rss` exits `128 + n` for a
#: death by signal `n` and names it, but a `SIGKILL` does not say who sent it,
#: so a leg the OOM reaper killed reads as one anything else killed; and
#: `nerdctl inspect` cannot answer it either, because `--rm` has destroyed the
#: container by the time there is anything to ask. The container's own
#: `memory.events` counter can, and it is read inside the container while it
#: still exists.
#:
#: **It is not part of any command shape.** `_script` builds what is measured;
#: this is the harness asking the container what happened afterwards, which is
#: why it is appended here rather than written into thirty branches — and why a
#: figure's recorded shape is unchanged by it. Nothing it does is timed: the
#: `time` builtin and `peak-rss` both closed before `$__st` is read.
OOM_ORACLE = "; __st=$?; cat /sys/fs/cgroup/memory.events >&2 || true; exit $__st"

#: The `oom_kill` line of a cgroup v2 `memory.events`.
OOM_KILL_RE = re.compile(r"^oom_kill (\d+)$", re.MULTILINE)


def parse_oom_kills(text: str) -> int | None:
    """How many processes the kernel OOM-killed in this run's cgroup, or `None`
    where the counter was not readable.

    `None` and `0` are different answers and the caller must keep them apart: a
    missing counter means the harness **cannot tell** what killed the run, which
    is the state the whole oracle exists to leave behind, so it can never be
    read as "not killed"."""
    matches = OOM_KILL_RE.findall(text)
    if not matches:
        return None
    return max(int(m) for m in matches)

#: A `key=value` line an instrument writes where the harness can read it. Two
#: write them today: the decode example, on stdout, which has no other output;
#: and `pgdt --features introspect`, into the file `INSTRUMENT_OUT_VAR` names,
#: which is a channel of its own rather than a stream it shares.
REPORTED_RE = re.compile(r"^([a-z_]+)=(\S+)$")

#: The environment variable naming the file `crates/pgdt/src/introspect.rs` writes its
#: report to. **Unset means no report at all**, so a default build and an
#: unmeasured run of the instrument build behave identically.
#:
#: Shared in fact rather than in type: the two constants live in two languages
#: and `figures/test_reserve.py` holds them to each other. An environment variable
#: rather than a flag, so the instrumented leg's argv is the argv a figure
#: times — see `RunSpec.instrument`.
INSTRUMENT_OUT_VAR = "PGDT_INTROSPECT_OUT"

#: Where the report is mounted inside the container, and the directory under a
#: sitting's own output that is bind-mounted there. One report per rep lands
#: here and stays, beside `raw.json`: `live_peak_bytes`, the `mimalloc_*` and
#: the `mallinfo_*` fields are per-rep readings, and mimalloc's JSON and the
#: `malloc_info` XML under them are the detail no summary carries.
INSTRUMENT_MOUNT = "/introspect"

INSTRUMENT_DIR = "instrument"


def parse_reported(text: str) -> dict[str, str]:
    """The `key=value` lines a timed binary printed about its own run.

    A binary that reports what it did — how many bytes it decoded, how many
    workers its plan admitted, how many bytes it was holding — closes a gap a
    file size cannot: the harness would otherwise have to compute the plaintext
    volume behind a compressed input by a second mechanism, and the day the two
    disagreed the table would publish a rate over the wrong denominator.

    Lines that are not `key=value` are ignored rather than refused, so a binary
    is free to write whatever else it likes — which is what lets the
    introspection build carry mimalloc's JSON, `malloc_info`'s XML and its own
    scope note in the same file.

    The text is a stream's for the decode example and a **file's** for the
    introspection build (`INSTRUMENT_OUT_VAR`). The parse is the same either
    way; what the file buys is a channel with one writer, where stderr already
    carries `peak-rss`'s own per-rep `maxrss_kib=<n>` by this same grammar.
    """
    out: dict[str, str] = {}
    for line in text.splitlines():
        match = REPORTED_RE.match(line.strip())
        if match:
            out[match.group(1)] = match.group(2)
    return out

#: The worker count and byte budget a scan says it is running under, off its own
#: `scan started` line.
#:
#: **The library's line, not the CLI's provenance one.** Both carry the pair;
#: this one states it bare — `jobs=3 memory_bytes=204537856` — where the CLI's
#: wraps each number in the words that say where it came from, and what a
#: resident reading needs is the arrangement rather than its provenance. Both
#: `scan started` lines of a run print the same pair, so the first match stands.
SCAN_RESOLVED_RE = re.compile(r"\bscan started\b[^\n]*?\bjobs=(\d+) memory_bytes=(\d+)\b")


def parse_resolution(text: str) -> dict[str, str]:
    """What a run resolved for itself, read back off its own log.

    `{}` where the run printed no such line, which is every shape that is not a
    scan. **It is read for every shape rather than for the flagless family
    alone**: the pair is free to read, and a stated shape whose resolved pair is
    not the pair it stated is exactly the apparatus failure the worker-count
    reconciliation cannot see — `--jobs 24` inside a budget affording three
    readers is a row labelled 24 that ran three.

    It is the flagless legs that *need* it. Under discovery the count is
    `Discovered::resolve`'s answer to the allocation, so the reader count a
    resident set belongs to exists nowhere else — the harness cannot compute it
    without reimplementing the rule, which is the second authority
    `QUERY_SUBSTREAM_CAP` refuses by name.
    """
    match = SCAN_RESOLVED_RE.search(text)
    if not match:
        return {}
    return {"resolved_jobs": match.group(1), "resolved_budget": match.group(2)}

#: A gzip first pass's started line (`first_pass.rs`, `announce`), and the
#: worker count it names where its inflate runs ahead. A serial pass names
#: none, saying `reading serially` instead.
FIRST_PASS_STARTED_RE = re.compile(r"\bfirst pass started\b([^\n]*)")

FIRST_PASS_WORKERS_RE = re.compile(r"\bworkers=(\d+)\b")


def parse_first_pass(text: str) -> dict[str, str]:
    """How many workers a gzip first pass inflated on — the frontier and the
    tasks run ahead of it — read off its own started line: `workers=` where it
    names one, one where it reads serially.

    `{}` where the run opened no first pass, which is every source but a gzip
    file no complete index covers. **The resolved count is not this number**:
    `scan started` states what the arrangement afforded, and a pass runs no
    more tasks ahead than its file has chunks past the first, so a leg whose
    two disagree held fewer workers than it was billed for, and its table says
    so rather than reading the bill as what ran."""
    match = FIRST_PASS_STARTED_RE.search(text)
    if not match:
        return {}
    workers = FIRST_PASS_WORKERS_RE.search(match.group(1))
    return {"first_pass_workers": workers.group(1) if workers else "1"}

#: What `pgdt query` says on stderr about the rows it did not read, and how many
#: it returned: the pruning note (`PlanNoteKind::StatisticsPruned`), the early
#: stop's note, and the closing count.
PRUNING_NOTE_RE = re.compile(
    r"^note: row-group statistics rule out (\d+) of (\d+) group\(s\), so (\d+) of the "
    r"(\d+) byte\(s\) of rows",
    re.MULTILINE,
)

STOP_NOTE_RE = re.compile(
    r"^note: reading stopped early in (\d+) block\(s\) .*? a further (\d+) byte\(s\)",
    re.MULTILINE,
)

ROWS_RETURNED_RE = re.compile(r"^(\d+) row\(s\)$", re.MULTILINE)

#: What the CLI says in place of a count when no row passed.
NO_ROWS_RE = re.compile(r"^no rows found for ", re.MULTILINE)


def parse_query_notes(text: str) -> dict[str, str]:
    """What a `pgdt query` reported of its own reading: the groups and bytes
    its statistics skipped, the bytes an early stop left unread, and the rows
    it returned. `{}` for a run that printed none of them.

    **The process's own count, read beside the timing** — which is what makes
    a pruned leg that silently skipped nothing a refusal rather than a table
    saying pruning buys nothing (`run_statistics_pruning`)."""
    out: dict[str, str] = {}
    if match := PRUNING_NOTE_RE.search(text):
        keys = ("skipped_groups", "groups", "skipped_bytes", "bytes")
        out.update(zip(keys, match.groups()))
    if match := STOP_NOTE_RE.search(text):
        out["stopped_blocks"], out["unread_bytes"] = match.groups()
    if match := ROWS_RETURNED_RE.search(text):
        out["rows_returned"] = match.group(1)
    elif NO_ROWS_RE.search(text):
        out["rows_returned"] = "0"
    return out

#: Where `peak-rss` is mounted in every container: the resident-set
#: instrument, a workspace crate (`crates/peak-rss/src/main.rs` says what it reads and
#: why), built static by `ensure_peak_rss_binary` so it needs nothing of the
#: image. A shape that reads a resident set runs its command under it, with
#: anything the command's environment needs assigned in front of it.
PEAK_RSS = "/peak-rss"

#: Where `count-instructions` is mounted in a counted figure's container: the
#: instruction counter (`crates/count-instructions/src/main.rs`), built static
#: as `peak-rss` is, by `ensure_count_instructions_binary`. A figure declaring
#: `Figure.counts_instructions` runs every `pgdt` leg's timed command under it
#: (`counted_script`), so its count and its wall clock are one run's.
COUNT_INSTRUCTIONS = "/count-instructions"

#: The capabilities a counted leg's container is given: the runtime's default
#: seccomp profile refuses `perf_event_open` without `CAP_PERFMON`, and nothing
#: else. Only a counted figure's containers carry it, and it moves nothing a
#: `pgdt` run does.
COUNT_INSTRUCTIONS_CAPS: tuple[str, ...] = ("PERFMON",)

#: What `count-instructions` prints on stderr: the count, or in its place the
#: times of a count the kernel multiplexed, which is refused rather than scaled.
INSTRUCTIONS_RE = re.compile(r"^instructions_u=(\d+)$", re.MULTILINE)

MULTIPLEXED_RE = re.compile(r"^instructions_multiplexed=(\d+)/(\d+)$", re.MULTILINE)


def parse_instructions(text: str) -> int:
    """The user-space instructions the counted command retired.

    Exactly one count, for `parse_maxrss_kib`'s reason. A multiplexed count
    is refused: scaling it by the time it ran would publish an estimate under
    the name of a count, and the attribution it is read for compares counts
    within their own spread."""
    if multiplexed := MULTIPLEXED_RE.findall(text):
        running, enabled = multiplexed[0]
        raise ValueError(
            f"the instruction counter was multiplexed, running {running} ns of {enabled} ns: "
            "another user of the machine's counters shares them, and a scaled count is no count"
        )
    matches = INSTRUCTIONS_RE.findall(text)
    if not matches:
        raise ValueError(f"no `instructions_u` line in counted output:\n{text[-2000:]}")
    if len(matches) > 1:
        raise ValueError(
            f"{len(matches)} `instructions_u` lines in one run; the script counts more than one command"
        )
    return int(matches[0])

#: The timed `/pgdt` of a shape, behind the `time` builtin and any environment
#: assigned in front of it, which is where `counted_script` puts the counter.
_TIMED_PGDT_RE = re.compile(r"(?:^|(?<= ))time ((?:[A-Z_]+=\S+ )*)/pgdt ")


def counted_script(script: str) -> str:
    """A shape's script with its timed `/pgdt` run under `COUNT_INSTRUCTIONS`.

    Inside the timer and after any environment assigned in front of the
    program, which the counter's child inherits; an untimed builder ahead of
    it is not counted. A shape whose timed command is not one `/pgdt` is
    refused rather than counted around something else."""
    counted, n = _TIMED_PGDT_RE.subn(rf"time \1{COUNT_INSTRUCTIONS} /pgdt ", script)
    if n != 1:
        raise ValueError(f"{n} timed `/pgdt` commands in a counted shape:\n{script}")
    return counted


def criterion_median_ns(criterion_root: Path, full_id: str) -> float:
    """The median point estimate, in nanoseconds, out of criterion's own JSON.

    Read from estimates.json rather than scraped off the console: the console
    rounds, and a rounded ratio is how a table acquires a number nobody can
    reproduce. `benchmark.json` carries the `full_id`, so the mangled directory
    name (criterion replaces `/` with `_`) never has to be guessed."""
    for bench in sorted(criterion_root.glob("*/new/benchmark.json")):
        meta = json.loads(bench.read_text())
        if meta.get("full_id") == full_id:
            est = json.loads((bench.parent / "estimates.json").read_text())
            return float(est["median"]["point_estimate"])
    raise FileNotFoundError(f"no criterion benchmark {full_id!r} under {criterion_root}")


@dataclass(frozen=True)
class Estimate:
    """A criterion median and its bootstrapped 95% confidence interval, in
    nanoseconds."""

    median: float
    lower: float
    upper: float

    @property
    def half_width(self) -> float:
        return (self.upper - self.lower) / 2


def criterion_estimate(criterion_root: Path, full_id: str) -> Estimate:
    """`criterion_median_ns`'s median with the interval criterion states
    beside it, out of the same `estimates.json`."""
    for bench in sorted(criterion_root.glob("*/new/benchmark.json")):
        meta = json.loads(bench.read_text())
        if meta.get("full_id") == full_id:
            median = json.loads((bench.parent / "estimates.json").read_text())["median"]
            interval = median["confidence_interval"]
            return Estimate(
                float(median["point_estimate"]),
                float(interval["lower_bound"]),
                float(interval["upper_bound"]),
            )
    raise FileNotFoundError(f"no criterion benchmark {full_id!r} under {criterion_root}")
