#!/usr/bin/env python3
"""The `.gz` inputs the register's gzip figures are taken on.

`--from-dump` compresses an input `scripts/measure.py` already generates (the
brace-free `control`) into **one gzip member, the shape `pg_dump -Fp -Z`
writes**: zlib's own deflate at `pg_dump`'s default level, with no flush point
anywhere in it, behind the header every routine major writes
(`PG_DUMP_HEADER`, which `test_measure.py` reads back off each major's
`compress.sql.gz`) — no `FNAME`, `MTIME` 0, `XFL` 0, OS Unix. What the figure reads is how fast a
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

**`--writer` and `--level` choose another single member's writer**, for
`gzip-first-pass`, which reads a parallel first pass over more than one
writer's blocks: `zlib` (the default) at `--level 6` is `pg_dump -Z` over
stock zlib; `zlib-ng-compat` is the same call where the zlib `pg_dump` links
is Fedora's `zlib-ng-compat` build; `zlib-ng` at `--level 1` is the writer
that ends no block before its last (`deflate_quick`); `gzip` is GNU gzip,
`pg_dump | gzip`. Each is refused unless it is the version this script names
(`ZLIBNG_COMPAT_VERSION`, `ZLIBNG_VERSION`, `GNU_GZIP_VERSION`), for
`ZLIB_VERSION`'s reason, and each header is held to `pg_dump`'s at that level
(`header_for`). zlib-ng is the `zlib-ng` package `pyproject.toml` pins, which
bundles its own; GNU gzip is the one on `PATH`.

**`zlib-ng-compat` is Fedora's library itself, not a build of the same
source**: zlib-ng 2.2.5, as the `zlib-ng` package bundles it, writes another
stream at level 6 than Fedora 44's build does, so a leg labelled `pg_dump -Z`
on Fedora is written by the `libz.so.1` Fedora ships. It is copied out of
`FEDORA_IMAGE`, pinned by digest, once its package is checked to be
`ZLIBNG_COMPAT_VERSION`, and `deflate` is called through `ctypes` at the
parameters `gzwrite` opens it with (`LibzDeflater`). The library needs only
glibc, so the host loads it. `PGDT_MEASURE_CONTAINER` names the container
runtime, as it does for `measure.py`.

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
    uv run generate_gzip_input.py --from-dump control.sql --writer zlib-ng-compat out.gz
    uv run generate_gzip_input.py --from-dump control.sql --writer zlib-ng --level 1 out.gz
    uv run generate_gzip_input.py --from-dump control.sql --bgzf out.gz
"""

from __future__ import annotations

import argparse
import ctypes
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

#: The zlib this script's bytes come from. Moving it is a decision about
#: which stream the figure is taken on.
ZLIB_VERSION = "1.3.1"

#: The zlib-ng `--writer zlib-ng` deflates through: the version the `zlib-ng`
#: package `pyproject.toml` pins bundles. Moving it is `ZLIB_VERSION`'s
#: decision.
ZLIBNG_VERSION = "2.2.5"

#: The Fedora `zlib-ng-compat` package `--writer zlib-ng-compat` deflates
#: through, as `rpm` names its version and release in `FEDORA_IMAGE`. Moving
#: it is `ZLIB_VERSION`'s decision.
ZLIBNG_COMPAT_VERSION = "2.3.3-3.fc44"

#: The image `ZLIBNG_COMPAT_VERSION`'s `libz.so.1` is copied out of: Fedora
#: 44, pinned by digest so the package the version check reads cannot move
#: under the tag.
FEDORA_IMAGE = (
    "docker.io/library/fedora@sha256:43b29f65a41eb9c35e1cd5323e3bdf3b655c2357a9f4f1ff2f9c2798e5045d80"
)

#: What that library's `zlibVersion()` answers: the zlib API it is compatible
#: with, marked as zlib-ng's. Checked so the library loaded is the one copied.
ZLIBNG_COMPAT_API = "1.3.1.zlib-ng"

#: The GNU gzip `--writer gzip` runs, as its `--version` names it, for the
#: same reason.
GNU_GZIP_VERSION = "1.15"

#: The single member's writers `--writer` names, the default first.
WRITERS = ("zlib", "zlib-ng-compat", "zlib-ng", "gzip")

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


def header_for(level: int) -> bytes:
    """`PG_DUMP_HEADER` at `level`: its `XFL` is 2 at level 9, 4 at level 1
    and 0 otherwise, as zlib's `deflate.c` writes it, and zlib-ng's and GNU
    gzip's with it."""
    xfl = 2 if level == 9 else 4 if level == 1 else 0
    return PG_DUMP_HEADER[:8] + bytes([xfl]) + PG_DUMP_HEADER[9:]


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


def check_zlib_ng(version: str) -> None:
    """Refuse a zlib-ng other than `ZLIBNG_VERSION`, naming both."""
    if version != ZLIBNG_VERSION:
        raise SystemExit(
            f"the zlib-ng package bundles zlib-ng {version}, and the figure's input is zlib-ng "
            f"{ZLIBNG_VERSION}'s deflate: pin the package that bundles it, or decide in the open "
            "that the figure is taken on another one and move ZLIBNG_VERSION"
        )


