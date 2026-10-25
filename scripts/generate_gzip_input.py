#!/usr/bin/env python3
"""The `.gz` inputs the register's gzip figures are taken on.

`--from-dump` compresses an input `scripts/measure.py` already generates (the
brace-free `control`) into **one gzip member, the shape `pg_dump -Fp -Z`
writes**: zlib's own deflate at `pg_dump`'s default level, with no flush point
anywhere in it, behind the header every routine major writes
(`docs/design/roadmap-P15.1-fixtures-notes.md`, "Negative results") — no
`FNAME`, `MTIME` 0, `XFL` 0, OS Unix. What the figure reads is how fast a
first pass can inflate that shape serially, so a file of any other shape would
be a reading of something `pg_dump` does not write.

**Through Python's `zlib`, and refused unless that is the zlib this script
names.** `pg_dump` writes through zlib's `gzwrite`, which is zlib's `deflate`
at the parameters below, and Python's `zlib` is that `deflate` called with
them. Which zlib it is decides the bytes — a zlib-ng
build in compatibility mode would deflate the same plaintext into a different
stream of blocks — so the version is checked against `ZLIB_VERSION` rather
than recorded: the constant is in this file, which `measure.input_stamp`
hashes, so moving it regenerates the input and the figure reading it goes
stale.

**The header is checked against `PG_DUMP_HEADER`** on the bytes written, for
the same reason: it is what makes the file `pg_dump`'s shape rather than a
gzip file that merely decodes to the same plaintext.

**`--bgzf` writes BGZF instead**, the shape `bgzip` writes: one member a
`BGZF_BLOCK` of plaintext, each deflated alone at the same level with no
history from the one before, its header carrying the `BC` subfield that states
the member's length (`BGZF_HEADER`), and the fixed empty member that marks the
end (`BGZF_EOF`). It is what `gzip-resident`'s block-path legs read: `pgdt`
walks a local BGZF file at open, so its first `parse` decodes whole units
from the start where a single member's first pass cannot. Written here rather
than by `bgzip`, which is neither installed nor pinned, through the same zlib
the single member is, so the two files differ by their framing alone.

Generated, never committed. `scripts/measure.py` owns the paths; this script
owns the bytes.

    cd scripts
    uv run generate_gzip_input.py --from-dump control.sql out.gz
    uv run generate_gzip_input.py --from-dump control.sql --bgzf out.gz
"""

from __future__ import annotations

import argparse
import struct
import sys
import zlib
from pathlib import Path

#: The zlib this script's bytes come from. Moving it is a decision about
#: which stream the figure is taken on.
ZLIB_VERSION = "1.3.1"

#: `pg_dump`'s default gzip level (`Z_DEFAULT_COMPRESSION`), which is what a
#: bare `-Z gzip` and the `compress` fixtures' `--compress=6` both write.
LEVEL = 6

#: `windowBits` for a gzip-wrapped stream with a 32 KiB window: 15, plus 16 for
#: the gzip framing, as `gzwrite` opens it; `memLevel` and the strategy are
#: zlib's defaults, as there. zlib then writes a header with no name and
#: `MTIME` 0.
GZIP_WINDOW_BITS = 15 + 16

#: The header every routine major's `-Z` output opens with: magic, deflate,
#: no flags, `MTIME` 0, `XFL` 0, OS 3 (Unix).
PG_DUMP_HEADER = bytes.fromhex("1f8b08000000000000" + "03")

#: The plaintext one BGZF member holds: `htslib`'s `BGZF_BLOCK_SIZE`, which
#: `bgzip` fills every member but the last to. Its compressed member must fit
#: `BSIZE`'s 16 bits, which text at `LEVEL` does with room to spare, and a
#: member that does not is refused rather than split (`bgzf_member`).
BGZF_BLOCK = 0xFF00

#: A BGZF member's header up to `BSIZE`: magic, deflate, `FEXTRA`, `MTIME` 0,
#: `XFL` 0, OS 255 (unknown), `XLEN` 6, and the `BC` subfield of length 2 whose
#: value follows — the member's total length less one. What `htslib` writes.
BGZF_HEADER = bytes.fromhex("1f8b0804" "00000000" "00ff" "0600" "4243" "0200")

