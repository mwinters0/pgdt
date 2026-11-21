"""Tests of `measurement.figures.inflate`."""

from __future__ import annotations

import re
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import borrowing
from measurement import config
from measurement import inputs
from measurement import shapes
from measurement import staging
from measurement.figures import declaration
from measurement.figures import inflate
from measurement.figures import register


class InflateThroughput(unittest.TestCase):
    """Serial inflate of one gzip member against the plain `parse`."""

    INSTRUMENT = config.REPO / "crates/gzip-inflate/src/main.rs"

    def test_it_is_taken_and_borrows_nothing(self):
        # Standing in no sharing edge is what lets it publish outside a sweep,
        # from the commit carrying its instrument.
        fig = register.SELECTABLE_BY_ID["inflate-throughput"]
        self.assertIn(fig, register.FIGURES)
        self.assertNotIn(fig, register.UNTAKEN)
        self.assertEqual(fig.shares, ())
        self.assertEqual(borrowing.entangled_with("inflate-throughput"), [])

    def test_the_gzip_leg_is_a_compression_of_the_control(self):
        # So its plaintext is `control`'s size exactly, which is what every
        # inflate row's rate is per and what the instrument's count is held to.
        spec = inputs.INPUTS["control_gz"]
        self.assertEqual(spec.derives_from, "control")
        self.assertEqual(spec.generator, "generate_gzip_input.py")
        self.assertEqual(inputs.input_file("control_gz"), "control_gz.gz")
        self.assertLess(staging.nominal_size(config.Config(), "control_gz"),
                        config.Config().size_gib * config.GIB)

    def test_what_can_move_it_is_declared(self):
        fig = register.SELECTABLE_BY_ID["inflate-throughput"]
        for path in (
            "crates/gzip-inflate/src/",
            # Where the backends' versions are pinned.
            "crates/gzip-inflate/Cargo.toml",
            "scripts/generate_gzip_input.py",
            *declaration.GEN_PERF,
            # The `parse` row's path, as every throughput figure declares it.
            *declaration.SCAN,
            *declaration.READ,
            *declaration.QUERY_CLI,
        ):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)

    def test_it_reads_both_files_on_every_device(self):
        fig = register.SELECTABLE_BY_ID["inflate-throughput"]
        self.assertEqual(fig.stage.split("+"), [r for r, _ in inflate.INFLATE_REGIMES])
        for inputs in (fig.warm_inputs, fig.cold_inputs, fig.nvme_inputs):
            self.assertEqual(set(inputs), {"control", "control_gz"})

    def test_the_ratio_against_the_parse_is_a_declared_subtraction(self):
        # Two inputs' readings divided, so both must come from one sweep.
        fig = register.SELECTABLE_BY_ID["inflate-throughput"]
        self.assertEqual(
            [set(s.inputs) for s in fig.subtracts], [{"control", "control_gz"}]
        )
        self.assertEqual(register.subtraction_sweep("inflate-throughput", "control", "control_gz"), 0)

    def test_every_backend_has_a_shape_naming_it(self):
        for backend in shapes.INFLATE_BACKENDS:
            with self.subTest(backend=backend):
                script = shapes._script(f"{shapes.INFLATE_FAMILY}{backend}")
                self.assertIn(f"--backend {backend}", script)
                self.assertEqual(script.count("time "), 1)
                # Its stdout is where the delivered count comes back.
                self.assertNotIn(">", script)

    def test_a_backend_the_figure_does_not_carry_is_an_error(self):
        for command in ("inflate-zlib", "inflate-", "inflate-libdeflate"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_the_instrument_spells_every_backend_the_harness_asks_for(self):
        # One vocabulary in two languages: a backend the harness names and the
        # instrument does not would fail a whole sitting at its first rep.
        source = self.INSTRUMENT.read_text()
        accepted = set(re.findall(r'^\s*"([a-z-]+)" => Ok\(Self::', source, re.MULTILINE))
        self.assertEqual(accepted, set(shapes.INFLATE_BACKENDS))

    def test_every_row_runs_in_every_regime(self):
        specs = inflate._inflate_specs()
        self.assertEqual(len(specs), len(inflate.INFLATE_ROWS) * len(inflate.INFLATE_REGIMES))
        self.assertEqual(
            {s.binary for s in specs if s.command.startswith(shapes.INFLATE_FAMILY)},
            {"gzinflate"},
        )
        self.assertEqual(
            [s.command for s in specs if s.regime == "warm" and s.binary == "pgdt"], ["parse"]
        )

    # -- the generator ---------------------------------------------------

    def test_the_header_it_holds_to_is_pg_dumps(self):
        # Read off `pg_dump`'s own `-Z` fixture rather than restated: the
        # generated file is `pg_dump`'s shape only while the two agree.
        import generate_gzip_input

        for fixture in sorted((config.REPO / "fixtures").glob("*/edge_cases/compress.sql.gz")):
            with self.subTest(fixture=str(fixture)):
                header = fixture.read_bytes()[: len(generate_gzip_input.PG_DUMP_HEADER)]
                self.assertEqual(header, generate_gzip_input.PG_DUMP_HEADER)

    def test_another_zlib_is_refused(self):
        import generate_gzip_input

        generate_gzip_input.check_zlib(generate_gzip_input.ZLIB_VERSION)
        with self.assertRaises(SystemExit):
            generate_gzip_input.check_zlib("1.2.13")

    def test_another_header_is_refused(self):
        import generate_gzip_input

        named = bytearray(generate_gzip_input.PG_DUMP_HEADER)
        named[3] = 1 << 3  # FNAME, which `gzip` writes and `pg_dump` does not
        with self.assertRaises(SystemExit):
            generate_gzip_input.check_header(bytes(named))

    def test_it_writes_one_member_holding_the_source(self):
        import gzip
        import zlib

        import generate_gzip_input

        with tempfile.TemporaryDirectory() as tmp:
            source, out = Path(tmp) / "plain.sql", Path(tmp) / "out.gz"
            source.write_bytes(b"COPY public.t (a) FROM stdin;\n1\n\\.\n" * 1000)
            # The version gate is tested above; this one is about the bytes.
            with unittest.mock.patch.object(
                generate_gzip_input.zlib, "ZLIB_RUNTIME_VERSION", generate_gzip_input.ZLIB_VERSION
            ):
                generate_gzip_input.compress(source, out)
            data = out.read_bytes()
            self.assertEqual(gzip.decompress(data), source.read_bytes())
            # One member: a raw inflate of it leaves nothing after its trailer.
            inflater = zlib.decompressobj(generate_gzip_input.GZIP_WINDOW_BITS)
            inflater.decompress(data)
            self.assertTrue(inflater.eof)
            self.assertEqual(inflater.unused_data, b"")
            self.assertFalse((Path(tmp) / "out.gz.partial").exists())


if __name__ == "__main__":
    unittest.main()
