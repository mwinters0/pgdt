#!/usr/bin/env python3
"""The single gzip member `gzip-resident`'s limit leg reads: an input
`scripts/measure.py` already generates, each row it keeps written `REPEAT`
times in a run, deflated as `pg_dump -Z` writes it.

**What it is for is a gzip member whose chunks reach the speculation limit.**
A first pass's task is charged `gz_seek::SPECULATION_LIMIT` of plaintext six
times over, and those terms fill only where a chunk of `INPUT_CHUNK`
compressed bytes inflates to the limit or past it, about 16 : 1. `control`
deflates about 4 : 1, so no leg reading it fills them, and no reading over it
can say whether the charge is a bound (`docs/design/roadmap-P15-gzip.md`,
"Slice order"). A row repeated in a run is coded as matches into the copy
before it, which deflate's window holds, `control`'s rows being far shorter
than the window, so the member packs several times tighter; `REPEAT` is chosen
so it does with room over the limit's ratio, which `measure.run_gzip_resident`
holds the generated file to before it reads a leg (`gzip_limit_ratio_problem`).

**The plaintext stays the source's size, block by block.** A row is kept only
while the block written so far is no longer than the block read so far, so
the rows kept are about every `REPEAT`-th, each written `REPEAT` times, and a
block ends within one run of its source's length. Everything outside a `COPY`
block is copied unchanged. So a first pass over the result scans as much
plaintext as one over the source, and the leg differs from `control_gz`'s by
how tightly its rows pack, not by how much there is to read.

**Duplicate rows are rows `pg_dump` writes**: a table with no key holds them,
and nothing in a plain dump's grammar refuses them. No claim is made that a
real table repeats its rows in runs of this length; a run is the shape that
reaches the limit without leaving `control`'s values.

**Deflated as `generate_gzip_input.py` deflates `control_gz`**: one member,
zlib's deflate at `pg_dump`'s level, behind `pg_dump`'s header, through the
zlib that script names and checks (`generate_gzip_input.compress`'s writer).
The repeated plaintext is streamed into it and never written, so the two
`.gz` legs differ by their plaintext alone.

Generated, never committed. `scripts/measure.py` owns the paths; this script
owns the bytes.

    cd scripts
    uv run generate_repeated_rows.py --from-dump control.sql out.gz
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable, Iterator
from pathlib import Path

import generate_gzip_input as gz

#: How many times each kept row is written: enough that `control`'s rows pack
#: well past the speculation limit's ratio at `pg_dump`'s level 6, so every
#: chunk's plaintext reaches the limit and not only the file's mean, which
#: `measure.gzip_limit_ratio_problem` asks of the generated file. Moving it
#: moves the bytes, which `measure.input_stamp` hashes.
REPEAT = 8

#: What opens a `COPY` block's data, and what ends it.
COPY_OPEN = b"COPY "
COPY_OPEN_END = b" FROM stdin;\n"
COPY_END = b"\\.\n"


def repeated_lines(lines: Iterable[bytes], repeat: int = REPEAT) -> Iterator[bytes]:
    """`lines`, each `COPY` block's rows repeated in runs of `repeat` and
    thinned so the block keeps its source's length, everything else as it
    is."""
    if repeat < 1:
        raise ValueError(f"a run of {repeat} rows writes nothing")
    in_block = False
    read = written = 0
    for line in lines:
        if not in_block:
            yield line
            if line.startswith(COPY_OPEN) and line.endswith(COPY_OPEN_END):
                in_block, read, written = True, 0, 0
            continue
        if line == COPY_END:
            yield line
            in_block = False
            continue
        read += len(line)
        if written < read:
            yield line * repeat
            written += len(line) * repeat
    if in_block:
        raise SystemExit("a COPY block never ends")


def write_member(source: Path, out: Path, repeat: int = REPEAT) -> int:
    """`source`'s rows repeated (`repeated_lines`) and deflated into one gzip
    member at `out`, beside it first and moved into place once whole and its
    header checked; the plaintext's length."""
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_name(out.name + ".partial")
    deflater = gz.deflater_for("zlib", gz.LEVEL)
    plaintext = 0
    with source.open("rb") as src, tmp.open("wb") as dst:
        pending: list[bytes] = []
        held = 0
        for piece in repeated_lines(src, repeat):
            pending.append(piece)
            held += len(piece)
            if held >= gz.CHUNK:
                dst.write(deflater.compress(b"".join(pending)))
                plaintext += held
                pending, held = [], 0
        dst.write(deflater.compress(b"".join(pending)))
        dst.write(deflater.flush())
        plaintext += held
    with tmp.open("rb") as written:
        gz.check_header(written.read(len(gz.PG_DUMP_HEADER)))
    tmp.replace(out)
    return plaintext


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--from-dump",
        type=Path,
        required=True,
        metavar="SOURCE",
        help="the plain dump whose rows are repeated, one of the inputs measure.py generates",
    )
    parser.add_argument("out", type=Path, help="where the .gz file goes")
    args = parser.parse_args(argv)
    plain = write_member(args.from_dump, args.out)
    packed = args.out.stat().st_size
    print(f"{args.out}: {packed} bytes, {plain} of plaintext, {plain / packed:.2f}x")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