#: The empty member every BGZF file ends with (SAM/BAM specification, §4.1.2).
BGZF_EOF = bytes.fromhex("1f8b08040000000000ff0600424302001b0003000000000000000000")

#: Plaintext read from the source at a time. Large enough that the call
#: overhead is nothing, small enough that the generator holds nothing.
CHUNK = 8 * 1024 * 1024


def check_zlib(version: str) -> None:
    """Refuse a zlib other than `ZLIB_VERSION`, naming both."""
    if version != ZLIB_VERSION:
        raise SystemExit(
            f"this Python links zlib {version}, and the figure's input is zlib {ZLIB_VERSION}'s "
            "deflate: another zlib writes another stream from the same plaintext. Run it under "
            "a Python linking that zlib, or decide in the open that the figure is taken on "
            "another one and move ZLIB_VERSION"
        )


def check_header(header: bytes) -> None:
    """Refuse a member whose header is not the one `pg_dump` writes."""
    if header != PG_DUMP_HEADER:
        raise SystemExit(
            f"the member opens {header.hex()}, not pg_dump's {PG_DUMP_HEADER.hex()}: "
            "this zlib writes a header pg_dump's does not"
        )


def compress(source: Path, out: Path) -> None:
    """`source` deflated into one gzip member at `out`, written beside it
    first and moved into place once whole."""
    check_zlib(zlib.ZLIB_RUNTIME_VERSION)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_name(out.name + ".partial")
    deflater = zlib.compressobj(LEVEL, zlib.DEFLATED, GZIP_WINDOW_BITS)
    with source.open("rb") as src, tmp.open("wb") as dst:
        while chunk := src.read(CHUNK):
            dst.write(deflater.compress(chunk))
        dst.write(deflater.flush())
    with tmp.open("rb") as written:
        check_header(written.read(len(PG_DUMP_HEADER)))
    tmp.replace(out)


def bgzf_member(plaintext: bytes) -> bytes:
    """One BGZF member holding `plaintext`, deflated alone at `LEVEL`.

    Refused where the member would not fit `BSIZE`: `bgzip` would split such a
    block, and a generator that silently wrote another layout would hand the
    figure a file whose units it never registered."""
    deflater = zlib.compressobj(LEVEL, zlib.DEFLATED, -15)
    body = deflater.compress(plaintext) + deflater.flush()
    total = len(BGZF_HEADER) + 2 + len(body) + 8
    if total > 1 << 16:
        raise SystemExit(
            f"a {len(plaintext)}-byte block deflates to a {total}-byte member, past BSIZE's "
            "16 bits; bgzip would split it, and this generator writes one layout"
        )
    trailer = struct.pack("<II", zlib.crc32(plaintext), len(plaintext))
    return BGZF_HEADER + struct.pack("<H", total - 1) + body + trailer


def compress_bgzf(source: Path, out: Path) -> None:
    """`source` written as BGZF at `out`: a member a `BGZF_BLOCK`, then
    `BGZF_EOF`, beside it first and moved into place once whole."""
    check_zlib(zlib.ZLIB_RUNTIME_VERSION)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_name(out.name + ".partial")
    with source.open("rb") as src, tmp.open("wb") as dst:
        while block := src.read(BGZF_BLOCK):
            dst.write(bgzf_member(block))
        dst.write(BGZF_EOF)
    tmp.replace(out)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--from-dump",
        type=Path,
        required=True,
        metavar="SOURCE",
        help="the plain dump to compress, one of the inputs measure.py generates",
    )
    parser.add_argument(
        "--bgzf",
        action="store_true",
        help="write BGZF, a member a 65,280-byte block, rather than pg_dump's one member",
    )
    parser.add_argument("out", type=Path, help="where the .gz file goes")
    args = parser.parse_args(argv)
    (compress_bgzf if args.bgzf else compress)(args.from_dump, args.out)
    plain = args.from_dump.stat().st_size
    packed = args.out.stat().st_size
    print(f"{args.out}: {packed} bytes, {plain} of plaintext, {plain / packed:.2f}x")
    return 0


if __name__ == "__main__":
    sys.exit(main())
