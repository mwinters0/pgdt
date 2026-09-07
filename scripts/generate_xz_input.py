#!/usr/bin/env python3
"""The two `.xz` inputs the `xz-decode-scaling` figure is taken on.

Both are read by `pgdump_query/examples/xz_decode.rs` under
`scripts/measure.py`, and both exist to answer one question — how a block
decoder's plaintext throughput scales with worker count — over the two kinds of
bytes that answer it differently:

* **`--from-dump`** compresses an input `scripts/measure.py` already generates
  (the brace-free `control`, today) at koji's own container parameters:
  preset 6, so an 8 MiB LZMA2 dictionary, 24 MiB blocks, CRC64. It is
  reproducible on any machine from committed sources, which is what makes the
  figure re-takeable by someone who does not have koji.

* **`--from-koji`** copies a **byte-exact prefix** of the upstream koji
  download, cut at a stream boundary. Real data at a real compression ratio,
  in the container shape the phase's arithmetic is written against — and, being
  a prefix of whole streams, still a valid `.xz` file that decodes to exactly
  what those streams held.

Why a prefix rather than the whole file: the download is 40,397,009,888 bytes
of 31,150 concatenated streams, and building its seek table means walking
31,150 footers — 85 s on the HDD, before any decode
(`docs/design/architecture.md`, "The compressed source"). A figure whose every
reading paid that would be a figure about the walk. The prefix is also small
enough to stage on tmpfs, which is what puts both legs in the same regime: the
HDD is deliberately not one (`docs/design/measurements.md`, "Scan throughput by
input shape").

**The cut is found by scanning forward, not by `xz --list`.** A stream ends
with the two-byte footer magic `YZ` and the next begins with the six-byte
header magic, so a boundary is the one adjacent to the other; `xz --list` on
this file walks every stream from the end and takes 85 s to say the same thing
about the first hundred. Stream padding, if a file had any, is a whole number
of four-byte zero groups between the two, so the scan steps back over zeros
before it checks.

Generated, never committed. `scripts/measure.py` owns the paths and the sizes;
this script owns the bytes.

    cd scripts
    uv run generate_xz_input.py --from-dump control.sql out.xz
    uv run generate_xz_input.py --from-koji /path/koji.multistream.xz \\
        --streams 128 out.xz
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

#: koji's own container parameters, which are what the phase's per-worker
#: footprint arithmetic is written against: preset 6 (an 8 MiB LZMA2
#: dictionary), 24 MiB blocks, CRC64. A generated leg that used anything else
#: would be a different file shape wearing the same table row.
PRESET = "6"
BLOCK_SIZE = "24MiB"
CHECK = "crc64"

#: The magic an `.xz` stream opens with, and the two bytes its footer closes
#: with. A boundary between two concatenated streams is the second followed by
#: the first, with optional four-byte-aligned zero padding in between.
STREAM_HEADER_MAGIC = b"\xfd7zXZ\x00"
STREAM_FOOTER_MAGIC = b"\x59\x5a"

#: How much of the koji file is read at a time while scanning for boundaries.
#: Large enough that the HDD is read sequentially rather than in seeks, small
#: enough that the scan holds nothing interesting.
SCAN_CHUNK = 32 * 1024 * 1024


def compress(source: Path, out: Path, threads: int) -> None:
    """`source` compressed into `out` at koji's container parameters.

    Through the `xz` binary rather than through Python's `lzma`, because the
    block size is the whole point and `lzma` exposes no way to set one: a
    single-block file admits exactly one worker, which would make every row of
    this figure's table the same reading.
    """
    if shutil.which("xz") is None:
        raise SystemExit("xz is not on PATH, and the block size cannot be set without it")
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_name(out.name + ".partial")
    with source.open("rb") as src, tmp.open("wb") as dst:
        subprocess.run(
            [
                "xz",
                f"-{PRESET}",
                f"-T{threads}",
                f"--block-size={BLOCK_SIZE}",
                f"--check={CHECK}",
                "-c",
            ],
            stdin=src,
            stdout=dst,
            check=True,
        )
    tmp.replace(out)


def stream_boundaries(fh, wanted: int, base: int = 0) -> list[int]:
    """The offsets of the first `wanted` stream starts after the file position
    `fh` is already at, which the caller states as `base`.

    Scanning forward, with an overlap between chunks so a boundary straddling
    one is still seen. Returns fewer than `wanted` only where the file ran out.
    A match at `base` itself is never a boundary: nothing precedes it in what
    was read, so there is no footer to check it against.
    """
    pattern = re.compile(re.escape(STREAM_HEADER_MAGIC))
    # Enough that a boundary straddling two chunks is seen whole in the
    # second, with its footer bytes and any padding in front of it.
    overlap = 64
    found: list[int] = []
    origin = base
    tail = b""
    while len(found) < wanted:
        chunk = fh.read(SCAN_CHUNK)
        if not chunk:
            break
        buf = tail + chunk
        start = base - len(tail)  # the source offset `buf[0]` sits at
        for match in pattern.finditer(buf):
            here = start + match.start()
            # The magic the scan started on is not a boundary, and a match in
            # the overlap was already judged with full context last time.
            if here == origin or (found and here <= found[-1]) or match.start() < 2:
                continue
            # Step back over stream padding, which is a whole number of
            # four-byte zero groups, then require the footer magic.
            back = match.start()
            while back >= 4 and buf[back - 4 : back] == b"\0\0\0\0":
                back -= 4
            if back >= 2 and buf[back - 2 : back] == STREAM_FOOTER_MAGIC:
                found.append(here)
                if len(found) == wanted:
                    break
        base += len(chunk)
        tail = buf[-overlap:] if len(buf) >= overlap else buf
    return found


def koji_slice(source: Path, out: Path, streams: int, offset: int) -> tuple[int, int]:
    """Copy `streams` whole streams of `source` into `out`, starting at the
    first stream boundary at or after `offset`.

    Any contiguous run of whole streams is itself a valid `.xz` file — that is
    what concatenation means here — so a slice out of the middle decodes to
    exactly the plaintext those streams held and needs nothing repaired.

    `offset` exists because the head of a dump is not representative of it:
    koji's first 3 GiB of plaintext compresses 56.19×, against the whole file's
    19.41×, and a decode rate is a rate per plaintext byte. Seeking past it
    costs one seek rather than a read of everything skipped.

    No offset is representative either — koji sampled at twelve depths runs
    from 5.02× to 33.05× — so the slice is one draw from a wide distribution
    and the rate it produces is quoted with its density rather than as the
    file's (`roadmap-P16.1-xz-decode-scaling-notes.md`).

    Returns the slice's `(start, end)` offsets in the source.
    """
    if streams < 1:
        raise SystemExit("--streams must be at least 1")
    with source.open("rb") as fh:
        if offset:
            fh.seek(offset)
            first = stream_boundaries(fh, 1, base=offset)
            if not first:
                raise SystemExit(f"{source}: no stream boundary at or after offset {offset}")
            start = first[0]
        else:
            start = 0
        fh.seek(start)
        # From `start`, `streams` whole streams end at the boundary that opens
        # the one after them.
        boundaries = stream_boundaries(fh, streams, base=start)
        if len(boundaries) < streams:
            raise SystemExit(
                f"{source} holds only {len(boundaries)} stream boundaries after offset {start}, "
                f"short of the {streams} asked for — is it a single-stream file?"
            )
        end = boundaries[-1]
        fh.seek(start)
        out.parent.mkdir(parents=True, exist_ok=True)
        tmp = out.with_name(out.name + ".partial")
        remaining = end - start
        with tmp.open("wb") as dst:
            while remaining:
                chunk = fh.read(min(SCAN_CHUNK, remaining))
                if not chunk:
                    raise SystemExit(f"{source} ended before offset {end}")
                dst.write(chunk)
                remaining -= len(chunk)
        tmp.replace(out)
    return start, end


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out", type=Path, help="the .xz file to write")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument(
        "--from-dump", type=Path, help="a plain dump to compress at koji's container parameters"
    )
    group.add_argument(
        "--from-koji", type=Path, help="a multistream .xz to take a stream-aligned prefix of"
    )
    parser.add_argument(
        "--streams",
        type=int,
        default=128,
        help="with --from-koji: how many whole streams to keep (default 128, which is 3.00 GiB "
        "of plaintext at the download's 24 MiB streams)",
    )
    parser.add_argument(
        "--from-offset",
        type=int,
        default=0,
        help="with --from-koji: seek here first and take the slice from the next stream "
        "boundary, so the bytes are not the head of the dump (default 0)",
    )
    parser.add_argument(
        "--threads",
        type=int,
        default=0,
        help="with --from-dump: xz worker threads (default 0, meaning as many as there are cores)",
    )
    args = parser.parse_args()

    if args.from_dump is not None:
        if not args.from_dump.exists():
            raise SystemExit(f"{args.from_dump} does not exist")
        compress(args.from_dump, args.out, args.threads)
    else:
        if not args.from_koji.exists():
            raise SystemExit(
                f"{args.from_koji} does not exist — this leg reads the koji download named in "
                "CLAUDE.local.md, and PGDQ_KOJI_XZ is what points measure.py elsewhere"
            )
        start, end = koji_slice(args.from_koji, args.out, args.streams, args.from_offset)
        print(f"streams {args.streams} from {start} to {end}", file=sys.stderr)

    size = args.out.stat().st_size
    print(f"wrote {args.out} ({size / (1024 * 1024):.1f} MiB)", file=sys.stderr)


if __name__ == "__main__":
    main()
