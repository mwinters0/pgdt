"""Tests for generate_long_lines.py: a cluster of lines of one exact length,
terminator included, between short rows, and the answer a query over them
gives."""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import generate_long_lines as gen

#: `COPY`'s text-format escapes the generator writes, decoded as PostgreSQL's
#: `CopyReadAttributesText` decodes them.
_ESCAPES = {b"n": b"\n", b"t": b"\t", b"\\": b"\\"}


def _copy_decode(field: bytes) -> bytes:
    out = bytearray()
    i = 0
    while i < len(field):
        if field[i : i + 1] == b"\\":
            out += _ESCAPES[field[i + 1 : i + 2]]
            i += 2
        else:
            out += field[i : i + 1]
            i += 1
    return bytes(out)


def _octets(kind: str, field: bytes) -> int:
    """`octet_length` of a field's value, decoded independently of the
    generator's own arithmetic."""
    value = _copy_decode(field)
    if kind == "bytea":
        assert value.startswith(b"\\x"), value[:8]
        return len(bytes.fromhex(value[2:].decode()))
    return len(value)


def _generate(kind: str, line_bytes: int, lines: int) -> list[bytes]:
    """The data lines of a generated file, each with its terminator."""
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "long.sql"
        gen.generate(out, kind, line_bytes, lines)
        text = out.read_bytes()
    _, _, body = text.partition(b" FROM stdin;\n")
    data, sep, _ = body.partition(b"\\.\n")
    assert sep, "the block is not terminated"
    return data.splitlines(keepends=True)


