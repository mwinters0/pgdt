"""Tests for generate_repeated_rows.py: a dump's rows repeated in runs, each
block kept to its source's length, deflated into `pg_dump`'s one member."""

from __future__ import annotations

import gzip
import tempfile
import unittest
from pathlib import Path

import generate_gzip_input
import generate_perf_data as perf
import generate_repeated_rows as gen

#: `measure.GZIP_SPECULATION_LIMIT` over `measure.GZIP_CHUNK`: the ratio a
#: chunk must inflate past to reach the limit. Restated rather than imported,
#: `measure` being the module whose check this generator answers to.
LIMIT_RATIO = 2 * ((1 << (9 + 6)) - 1) * 258 / (4 << 20)


def _blocks(data: bytes) -> tuple[list[bytes], list[list[bytes]]]:
    """Everything outside the `COPY` blocks, in order, and each block's rows."""
    outside, blocks, rows = [], [], None
    for line in data.splitlines(keepends=True):
        if rows is None:
            outside.append(line)
            if line.startswith(gen.COPY_OPEN) and line.endswith(gen.COPY_OPEN_END):
                rows = []
        elif line == gen.COPY_END:
            outside.append(line)
            blocks.append(rows)
            rows = None
        else:
            rows.append(line)
    return outside, blocks


class RepeatRows(unittest.TestCase):
    def _generate(self, size_bytes: int) -> tuple[bytes, bytes, bytes, int]:
        """The source, the member's plaintext, the member, and the length
        `write_member` said it deflated."""
        with tempfile.TemporaryDirectory() as tmp:
            source, out = Path(tmp) / "control.sql", Path(tmp) / "repeated.gz"
            perf.generate(source, size_bytes, 42, arrays=False, composites=False)
            said = gen.write_member(source, out)
            member = out.read_bytes()
            return source.read_bytes(), gzip.decompress(member), member, said

    def test_everything_outside_a_block_is_the_sources(self):
        source, out, _, _ = self._generate(1 << 20)
        self.assertEqual(_blocks(out)[0], _blocks(source)[0])

    def test_each_kept_row_is_a_source_row_in_a_run_of_repeat(self):
        source, out, _, _ = self._generate(1 << 20)
        (src_rows,), (out_rows,) = _blocks(source)[1], _blocks(out)[1]
        self.assertEqual(len(out_rows) % gen.REPEAT, 0)
        runs = [out_rows[i : i + gen.REPEAT] for i in range(0, len(out_rows), gen.REPEAT)]
        for run in runs:
            self.assertEqual(set(run), {run[0]})
        kept = [run[0] for run in runs]
        # In the source's order, each one of its rows, none twice.
        positions = [src_rows.index(row) for row in kept]
        self.assertEqual(positions, sorted(set(positions)))

    def test_a_block_keeps_its_sources_length_within_one_run(self):
        source, out, _, said = self._generate(1 << 20)
        self.assertEqual(said, len(out))
        (src_rows,), (out_rows,) = _blocks(source)[1], _blocks(out)[1]
        read, written = sum(map(len, src_rows)), sum(map(len, out_rows))
        longest = max(map(len, src_rows))
        self.assertGreaterEqual(written + longest * gen.REPEAT, read)
        self.assertLessEqual(written, read + longest * gen.REPEAT)

    def test_the_rows_pack_past_the_limits_ratio_at_pg_dumps_level(self):
        # The point of the input: at level 6, a chunk inflates past the
        # speculation limit. `measure` asks it of the whole generated file
        # before a sitting reads it; this asks it of a sample.
        _, out, member, _ = self._generate(8 << 20)
        self.assertGreater(len(out) / len(member), LIMIT_RATIO)

    def test_the_member_is_pg_dumps_shape(self):
        # One member behind `pg_dump`'s header, as `control_gz` is.
        _, _, member, _ = self._generate(1 << 20)
        self.assertEqual(
            member[: len(generate_gzip_input.PG_DUMP_HEADER)], generate_gzip_input.PG_DUMP_HEADER
        )

    def test_a_block_that_never_ends_is_refused(self):
        lines = [b"COPY public.t (a) FROM stdin;\n", b"1\n", b"2\n"]
        with self.assertRaises(SystemExit):
            list(gen.repeated_lines(lines))

    def test_a_run_of_nothing_is_refused(self):
        with self.assertRaises(ValueError):
            list(gen.repeated_lines([], repeat=0))


if __name__ == "__main__":
    unittest.main()