def check_zlib_ng_compat(version: str) -> None:
    """Refuse a `zlib-ng-compat` other than `ZLIBNG_COMPAT_VERSION`, naming
    both."""
    if version != ZLIBNG_COMPAT_VERSION:
        raise SystemExit(
            f"{FEDORA_IMAGE} holds zlib-ng-compat {version!r}, and the figure's input is "
            f"zlib-ng-compat {ZLIBNG_COMPAT_VERSION}'s deflate: pin the image that holds it, or "
            "decide in the open that the figure is taken on another one and move "
            "ZLIBNG_COMPAT_VERSION"
        )


def load_zlib_ng_compat() -> ctypes.CDLL:
    """Fedora's `libz.so.1`, copied out of `FEDORA_IMAGE` once its package's
    version is checked, loaded, and its copy removed: the mapping outlives the
    file."""
    container = os.environ.get("PGDT_MEASURE_CONTAINER", "sudo nerdctl").split()
    with tempfile.TemporaryDirectory() as tmp:
        ran = subprocess.run(
            [
                *container, "run", "--rm", "--network", "none", "-v", f"{tmp}:/out",
                FEDORA_IMAGE, "sh", "-c",
                'rpm -q --qf "%{VERSION}-%{RELEASE}" zlib-ng-compat '
                "&& cp -L /usr/lib64/libz.so.1 /out/libz.so.1",
            ],
            capture_output=True,
            text=True,
        )
        if ran.returncode != 0:
            raise SystemExit(
                f"could not copy zlib-ng-compat out of {FEDORA_IMAGE}: {ran.stderr.strip()}"
            )
        check_zlib_ng_compat(ran.stdout.strip())
        lib = ctypes.CDLL(str(Path(tmp) / "libz.so.1"))
    lib.zlibVersion.restype = ctypes.c_char_p
    if (api := lib.zlibVersion().decode()) != ZLIBNG_COMPAT_API:
        raise SystemExit(f"the library copied answers zlib {api}, not {ZLIBNG_COMPAT_API}")
    return lib


class _ZStream(ctypes.Structure):
    """zlib's `z_stream`, which zlib-ng's compatibility build keeps."""

    _fields_ = [
        ("next_in", ctypes.c_void_p),
        ("avail_in", ctypes.c_uint),
        ("total_in", ctypes.c_ulong),
        ("next_out", ctypes.c_void_p),
        ("avail_out", ctypes.c_uint),
        ("total_out", ctypes.c_ulong),
        ("msg", ctypes.c_char_p),
        ("state", ctypes.c_void_p),
        ("zalloc", ctypes.c_void_p),
        ("zfree", ctypes.c_void_p),
        ("opaque", ctypes.c_void_p),
        ("data_type", ctypes.c_int),
        ("adler", ctypes.c_ulong),
        ("reserved", ctypes.c_ulong),
    ]


class LibzDeflater:
    """A `compressobj` over a loaded libz's own `deflate`, opened as `gzwrite`
    opens it: `deflateInit2` at `level`, `Z_DEFLATED`, `wbits`, memLevel 8 and
    the default strategy, as `zlib.compressobj` does over Python's zlib."""

    Z_OK, Z_STREAM_END, Z_BUF_ERROR = 0, 1, -5
    Z_NO_FLUSH, Z_FINISH = 0, 4
    #: The output buffer each `deflate` call writes into.
    OUT = 1 << 20

    def __init__(self, lib: ctypes.CDLL, level: int, wbits: int):
        self._lib = lib
        self._strm = _ZStream()
        self._out = ctypes.create_string_buffer(self.OUT)
        lib.zlibVersion.restype = ctypes.c_char_p
        lib.deflateInit2_.argtypes = [
            ctypes.POINTER(_ZStream), ctypes.c_int, ctypes.c_int, ctypes.c_int,
            ctypes.c_int, ctypes.c_int, ctypes.c_char_p, ctypes.c_int,
        ]
        lib.deflate.argtypes = [ctypes.POINTER(_ZStream), ctypes.c_int]
        lib.deflateEnd.argtypes = [ctypes.POINTER(_ZStream)]
        rc = lib.deflateInit2_(
            ctypes.byref(self._strm), level, zlib.DEFLATED, wbits, 8, 0,
            lib.zlibVersion(), ctypes.sizeof(_ZStream),
        )
        if rc != self.Z_OK:
            raise SystemExit(f"deflateInit2 returned {rc}")

    def _run(self, data: bytes, flush: int) -> bytes:
        held = ctypes.create_string_buffer(data, len(data))
        self._strm.next_in = ctypes.cast(held, ctypes.c_void_p)
        self._strm.avail_in = len(data)
        out = []
        while True:
            self._strm.next_out = ctypes.cast(self._out, ctypes.c_void_p)
            self._strm.avail_out = self.OUT
            rc = self._lib.deflate(ctypes.byref(self._strm), flush)
            if rc not in (self.Z_OK, self.Z_STREAM_END, self.Z_BUF_ERROR):
                raise SystemExit(f"deflate returned {rc}")
            out.append(self._out.raw[: self.OUT - self._strm.avail_out])
            if flush == self.Z_FINISH:
                if rc == self.Z_STREAM_END:
                    self._lib.deflateEnd(ctypes.byref(self._strm))
                    return b"".join(out)
            elif self._strm.avail_out != 0 and self._strm.avail_in == 0:
                return b"".join(out)

    def compress(self, data: bytes) -> bytes:
        return self._run(data, self.Z_NO_FLUSH)

    def flush(self) -> bytes:
        return self._run(b"", self.Z_FINISH)