class GenerateLongLines(unittest.TestCase):
    def test_every_long_line_is_exactly_the_length_asked_terminator_included(self):
        for kind in gen.KINDS:
            for line_bytes in (64, 1000, 3 * gen.MIB):
                with self.subTest(kind=kind, line_bytes=line_bytes):
                    rows = _generate(kind, line_bytes, 3)
                    long = rows[gen.SHORT_ROWS : gen.SHORT_ROWS + 3]
                    self.assertEqual([len(r) for r in long], [line_bytes] * 3)
                    self.assertTrue(all(r.endswith(b"\n") for r in long))

    def test_the_cluster_is_consecutive_between_short_rows(self):
        rows = _generate("text", 4096, 4)
        lengths = [len(r) for r in rows]
        s = gen.SHORT_ROWS
        self.assertEqual(len(rows), 2 * s + 4)
        self.assertEqual(lengths[s : s + 4], [4096] * 4)
        self.assertTrue(all(n < 64 for n in lengths[:s] + lengths[s + 4 :]))
        ids = [int(r.split(b"\t", 1)[0]) for r in rows]
        self.assertEqual(len(set(ids)), len(ids))

    def test_the_row_count_is_every_row_written(self):
        for lines in (1, 4):
            with self.subTest(lines=lines):
                self.assertEqual(len(_generate("bytea", 64, lines)), gen.row_count(lines))

    def test_each_value_decodes_as_the_kind_says(self):
        for kind in gen.KINDS:
            for line_bytes in (64, 1000, 2 * gen.MIB + 6):
                with self.subTest(kind=kind, line_bytes=line_bytes):
                    rows = _generate(kind, line_bytes, 2)
                    s = gen.SHORT_ROWS
                    got = [_octets(kind, r.rstrip(b"\n").split(b"\t", 1)[1]) for r in rows]
                    want = gen.decoded_bytes(kind, gen.field_bytes(kind, line_bytes))
                    self.assertEqual(got[s : s + 2], [want, want])
                    short = gen.SHORT_VALUE[kind][1]
                    self.assertEqual(got[:s] + got[s + 2 :], [short] * (2 * s))

    def test_decoding_shortens_each_kind_as_its_docstring_says(self):
        n = 8 * 1000 + 3
        self.assertEqual(gen.decoded_bytes("text", n), n)
        self.assertEqual(gen.decoded_bytes("escaped", 8 * 1000), 7 * 1000)
        self.assertEqual(gen.decoded_bytes("bytea", n), (n - 3) // 2)

    def test_an_escape_is_never_cut(self):
        for line_bytes in range(64, 80):
            with self.subTest(line_bytes=line_bytes):
                field = _generate("escaped", line_bytes, 1)[gen.SHORT_ROWS].split(b"\t", 1)[1]
                self.assertIsNotNone(re.fullmatch(rb"(?:[a-z]|\\n)*\n", field))

    def test_an_array_is_its_elements_and_a_jsonb_object_its_pairs(self):
        for line_bytes in range(64, 80, 2):
            with self.subTest(line_bytes=line_bytes):
                field = lambda kind: (  # noqa: E731
                    _generate(kind, line_bytes, 1)[gen.SHORT_ROWS].split(b"\t", 1)[1].rstrip(b"\n")
                )
                self.assertIsNotNone(re.fullmatch(rb"\{(?:[1-9],)*[1-9]\}", field("int8array")))
                self.assertIsNotNone(re.fullmatch(rb"\{(?:[a-z],)*[a-z]\}", field("textarray")))
                doc = field("jsonb")
                self.assertIsNotNone(re.fullmatch(rb'\{(?:"":0,)*"":10{0,4}\}', doc))
                # Valid JSON, its keys repeated as `jsonb_in` reads them.
                self.assertEqual(json.loads(doc), {"": json.loads(doc.rpartition(b":")[2][:-1])})

    def test_a_line_the_kind_cannot_fill_is_refused(self):
        with self.assertRaises(ValueError):
            gen.field_bytes("bytea", 65)
        with self.assertRaises(ValueError):
            gen.field_bytes("text", 5)
        with self.assertRaises(ValueError):
            gen.field_bytes("json", 64)
        for kind in ("int8array", "textarray"):
            with self.assertRaises(ValueError):
                gen.field_bytes(kind, 65)
        with self.assertRaises(ValueError):
            gen.field_bytes("jsonb", 10)
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                gen.generate(Path(tmp) / "x.sql", "text", 64, gen.MAX_LINES + 1)

    def test_every_mebibyte_length_and_the_bound_fill_each_kind(self):
        # I71's bound, `MaxAllocSize - 1`, is two bytes under a power of two.
        for kind in gen.KINDS:
            for line_bytes in (gen.MIB, 32 * gen.MIB, 288 * gen.MIB, 512 * gen.MIB, 0x3FFFFFFE):
                with self.subTest(kind=kind, line_bytes=line_bytes):
                    gen.field_bytes(kind, line_bytes)

    def test_the_id_is_nullable_and_every_row_holds_one(self):
        # `measurement.shapes.LONG_LINE_SQL` filters on `id IS NOT NULL` so that its
        # `count(v)` reads every value: a column declared `NOT NULL` lets the
        # filter be simplified away, and the count be answered from the plan
        # node's statistics with no row read.
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "long.sql"
            gen.generate(out, "text", 64, 2)
            ddl = out.read_text().partition("CREATE TABLE")[2].partition(";")[0]
        self.assertIn("    id integer,\n", ddl)
        self.assertNotIn("NOT NULL", ddl.upper())
        self.assertNotIn("PRIMARY KEY", ddl.upper())
        for row in _generate("text", 64, 2):
            with self.subTest(row=row[:16]):
                self.assertNotEqual(row.split(b"\t", 1)[0], b"\\N")

    def test_the_table_declares_the_kinds_type(self):
        with tempfile.TemporaryDirectory() as tmp:
            for kind, declared in (
                ("text", "text"),
                ("escaped", "text"),
                ("bytea", "bytea"),
                ("int8array", "bigint[]"),
                ("textarray", "text[]"),
                ("jsonb", "jsonb"),
            ):
                out = Path(tmp) / f"{kind}.sql"
                gen.generate(out, kind, 64, 1)
                self.assertIn(f"    v {declared}\n", out.read_text())

    def test_help(self):
        out = subprocess.run(
            [sys.executable, str(Path(gen.__file__)), "--help"], capture_output=True, text=True
        )
        self.assertEqual(out.returncode, 0)
        self.assertIn("--line-bytes", out.stdout)


if __name__ == "__main__":
    unittest.main()
