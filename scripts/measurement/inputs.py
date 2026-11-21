"""The register's inputs: each generated file, the generator and arguments that make it, and the
stamp that says when it must be regenerated."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

import generate_long_lines as long_lines

from measurement import config

if TYPE_CHECKING:
    from measurement.config import Config


@dataclass(frozen=True)
class InputSpec:
    name: str
    generator: str
    args: tuple[str, ...]
    #: Scale `--size-mb`/`--size-gb` with Config.size_gib. The block-count
    #: inputs do not scale -- their size *is* the block count.
    scales: bool = True
    #: What the staged file is called. `.sql` for a plain dump; `.xz` for a
    #: compressed one, which is a *different kind of input* rather than a plain
    #: one under another name -- `pgdt` recognises a source by content, but the
    #: harness's own staging, eviction and stamping all key on the file name,
    #: and two inputs whose names collided would evict each other silently.
    suffix: str = ".sql"
    #: The input this one is built from, staged first and passed to the
    #: generator as `@SOURCE@`. The `.xz` legs are compressions of inputs the
    #: register already generates, and re-generating the plaintext separately
    #: would put two files with different bytes behind one figure's rows.
    #: Its stamp folds into this one's, so a change to the *source's* generator
    #: regenerates both.
    derives_from: str | None = None
    #: What this input weighs when it has never been generated, where that is
    #: not `size_gib`. A compressed input is a fraction of its plaintext, and a
    #: dry run that guessed 3.00 GiB for it would size the tmpfs budget against
    #: a file five times larger than the one it stages.
    nominal_bytes: int | None = None

    def argv(self, cfg: Config, out: Path) -> list[str]:
        args = list(self.args)
        if self.scales:
            args = [
                f"{cfg.size_gib * 1024:g}" if a == "@SIZE_MB@" else a for a in args
            ]
            args = [f"{cfg.size_gib:g}" if a == "@SIZE_GB@" else a for a in args]
        if self.derives_from:
            source = str(cfg.cache_dir / input_file(self.derives_from))
            args = [source if a == "@SOURCE@" else a for a in args]
        return ["uv", "run", self.generator, *args, str(out)]


def input_file(name: str) -> str:
    """What an input is called wherever it is staged.

    One function rather than an `f"{name}.sql"` at each of the dozen staging,
    eviction and preflight sites: the moment one input stopped being a plain
    dump, every one of those was a place the `.xz` leg could be staged under a
    name nothing else looked for."""
    return name + INPUTS[name].suffix


def _perf(name: str, *flags: str) -> InputSpec:
    return InputSpec(name, "generate_perf_data.py", (*flags, "--size-mb", "@SIZE_MB@"))

INPUTS: dict[str, InputSpec] = {
    # The brace-free control. Its lack of `{`/`[` is a contract, not an
    # accident: the scan-throughput figures are taken on it.
    "control": _perf("control", "--seed", "42"),
    # The instrument's own floor: identical shape, different seed.
    "control43": _perf("control43", "--seed", "43"),
    "composite": _perf("composite", "--composite", "--seed", "42"),
    "arrays": _perf("arrays", "--arrays", "--composite", "--seed", "42"),
    "large_object": InputSpec(
        "large_object",
        "generate_large_object_bench.py",
        ("--size-gb", "@SIZE_GB@", "--seed", "42"),
    ),
    "insert_run": InputSpec(
        "insert_run",
        "generate_insert_run_bench.py",
        ("--size-gb", "@SIZE_GB@", "--seed", "42"),
    ),
    # The quadratic's control: the same byte count in one COPY block.
    "one_block": InputSpec(
        "one_block", "generate_perf_data.py", ("--seed", "42", "--size-mb", "2"), scales=False
    ),
    # `statistics-pruning`'s input: the control's rows, same seed and draws,
    # with a low-cardinality column appended (`generate_pruning_bench.py`).
    "pruning": InputSpec(
        "pruning", "generate_pruning_bench.py", ("--seed", "42", "--size-mb", "@SIZE_MB@")
    ),
    # The dynamic-filter figures' input: the control's rows, same seed and
    # draws, with two join keys appended, and three small build tables
    # (`generate_dynamic_filter_bench.py`).
    "dynfilter": InputSpec(
        "dynfilter",
        "generate_dynamic_filter_bench.py",
        ("--seed", "42", "--size-mb", "@SIZE_MB@"),
    ),
}

for _n in (500, 1000, 2000, 4000):
    INPUTS[f"blocks{_n}"] = InputSpec(
        f"blocks{_n}",
        "generate_block_count_bench.py",
        ("--blocks", str(_n), "--out"),
        scales=False,
    )

# The two `.xz` legs of `xz-decode-scaling`, both at koji's own container
# parameters -- preset 6, so an 8 MiB LZMA2 dictionary, 24 MiB blocks, CRC64 --
# and both about 3.00 GiB of *plaintext*, which is the register's input size and
# the quantity a decode rate is a rate of.
#
# Two of them because the two answer the question differently and neither alone
# is honest. The generated leg is reproducible from committed sources on a
# machine that has never seen koji, which is what makes the figure re-takeable;
# the koji leg is real data at a real compression ratio, which is what the
# per-worker charge's arithmetic is written against.
INPUTS["control_xz"] = InputSpec(
    "control_xz",
    "generate_xz_input.py",
    ("--from-dump", "@SOURCE@"),
    scales=False,
    suffix=".xz",
    derives_from="control",
    # ~5.4x on the control's rows; a generous nominal, since over-reserving
    # tmpfs is recoverable and under-reserving is a figure lost mid-sweep.
    nominal_bytes=700 * config.MIB,
)

#: The same plaintext as `control_xz`, written in 128 MiB blocks instead of
#: koji's 24 MiB — the second block size `parallel-peak-rss` is taken at.
#:
#: **Two block sizes because a resident set at one of them is a number about the
#: file, not about the library.** A compressed reader's per-worker footprint is
#: one decoded block, so the same stated budget admits a different number of
#: concurrent readers at each size, and a figure taken at one would publish that
#: file's shape as the library's bound.
#:
#: 128 MiB is the size the locally recompressed koji copy has and what
#: `xz --block-size=128MiB` writes (`CLAUDE.local.md`), so it is a shape a user
#: reaches with one flag rather than a number chosen to make a point. It is also
#: the size the shipped 64 MiB budget declines outright: at `--jobs 1` this leg
#: reads through the streaming fallback, and every row above it block-decodes —
#: which is a discontinuity between the first two rows and is what the table
#: says about them.
#:
#: It derives from `control` exactly as `control_xz` does, so both compressed
#: legs are compressions of the same bytes and a change to the perf generator
#: regenerates all three.
INPUTS["control_xz128"] = InputSpec(
    "control_xz128",
    "generate_xz_input.py",
    ("--from-dump", "@SOURCE@", "--block-size", "128MiB"),
    scales=False,
    suffix=".xz",
    derives_from="control",
    # Larger blocks compress a shade better than smaller ones; the same
    # generous nominal `control_xz` carries, for the same reason.
    nominal_bytes=700 * config.MIB,
)

#: The `inflate-throughput` figure's compressed leg: `control` deflated into
#: the one gzip member `pg_dump -Fp -Z` writes (`generate_gzip_input.py`).
#:
#: **A compression of `control`, as the `.xz` legs are**, so the plaintext the
#: figure's rates are per is `control`'s own size, and a change to the perf
#: generator regenerates this leg with the file it is set against.
INPUTS["control_gz"] = InputSpec(
    "control_gz",
    "generate_gzip_input.py",
    ("--from-dump", "@SOURCE@"),
    scales=False,
    suffix=".gz",
    derives_from="control",
    # gzip at level 6 packs text a shade under half as tightly as xz at
    # preset 6; a generous nominal, for `control_xz`'s reason.
    nominal_bytes=1024 * config.MIB,
)

#: `gzip-resident`'s block-path leg: `control` written as BGZF
#: (`generate_gzip_input.py --bgzf`), a member a `BGZF_BLOCK` of plaintext.
#:
#: **The gzip file a `parse` block-decodes from its first byte.** A local BGZF
#: file is walked at open, so its index is complete before the scan and every
#: reader decodes whole units through the block cache, as `control_xz`'s do;
#: a single member's first `parse` is its first pass instead. A compression of
#: `control`, so the plaintext is the other legs' exactly.
INPUTS["control_bgzf"] = InputSpec(
    "control_bgzf",
    "generate_gzip_input.py",
    ("--from-dump", "@SOURCE@", "--bgzf"),
    scales=False,
    suffix=".gz",
    derives_from="control",
    # Each member deflated with no history packs a little looser than one
    # member; the single member's generous nominal holds it.
    nominal_bytes=1024 * config.MIB,
)

#: `gzip-first-pass`'s other writers' files: `control` written by another
#: writer's deflate into one member (`generate_gzip_input.py --writer`), so
#: every leg's plaintext is `control_gz`'s exactly and the files differ by
#: their blocks alone (`GZPASS_WRITERS`).
#:
#: **Fedora's `zlib-ng-compat` at level 6** is `pg_dump -Z` on Fedora, whose
#: zlib it is; **GNU gzip at level 6** is `pg_dump | gzip`; **zlib-ng at level
#: 1** is the writer that ends no block before its last (`deflate_quick`),
#: whose first pass the frontier carries alone. Each a generous nominal, level
#: 1 packing looser.
INPUTS["control_gz_ng"] = InputSpec(
    "control_gz_ng",
    "generate_gzip_input.py",
    ("--from-dump", "@SOURCE@", "--writer", "zlib-ng-compat"),
    scales=False,
    suffix=".gz",
    derives_from="control",
    nominal_bytes=1024 * config.MIB,
)

INPUTS["control_gz_gnu"] = InputSpec(
    "control_gz_gnu",
    "generate_gzip_input.py",
    ("--from-dump", "@SOURCE@", "--writer", "gzip"),
    scales=False,
    suffix=".gz",
    derives_from="control",
    nominal_bytes=1024 * config.MIB,
)

INPUTS["control_gz_quick"] = InputSpec(
    "control_gz_quick",
    "generate_gzip_input.py",
    ("--from-dump", "@SOURCE@", "--writer", "zlib-ng", "--level", "1"),
    scales=False,
    suffix=".gz",
    derives_from="control",
    nominal_bytes=1536 * config.MIB,
)

#: `gzip-resident`'s limit leg: `control`'s rows repeated in runs and deflated
#: into one member as `control_gz` is (`generate_repeated_rows.py`), so it
#: packs past the speculation limit's ratio and every chunk's plaintext reaches
#: the limit (`gzip_limit_ratio_problem`). **Not a compression of `control`**:
#: its plaintext is another file of `control`'s length, so it is no
#: `gzip-first-pass` writer, whose legs differ by their blocks alone. Its
#: nominal is a little over what it weighs, and no more: a dry run reads the
#: limit leg's ratio off the nominals, so one past `control`'s over
#: `GZIP_LIMIT_ROOM` times the limit's ratio would refuse the figure there.
INPUTS["repeated_gz"] = InputSpec(
    "repeated_gz",
    "generate_repeated_rows.py",
    ("--from-dump", "@SOURCE@"),
    scales=False,
    suffix=".gz",
    derives_from="control",
    nominal_bytes=144 * config.MIB,
)

#: The koji download, and where in it the koji leg's streams are taken from.
#: **Not the head of the file**: koji's first 3 GiB of plaintext compresses
#: 56.19x against the whole file's 19.41x, and a decode rate is per plaintext
#: byte, so a slice cut there would report a rate for bytes unlike the rest of
#: the dump. Seeking past it costs one seek.
#:
#: **This offset is a sample, not a representative, and nothing pretends
#: otherwise.** koji sampled at twelve depths runs from 5.02x to 33.05x, so no
#: single offset stands for the file; 20 GB yields 15.70x, a little denser than
#: the middle. What that costs is confined to the absolute rates -- the scaling
#: curve is a within-leg ratio and barely moves -- and it is why the ratio is
#: gated rather than assumed (`docs/design/decisions.md`, "The compressed source and the cache").
KOJI_XZ = config._env(
    "PGDT_KOJI_XZ", "/mnt/wd12t/fedora/koji/koji-2026-07-23.dump.multistream.xz"
)

KOJI_XZ_OFFSET = int(config._env("PGDT_KOJI_XZ_OFFSET", str(20_000_000_000)))

#: How many whole streams the koji leg keeps. The download is one 24 MiB block
#: per stream, so 128 of them is 3.00 GiB of plaintext -- the register's input
#: size, and enough blocks that 24 workers are never clamped by the work.
KOJI_XZ_STREAMS = 128

INPUTS["koji_xz"] = InputSpec(
    "koji_xz",
    "generate_xz_input.py",
    (
        "--from-koji",
        KOJI_XZ,
        "--streams",
        str(KOJI_XZ_STREAMS),
        "--from-offset",
        str(KOJI_XZ_OFFSET),
    ),
    scales=False,
    suffix=".xz",
    nominal_bytes=250 * config.MIB,
)

#: The figure's longest line: I71's bound, `MaxAllocSize - 1`, terminator
#: included (`docs/design/postgres-invariants.md`), which every pass reads to
#: and no leg states (`pgdt_core::LINE_BOUND`).
LONG_LINE_BOUND = 0x3FFF_FFFE

#: `long-line-resident`'s line lengths, each the token its input is named by
#: and its bytes, terminator included, in the table's row order:
#:
#: - **the ladder**, 32, 128 and 512 MiB, a factor of four apart, its secant
#:   over the two longest (`LONG_LINE_SECANT`), the longest past the reference
#:   dump's longest line (`runs/datanommer-survey/report.md`, machine-local);
#: - **288 MiB, off the ladder**, read at both counts: every ladder length is a
#:   power of two, and an owner doubling its read sees each at one phase of
#:   its growth, so only a length just past a doubling can show a peak that
#:   follows the growth rather than the line;
#: - **the bound itself**, read at one worker alone (`LONG_LINE_SERIAL_ONLY`):
#:   what one reader holds at the bound is the floor's question
#:   (`docs/design/decisions.md`, "D161"), the one cell the
#:   ladder's secant would otherwise extrapolate to.
LONG_LINE_LENGTHS: tuple[tuple[str, int], ...] = (
    ("32", 32 * config.MIB),
    ("128", 128 * config.MIB),
    ("288", 288 * config.MIB),
    ("512", 512 * config.MIB),
    ("bound", LONG_LINE_BOUND),
)

#: The two lengths each pass's secant is taken between (`long_line_secant`);
#: every other length is a residual against it (`long_line_residual`).
LONG_LINE_SECANT: tuple[str, str] = ("128", "512")

#: The lengths read at one worker or partition alone. **Four workers each
#: holding what one holds at the bound** (`docs/design/measurements.md`, "What
#: a held line holds resident") would need, on the widest pass, more than
#: `LONG_LINE_MEMORY`, the figure's one ceiling, which `memory_preflight` holds
#: under `contained.MAX_FRACTION` of the machine; one fits.
LONG_LINE_SERIAL_ONLY: tuple[str, ...] = ("bound",)

#: The long lines in each input's cluster: the four-worker leg's count, so each
#: worker can hold one at once (`LONG_LINE_JOBS`). The same at every length,
#: so two inputs of a kind differ in their lines' length alone.
LONG_LINE_CLUSTER = 4

#: The kinds read for **a term of a query's factor no count of Arrow's buffers
#: sees** (`docs/design/decisions.md`, "D154"): a fixed-width
#: array's form, where a builder's growth shows; the stage a `text[]` part's
#: token and unquoted copy hold; and a `jsonb` ordering key. Each is read by
#: the one pass holding that term (`long_line_passes`), at one partition alone
#: (`long_line_jobs`): **what one line holds is the term's question**, four
#: partitions answering only how many lines the cut holds at once, which the
#: other kinds' columns read. Each reads the bound beside its ladder, as every
#: kind does.
LONG_LINE_TERM_KINDS: tuple[str, ...] = ("int8array", "textarray", "jsonb")


def long_line_lengths(kind: str) -> tuple[str, ...]:
    """The tokens of `LONG_LINE_LENGTHS` `kind`'s lines are generated at:
    every one, the same for every kind."""
    if kind not in long_lines.KINDS:
        raise ValueError(f"{kind!r} is no value kind of long-line-resident")
    return tuple(length for length, _ in LONG_LINE_LENGTHS)


def long_line_bytes(length: str) -> int:
    """The bytes of a line of `LONG_LINE_LENGTHS`' token `length`."""
    return dict(LONG_LINE_LENGTHS)[length]


