#!/usr/bin/env python3
"""The `.xz` inputs the register's compressed figures are taken on.

Two of them are `xz-decode-scaling`'s, read by
`pgdump_query/examples/xz_decode.rs` under `scripts/measure.py`, and both exist
to answer one question — how a block decoder's plaintext throughput scales with
worker count — over the two kinds of bytes that answer it differently:

* **`--from-dump`** compresses an input `scripts/measure.py` already generates
  (the brace-free `control`, today) at koji's own container parameters:
  preset 6, so an 8 MiB LZMA2 dictionary, 24 MiB blocks, CRC64. It is
  reproducible on any machine from committed sources, which is what makes the
  figure re-takeable by someone who does not have koji. `--block-size` moves
  the one parameter of those that a figure is taken *over* — a compressed
  reader's per-worker footprint is one decoded block — and moves it only within
  `BLOCK_SIZES`.

* **`--from-koji`** copies a **byte-exact prefix** of the upstream koji
  download, cut at a stream boundary. Real data at a real compression ratio,
  in the container shape the phase's arithmetic is written against — and, being
  a prefix of whole streams, still a valid `.xz` file that decodes to exactly
  what those streams held.

Why a prefix rather than the whole file: the download is 40,397,009,888 bytes
of 31,150 concatenated streams, and building its seek table means walking
31,150 footers — 85 s on the HDD, before any decode
(`docs/design/decisions.md`, "The compressed source and the cache"). A figure whose every
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

**The koji slice's density is gated, not remembered.** A decode rate is a rate
per plaintext byte, so it is a property of the bytes, and koji's own regions
differ from one another by 6.6x — so the slice's compression ratio is divided
out of the file that was just written and refused unless it is in the band
`KOJI_RATIO_MIN`/`KOJI_RATIO_MAX`. Both legs report the ratio whether they gate
on it or not, since it is what the figure's rates are quoted with.

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

#: The block sizes `--block-size` will accept, and the only ones any figure
#: asks for.
#:
#: **An allowlist rather than a pass-through to `xz`.** The block size is the
#: quantity a compressed figure's per-worker footprint *is*, so a typo'd or
#: casually chosen value produces a perfectly valid `.xz` file whose table row
#: describes a shape nobody registered — the same failure the koji density gate
#: refuses one column over. `24MiB` is koji's own and stays the default, so an
#: invocation that names nothing writes exactly the bytes it wrote before;
#: `128MiB` is the second shape `parallel-peak-rss` reads, chosen because it is
#: what `xz --block-size=128MiB` writes and what the locally recompressed koji
#: copy has (`CLAUDE.local.md`).
BLOCK_SIZES = ("24MiB", "128MiB")

#: The magic an `.xz` stream opens with, and the two bytes its footer closes
#: with. A boundary between two concatenated streams is the second followed by
#: the first, with optional four-byte-aligned zero padding in between.
STREAM_HEADER_MAGIC = b"\xfd7zXZ\x00"
STREAM_FOOTER_MAGIC = b"\x59\x5a"

#: How much of the koji file is read at a time while scanning for boundaries.
#: Large enough that the HDD is read sequentially rather than in seeks, small
#: enough that the scan holds nothing interesting.
SCAN_CHUNK = 32 * 1024 * 1024

#: The band the koji slice's compression ratio must land in, and the reason it
#: is a band rather than a remembered number.
#:
#: A decode rate is a rate per *plaintext* byte, so it is a property of the
#: bytes: this figure's own two legs are the proof, the generated control at
#: 5.4x decoding around 200 MB/s on one core where koji's 15.70x decodes around
#: 431. koji is nowhere near homogeneous — sampled at twelve depths it runs
#: from 5.02x to 33.05x — so `--from-offset` picks a draw rather than a
#: representative, and a moved offset, or a koji dump refreshed next year,
#: could land in the 31.74x band and republish a rate for quite different bytes
#: under the same table heading. That is the failure this instrument already
#: refuses twice elsewhere: what comes out is a plausible table rather than an
#: error.
#:
#: The band brackets the 15.70x the published slice has, wide enough that
#: ordinary drift in the corpus does not fire it and narrow enough that a slice
#: from another of koji's regions does. Widening it is a decision about which
#: bytes the figure is taken on, so it is made in the open — the constant is
#: hashed into the input's stamp, so changing it regenerates the slice and the
#: figure that reads it goes stale.
KOJI_RATIO_MIN = 14.0
KOJI_RATIO_MAX = 18.0


def compress(source: Path, out: Path, threads: int, block_size: str = BLOCK_SIZE) -> None:
    """`source` compressed into `out` at koji's container parameters.

    Through the `xz` binary rather than through Python's `lzma`, because the
    block size is the whole point and `lzma` exposes no way to set one: a
    single-block file admits exactly one worker, which would make every row of
    this figure's table the same reading.

    `block_size` is the one parameter a caller may move, and only within
    `BLOCK_SIZES`. Everything else stays koji's, so two legs at two block sizes
    differ in the one variable their table is about.
    """
    if block_size not in BLOCK_SIZES:
        raise SystemExit(
            f"block size {block_size!r} is not one any figure asks for; "
            f"registered: {', '.join(BLOCK_SIZES)}"
        )
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
                f"--block-size={block_size}",
                f"--check={CHECK}",
                "-c",
            ],
            stdin=src,
            stdout=dst,
            check=True,
        )
    tmp.replace(out)


def parse_xz_totals(text: str) -> tuple[int, int]:
    """`(compressed, uncompressed)` bytes out of `xz --list --robot` output.

    The `totals` line is tab-separated and positional:
    `totals streams blocks compressed uncompressed ratio check padding files`.
    Parsed apart from the subprocess call so the parsing is testable without a
    file to run `xz` against.
    """
    for line in text.splitlines():
        fields = line.split("\t")
        if fields[0] == "totals" and len(fields) > 4:
            return int(fields[3]), int(fields[4])
    raise SystemExit("xz --list --robot printed no totals line")


def xz_totals(path: Path) -> tuple[int, int]:
    """What `path` holds: `(compressed, uncompressed)` bytes.

    The plaintext volume of an `.xz` file is written down in its stream
    indexes and nowhere a `stat` can reach it, so this is the only cheap way
    to divide one by the other. It costs a seek per stream rather than a read:
    `xz --list` walks footers backwards, and 128 of them return instantly on a
    local file — the 85 s the koji *download* takes is 31,150 of them on an
    HDD.
    """
    if shutil.which("xz") is None:
        raise SystemExit("xz is not on PATH, and an .xz file's plaintext size is in its index")
    proc = subprocess.run(
        ["xz", "--list", "--robot", str(path)],
        stdout=subprocess.PIPE,
        text=True,
        check=True,
    )
    return parse_xz_totals(proc.stdout)


def check_koji_density(compressed: int, uncompressed: int, offset: int) -> float:
    """The slice's compression ratio, refused unless it is in the band.

    Refused rather than reported, because the thing being guarded against
    produces a perfectly plausible table: a slice from another of koji's
    regions decodes at its own rate, and the table would publish that rate
    under a heading naming this figure's bytes.
    """
    if compressed <= 0:
        raise SystemExit("the slice is empty, so it has no compression ratio")
    ratio = uncompressed / compressed
    if not KOJI_RATIO_MIN <= ratio <= KOJI_RATIO_MAX:
        raise SystemExit(
            f"the slice at offset {offset} compresses {ratio:.2f}x, outside the "
            f"{KOJI_RATIO_MIN:g}-{KOJI_RATIO_MAX:g}x this figure is taken on "
            f"({uncompressed} plaintext bytes in {compressed} compressed). A decode rate is a "
            "rate per plaintext byte, so bytes of another density belong to a different figure: "
            "move the offset (PGDT_KOJI_XZ_OFFSET) to a region inside the band, or decide in the "
            "open that the figure is taken on these bytes and widen KOJI_RATIO_MIN/MAX"
        )
    return ratio


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
    file's (`docs/design/decisions.md`, "The compressed source and the cache").

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
    parser.add_argument(
        "--block-size",
        default=BLOCK_SIZE,
        choices=BLOCK_SIZES,
        help=f"with --from-dump: the uncompressed block size (default {BLOCK_SIZE}, koji's own). "
        "A compressed reader's per-worker footprint is one decoded block, so this is the "
        "variable a two-block-size figure is taken over",
    )
    args = parser.parse_args()

    if args.from_dump is not None:
        if not args.from_dump.exists():
            raise SystemExit(f"{args.from_dump} does not exist")
        compress(args.from_dump, args.out, args.threads, args.block_size)
        compressed, uncompressed = xz_totals(args.out)
        ratio = uncompressed / compressed
    else:
        if not args.from_koji.exists():
            raise SystemExit(
                f"{args.from_koji} does not exist — this leg reads the koji download named in "
                "CLAUDE.local.md, and PGDT_KOJI_XZ is what points measure.py elsewhere"
            )
        start, end = koji_slice(args.from_koji, args.out, args.streams, args.from_offset)
        print(f"streams {args.streams} from {start} to {end}", file=sys.stderr)
        # Read off the slice that was written rather than off the source, and
        # after it is written rather than before: what the figure is taken on
        # is this file. A refusal leaves it in place — `measure.py` writes the
        # stamp only for a generator that exited 0, so the next sitting
        # regenerates regardless, and the bytes are still there to run
        # `xz --list` against.
        compressed, uncompressed = xz_totals(args.out)
        ratio = check_koji_density(compressed, uncompressed, args.from_offset)

    size = args.out.stat().st_size
    print(f"wrote {args.out} ({size / (1024 * 1024):.1f} MiB)", file=sys.stderr)
    # The density every sitting of this figure quotes with its rates. The
    # harness reports it again from the instrument's own delivered byte count,
    # which is the same division over the bytes that were actually decoded.
    print(
        f"plaintext {uncompressed} bytes, compressed {compressed}, ratio {ratio:.2f}x",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
