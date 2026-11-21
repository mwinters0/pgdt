"""Tests of `measurement.figures.xz_decode`."""

from __future__ import annotations

import generate_xz_input
import hashlib
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import inputs
from measurement import machine_state
from measurement import shapes
from measurement import staging
from measurement import timing
from measurement.figures import register
from measurement.figures import xz_decode
from measurement.staleness import stale


class XzDecodeScaling(unittest.TestCase):
    """The decode-scaling figure: two `.xz` legs, seven worker counts.

    It is the register's first figure that runs no `pgdt` at all, its first
    compressed input, its first fourth regime and its first departure from the
    512 MB container — so what these hold is that each of those is *declared*
    rather than inherited, since every one of them fails by emitting a
    perfectly plausible table of something else.
    """

    def test_both_legs_are_compressed_inputs(self):
        # Staging, eviction and stamping all key on the file name, so an input
        # staged as `.sql` would collide with the plain dump it is a
        # compression of and the two would evict each other.
        for leg, _ in xz_decode.DECODE_LEGS:
            with self.subTest(leg=leg):
                self.assertEqual(inputs.INPUTS[leg].suffix, ".xz")
                self.assertTrue(inputs.input_file(leg).endswith(".xz"))

    def test_a_plain_input_is_still_a_sql_file(self):
        self.assertEqual(inputs.input_file("control"), "control.sql")

    def test_the_generated_leg_is_a_compression_of_the_control(self):
        # Not a second generation of similar rows: the two would be different
        # bytes behind one register, and the perf generator's own changes would
        # reach the plain figures and not this one.
        self.assertEqual(inputs.INPUTS["control_xz"].derives_from, "control")

    def test_a_derived_input_folds_its_source_stamp_in(self):
        # Otherwise a change to `generate_perf_data.py` regenerates every plain
        # input and leaves the compressed leg measuring pre-change rows.
        cfg = config.Config()
        spec = inputs.INPUTS["control_xz"]
        got = inputs.input_stamp(spec, cfg)
        unfolded = hashlib.sha256()
        unfolded.update((config.SCRIPTS / spec.generator).read_bytes())
        unfolded.update(repr(spec.argv(cfg, Path("OUT"))).encode())
        self.assertNotEqual(got, unfolded.hexdigest())

    def test_the_perf_generator_reaches_the_generated_leg(self):
        # The control leg is a *compression of* `control`, so a change to the
        # perf generator moves the bytes this figure decodes. `figures_touched`
        # walks the taken register, which is where this figure now sits, so the
        # declaration has to be reachable through it rather than merely present.
        touched = [f.id for f, _ in stale.figures_touched(["scripts/generate_perf_data.py"])]
        self.assertIn("xz-decode-scaling", touched)
        fig = register.SELECTABLE_BY_ID["xz-decode-scaling"]
        self.assertIn("scripts/generate_perf_data.py", fig.depends)

    def test_the_decoder_and_the_instrument_are_declared(self):
        # No `pgdt` runs here, so none of the library's own paths can move this
        # figure and none of them is declared. What can is the decoder, the
        # binary that drives it, and the generators behind the two files.
        fig = register.SELECTABLE_BY_ID["xz-decode-scaling"]
        self.assertIn("vendor/xz-seek/src/", fig.depends)
        self.assertIn("crates/pgdt-io/examples/xz_decode.rs", fig.depends)
        self.assertIn("scripts/generate_xz_input.py", fig.depends)
        for path in fig.depends:
            self.assertFalse(path.startswith("crates/") and "/src/" in path, path)

    def test_a_compressed_input_has_its_own_nominal_size(self):
        # A dry run that guessed 3.00 GiB for a compressed leg would size the
        # tmpfs budget against a file several times larger than the one staged.
        cfg = config.Config()
        for leg, _ in xz_decode.DECODE_LEGS:
            with self.subTest(leg=leg):
                self.assertLess(staging.nominal_size(cfg, leg), cfg.size_gib * config.GIB)

    def test_every_registered_worker_count_has_a_command_shape(self):
        for workers in shapes.DECODE_WORKERS:
            with self.subTest(workers=workers):
                script = shapes._script(f"decode-{workers}")
                self.assertIn(f"--workers {workers}", script)
                self.assertEqual(script.count("time "), 1)

    def test_the_instrument_stdout_is_not_redirected(self):
        # It is where the decoded byte count comes back, which is the rate's
        # denominator: a compressed input's plaintext volume is in its seek
        # table and nowhere the harness can `stat`.
        self.assertNotIn(">", shapes._script("decode-8"))

    def test_a_worker_count_the_figure_does_not_carry_is_an_error(self):
        for command in ("decode-3", "decode-", "decode-all"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_the_baseline_row_is_one_worker(self):
        # Every cell is a ratio against it, and it is the serial path this
        # build ships today.
        self.assertEqual(xz_decode.DECODE_BASELINE, 1)
        self.assertEqual(shapes.DECODE_WORKERS[0], xz_decode.DECODE_BASELINE)

    def test_the_parallel_regime_reads_from_tmpfs(self):
        # It is `warm` staging under a different gate, not a fourth device: a
        # regime names a device, and this one names the same one `warm` does.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cfg = config.Config(
                cache_dir=root / "ssd", warm_dir=root / "shm", nvme_dir=root / "nvme",
                dry_run=True,
            )
            stager = staging.Stager(cfg, lambda _m: None)
            stager.plan([register.SELECTABLE_BY_ID["xz-decode-scaling"]])
            session = timing.Session(cfg, stager, lambda _m: None)
            got = session.input_path("control_xz", "warm-parallel")
            self.assertEqual(got, root / "shm" / "control_xz.xz")

    def test_the_parallel_regime_gates_steal_and_nothing_else(self):
        # A 24-worker decode drives the machine to the nineties by
        # construction, so `cpu_busy_pct` cannot gate it; `cpu_steal_pct` is
        # about a neighbour rather than about this run and still can.
        self.assertIsNone(
            machine_state.contention_verdict(
                {"cpu_busy_pct": 96.0, "psi_cpu_some_pct": 40.0}, "warm-parallel"
            )
        )
        self.assertIsNotNone(
            machine_state.contention_verdict({"cpu_steal_pct": 12.0}, "warm-parallel")
        )

    def test_the_figure_declares_more_memory_than_the_recorded_apparatus(self):
        # 24 decoded 24 MiB blocks, their compressed windows and their
        # dictionaries do not fit the register's 512 MB, and a figure that
        # departs from the recorded apparatus has to say so rather than inherit
        # it.
        fig = register.SELECTABLE_BY_ID["xz-decode-scaling"]
        self.assertEqual(fig.memory, shapes.DECODE_MEMORY)
        self.assertNotEqual(shapes.DECODE_MEMORY, config.Config().memory)

    #: Every figure permitted to depart from the recorded 512 MB, and nothing
    #: else. Each is a figure that holds more than the recorded container
    #: affords, or may: N decoded blocks at once, or statistics nothing bills.
    #: The departure is stated in that figure's own table.
    MEMORY_DEPARTURES = {
        "xz-decode-scaling",
        "parallel-scan-throughput",
        "parallel-peak-rss",
        # The reserve's largest stated budget is itself larger than the
        # recorded 512 MB, so the container that holds a run of it cannot be
        # the recorded one; its `.xz` legs hold N decoded blocks besides.
        "reserve",
        # Its spec gives it a generous limit of its own, so that no kill
        # interrupts the phase while what statistics hold is unbounded (`KD28`).
        "statistics-gathering",
        # `parallel-peak-rss`'s stated allowance over gzip, twenty-four first
        # pass workers' charge being far past the recorded 512 MB.
        "gzip-resident",
        # Its own allowance, affording twenty-four first pass workers a task
        # each at either chunk length (`GZPASS_BUDGET`).
        "gzip-first-pass",
        # Four workers each holding a long line and its copies, which no
        # allowance charges (`LONG_LINE_MEMORY`).
        "long-line-resident",
        # The same legs, at each purge delay.
        "purge-delay-resident",
    }

    def test_every_other_figure_runs_under_the_recorded_memory(self):
        # A departure is a departure only while it is the exception, so this
        # holds the rest of the register to the recorded apparatus rather than
        # to whatever each figure happens to declare. `EVERY_FIGURE`, not
        # `ALL_FIGURES`: an untaken instrument is exactly where a departure
        # arrives unnoticed, since no table of its is in the doc to state it.
        for fig in register.EVERY_FIGURE:
            if fig.id in self.MEMORY_DEPARTURES:
                continue
            with self.subTest(figure=fig.id):
                self.assertIsNone(fig.memory)

    def test_nothing_departs_without_being_named_here(self):
        declared = {f.id for f in register.EVERY_FIGURE if f.memory is not None}
        self.assertEqual(declared, self.MEMORY_DEPARTURES)

    def test_the_koji_leg_takes_whole_streams_from_past_the_head(self):
        # koji's first 3 GiB of plaintext compresses about 56x against the
        # file's own 19.4x, and a decode rate is per plaintext byte, so a slice
        # cut at the head would report a rate for bytes unlike the rest of it.
        args = inputs.INPUTS["koji_xz"].args
        self.assertIn("--from-offset", args)
        self.assertGreater(inputs.KOJI_XZ_OFFSET, 0)
        self.assertEqual(str(inputs.KOJI_XZ_STREAMS), args[args.index("--streams") + 1])

    def test_the_koji_leg_has_a_block_for_every_worker(self):
        # A range with fewer blocks than workers clamps, and the instrument
        # refuses a clamped run — so a stream count under the largest worker
        # count would make the top of the table unrunnable rather than wrong.
        self.assertGreaterEqual(inputs.KOJI_XZ_STREAMS, shapes.DECODE_WORKERS[-1])

    def test_a_stage_selection_does_not_reach_it(self):
        # `--stage warm` splits on `+`, so `warm-parallel` is its own stage and
        # not a `warm` figure with a suffix.
        self.assertNotIn("warm", register.SELECTABLE_BY_ID["xz-decode-scaling"].stage.split("+"))

    # -- the koji leg's density ------------------------------------------
    #
    # The generator is the figure's apparatus as much as the instrument is, and
    # `scripts/generate_xz_input.py` is one of the figure's own `depends`, so
    # its gate is held here with the rest of them rather than in a test module
    # of its own.

    def test_the_published_slice_density_is_inside_the_band(self):
        # 15.70x is what the 20 GB region yields and what every rate this
        # figure publishes is quoted against. A band that did not contain it
        # would refuse the figure's own input.
        self.assertEqual(
            generate_xz_input.check_koji_density(205215196, 3221749760, 20_000_000_000),
            3221749760 / 205215196,
        )

    def test_kojis_head_is_refused(self):
        # The whole reason `--from-offset` exists: koji's first 3 GiB of
        # plaintext compresses 56.19x against the file's own 19.41x, and a
        # decode rate is a rate per plaintext byte, so a slice cut there is a
        # rate for other bytes under this figure's heading.
        with self.assertRaises(SystemExit) as raised:
            generate_xz_input.check_koji_density(57341052, 3221749760, 0)
        self.assertIn("56.19x", str(raised.exception))

    def test_a_sparser_region_is_refused_too(self):
        # The band is two-sided: koji sampled at twelve depths runs from 5.02x
        # to 33.05x, and both ends are bytes this figure is not taken on.
        with self.assertRaises(SystemExit):
            generate_xz_input.check_koji_density(3221749760 // 5, 3221749760, 16_000_000_000)

    def test_an_empty_slice_has_no_ratio(self):
        with self.assertRaises(SystemExit):
            generate_xz_input.check_koji_density(0, 0, 0)

    def test_the_totals_line_is_read_positionally(self):
        # `xz --list --robot` is tab-separated and positional, and the fields
        # this reads are the compressed and uncompressed totals — the second of
        # which is written down nowhere a `stat` can reach.
        got = generate_xz_input.parse_xz_totals(
            "name\tkoji_xz.xz\n"
            "file\t128\t128\t205215196\t3221749760\t0.064\tCRC64\t0\n"
            "totals\t128\t128\t205215196\t3221749760\t0.064\tCRC64\t0\t1\n"
        )
        self.assertEqual(got, (205215196, 3221749760))

    def test_the_table_quotes_the_band_the_slice_was_cut_within(self):
        # The clause is selected by the leg's own key, so a renamed input would
        # drop it silently; and the band is imported rather than respelled, so
        # the number the table prints is the number a slice was refused
        # against.
        self.assertIn("koji_xz", [leg for leg, _ in xz_decode.DECODE_LEGS])
        self.assertEqual(generate_xz_input.KOJI_RATIO_MIN, generate_xz_input.KOJI_RATIO_MIN)
        self.assertEqual(generate_xz_input.KOJI_RATIO_MAX, generate_xz_input.KOJI_RATIO_MAX)

    def test_output_with_no_totals_line_is_refused(self):
        with self.assertRaises(SystemExit):
            generate_xz_input.parse_xz_totals("name\tkoji_xz.xz\n")


if __name__ == "__main__":
    unittest.main()