def gnu_gzip_version() -> str:
    """The version the `gzip` on `PATH` names, or what it said instead."""
    if shutil.which("gzip") is None:
        return "no gzip on PATH"
    first = subprocess.run(
        ["gzip", "--version"], check=True, capture_output=True, text=True
    ).stdout.splitlines()
    return first[0].removeprefix("gzip ").strip() if first else ""


def check_gnu_gzip(version: str) -> None:
    """Refuse a GNU gzip other than `GNU_GZIP_VERSION`, naming both."""
    if version != GNU_GZIP_VERSION:
        raise SystemExit(
            f"the gzip on PATH is {version!r}, and the figure's input is GNU gzip "
            f"{GNU_GZIP_VERSION}'s deflate: run it where that gzip is, or decide in the open "
            "that the figure is taken on another one and move GNU_GZIP_VERSION"
        )


def check_header(header: bytes, level: int = LEVEL) -> None:
    """Refuse a member whose header is not the one `pg_dump` writes at
    `level`."""
    want = header_for(level)
    if header != want:
        raise SystemExit(
            f"the member opens {header.hex()}, not pg_dump's {want.hex()}: "
            "this writer writes a header pg_dump's does not"
        )


def deflater_for(writer: str, level: int):
    """A `compressobj` writing one gzip member at `level` through `writer`'s
    deflate, its version checked: `zlib`, `zlib-ng-compat` or `zlib-ng`."""
    if writer == "zlib":
        check_zlib(zlib.ZLIB_RUNTIME_VERSION)
        return zlib.compressobj(level, zlib.DEFLATED, GZIP_WINDOW_BITS)
    if writer == "zlib-ng-compat":
        return LibzDeflater(load_zlib_ng_compat(), level, GZIP_WINDOW_BITS)
    if writer == "zlib-ng":
        from zlib_ng import zlib_ng

        check_zlib_ng(zlib_ng.ZLIBNG_RUNTIME_VERSION)
        return zlib_ng.compressobj(level, zlib_ng.DEFLATED, GZIP_WINDOW_BITS)
    raise ValueError(f"no in-process writer named {writer!r}")


def compress(source: Path, out: Path, writer: str = "zlib", level: int = LEVEL) -> None:
    """`source` written by `writer` at `level` into one gzip member at `out`,
    beside it first and moved into place once whole. GNU gzip is run as
    `gzip -n`, so its header carries no name and `MTIME` 0, as a pipe's would
    but for the time."""
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_name(out.name + ".partial")
    if writer == "gzip":
        check_gnu_gzip(gnu_gzip_version())
        with source.open("rb") as src, tmp.open("wb") as dst:
            subprocess.run(["gzip", "-n", f"-{level}", "-c"], stdin=src, stdout=dst, check=True)
    else:
        deflater = deflater_for(writer, level)
        with source.open("rb") as src, tmp.open("wb") as dst:
            while chunk := src.read(CHUNK):
                dst.write(deflater.compress(chunk))
            dst.write(deflater.flush())
    with tmp.open("rb") as written:
        check_header(written.read(len(PG_DUMP_HEADER)), level)
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
    parser.add_argument(
        "--writer",
        choices=WRITERS,
        default=WRITERS[0],
        help="whose deflate writes the single member: pg_dump's zlib (the default), Fedora's "
        "zlib-ng-compat, zlib-ng's, or GNU gzip's",
    )
    parser.add_argument(
        "--level",
        type=int,
        choices=range(1, 10),
        default=LEVEL,
        metavar="1-9",
        help=f"the single member's level, {LEVEL} by default as pg_dump's",
    )
    parser.add_argument("out", type=Path, help="where the .gz file goes")
    args = parser.parse_args(argv)
    if args.bgzf:
        if (args.writer, args.level) != (WRITERS[0], LEVEL):
            parser.error("--bgzf is written through zlib at pg_dump's level alone")
        compress_bgzf(args.from_dump, args.out)
    else:
        compress(args.from_dump, args.out, args.writer, args.level)
    plain = args.from_dump.stat().st_size
    packed = args.out.stat().st_size
    print(f"{args.out}: {packed} bytes, {plain} of plaintext, {plain / packed:.2f}x")
    return 0


if __name__ == "__main__":
    sys.exit(main())