def long_line_input(kind: str, length: str) -> str:
    """The input holding `long_lines`' cluster of `kind` lines of `length`, a
    token of `long_line_lengths(kind)`."""
    if length not in long_line_lengths(kind):
        raise ValueError(f"{length!r} is no line length of long-line-resident's {kind}")
    return f"long_{kind}_{length}"

#: Every `long-line-resident` input, by kind then length.
LONG_LINE_INPUTS: tuple[str, ...] = tuple(
    long_line_input(kind, length) for kind in long_lines.KINDS for length in long_line_lengths(kind)
)

#: One input a value kind and a line length (`generate_long_lines.py`), each
#: a cluster of `LONG_LINE_CLUSTER` lines between short rows. Its nominal is its
#: cluster and a mebibyte for the rest, which is a few hundred bytes.
for _kind in long_lines.KINDS:
    for _length, _bytes in LONG_LINE_LENGTHS:
        if _length not in long_line_lengths(_kind):
            continue
        INPUTS[long_line_input(_kind, _length)] = InputSpec(
            long_line_input(_kind, _length),
            "generate_long_lines.py",
            ("--kind", _kind, "--line-bytes", str(_bytes), "--lines", str(LONG_LINE_CLUSTER)),
            scales=False,
            nominal_bytes=LONG_LINE_CLUSTER * _bytes + config.MIB,
        )


def input_stamp(spec: InputSpec, cfg: Config) -> str:
    """A hash of the generator's source and the arguments it was given.

    The whole-file bench's lesson, applied to the harness's own inputs: a
    generated file regenerated only when *missing* means a checkout that
    already has one benchmarks pre-change bytes forever, silently -- and the
    population that has one is exactly the population comparing a new number to
    an old one.

    **A derived input folds its source's stamp in.** `control_xz` is a
    compression of `control`, so a change to the *perf* generator moves the
    bytes behind it exactly as it moves the bytes behind every other figure --
    and hashing only this generator would leave the compressed leg measuring
    pre-change rows for as long as the file sat there.
    """
    h = hashlib.sha256()
    h.update((config.SCRIPTS / spec.generator).read_bytes())
    h.update(repr(spec.argv(cfg, Path("OUT"))).encode())
    if spec.derives_from:
        h.update(input_stamp(INPUTS[spec.derives_from], cfg).encode())
    return h.hexdigest()


# -- what a held line holds resident ----------------------------------------


def long_line_answer(name: str) -> int:
    """What `LONG_LINE_SQL` and `LONG_LINE_FILTER` answer over the input
    `name`: every row of its table (`generate_long_lines.row_count`)."""
    if name not in LONG_LINE_INPUTS:
        raise ValueError(f"{name!r} is no input of long-line-resident")
    return long_lines.row_count(LONG_LINE_CLUSTER)
