"""Tests of `measurement.figures.long_line`."""

from __future__ import annotations

import dataclasses
import generate_long_lines as long_lines
import unittest
import unittest.mock
from fractions import Fraction

from measurement import test_support
from measurement import config
from measurement import inputs
from measurement import memory_model
from measurement import parsing
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement import timing
from measurement.figures import declaration
from measurement.figures import long_line
from measurement.figures import register


class LongLineResident(unittest.TestCase):
    """What a pass holds while it holds a long line: the passes at two counts
    over a cluster of long lines, by value kind and line length, and each
    query's factor read off its plan."""

    FIGURE = "long-line-resident"
    TERMS = ("int8array", "textarray", "jsonb")
    #: Stand-in factors, a kind each: never the library's arithmetic.
    FACTORS = {
        "text": Fraction(1),
        "escaped": Fraction(1),
        "bytea": Fraction(1),
        "int8array": Fraction(3),
        "textarray": Fraction(9),
        "jsonb": Fraction(1, 2),
    }

    def test_it_is_taken_and_borrows_nothing(self):
        # Standing in no sharing edge is what lets it publish from its own
        # commit, outside a sweep; taken, it is in `FIGURES`, where the
        # doc-side checks apply.
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        self.assertIn(fig, register.FIGURES)
        self.assertNotIn(fig, register.UNTAKEN)
        self.assertEqual(fig.shares, ())
        self.assertEqual(fig.stage, "warm-parallel")
        self.assertEqual(fig.memory, shapes.LONG_LINE_MEMORY)

    def test_what_can_move_it_is_declared(self):
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        for path in (
            "scripts/generate_long_lines.py",
            *declaration.RSS_INSTRUMENT,
            *declaration.SCAN,
            *declaration.INTERIOR,
            *declaration.STATISTICS,
            *declaration.DECODE,
            *declaration.DATAFUSION,
            *declaration.QUERY_CLI,
        ):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)

    def test_the_term_kinds_are_the_generators_last_three(self):
        self.assertEqual(inputs.LONG_LINE_TERM_KINDS, self.TERMS)
        self.assertEqual(long_lines.KINDS[-3:], self.TERMS)
        # Every kind reads the bound: none along the ladder alone.
        self.assertFalse(test_support.harness_has("LONG_LINE_LADDER_ONLY"))

    def test_an_input_a_kind_and_a_length_each_a_cluster(self):
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        lengths = dict(inputs.LONG_LINE_LENGTHS)
        self.assertEqual(fig.warm_inputs, inputs.LONG_LINE_INPUTS)
        # Every length of every kind.
        self.assertEqual(len(fig.warm_inputs), 6 * len(lengths))
        for kind in long_lines.KINDS:
            for length in inputs.long_line_lengths(kind):
                name = inputs.long_line_input(kind, length)
                with self.subTest(input=name):
                    spec = inputs.INPUTS[name]
                    self.assertEqual(spec.generator, "generate_long_lines.py")
                    self.assertFalse(spec.scales)
                    args = dict(zip(spec.args[::2], spec.args[1::2]))
                    self.assertEqual(args["--kind"], kind)
                    self.assertEqual(args["--line-bytes"], str(lengths[length]))
                    self.assertEqual(args["--lines"], str(inputs.LONG_LINE_CLUSTER))
                    self.assertGreater(
                        spec.nominal_bytes, inputs.LONG_LINE_CLUSTER * lengths[length]
                    )
                    long_lines.field_bytes(kind, lengths[length])
        for kind in self.TERMS:
            with self.subTest(kind=kind):
                self.assertIn("bound", inputs.long_line_lengths(kind))

    def test_the_lengths_are_the_ladder_one_off_it_and_the_bound(self):
        lengths = dict(inputs.LONG_LINE_LENGTHS)
        mib = 1 << 20
        # The ladder a factor of four apart, the secant over its two longest.
        self.assertEqual([lengths[t] for t in ("32", "128", "512")], [32 * mib, 128 * mib, 512 * mib])
        self.assertEqual(inputs.LONG_LINE_SECANT, ("128", "512"))
        # Off the ladder: just past a power of two, under the longest.
        off = lengths["288"]
        self.assertGreater(off, 256 * mib)
        self.assertLess(off, 512 * mib)
        self.assertNotEqual(off & (off - 1), 0)
        # The longest is the bound itself, read at one worker alone.
        self.assertEqual(lengths["bound"], inputs.LONG_LINE_BOUND)
        self.assertEqual(max(lengths.values()), inputs.LONG_LINE_BOUND)
        self.assertEqual(inputs.LONG_LINE_SERIAL_ONLY, ("bound",))
        self.assertEqual(shapes.long_line_jobs("text", "bound"), (1,))
        for length in ("32", "128", "288", "512"):
            self.assertEqual(shapes.long_line_jobs("text", length), shapes.LONG_LINE_JOBS)
            for kind in self.TERMS:
                self.assertEqual(shapes.long_line_jobs(kind, length), (1,))
        # Row order is length order.
        self.assertEqual(sorted(lengths.values()), [line for _, line in inputs.LONG_LINE_LENGTHS])
        with self.assertRaises(ValueError):
            inputs.long_line_input("text", "64")
        # A term's kind reads the bound at one partition, as every kind does.
        for kind in self.TERMS:
            self.assertEqual(shapes.long_line_jobs(kind, "bound"), (1,))

    def test_a_term_is_read_by_the_one_pass_holding_it(self):
        for kind in ("text", "escaped", "bytea"):
            self.assertEqual(shapes.long_line_passes(kind), ("parse", "strict", "query"))
        self.assertEqual(shapes.long_line_passes("int8array"), ("query",))
        self.assertEqual(shapes.long_line_passes("textarray"), ("query",))
        self.assertEqual(shapes.long_line_passes("jsonb"), ("filter",))
        self.assertEqual(shapes.LONG_LINE_PLANNED, ("query", "filter"))
        with self.assertRaises(ValueError):
            shapes.long_line_passes("json")

    def test_the_cluster_is_the_widest_count(self):
        self.assertEqual(inputs.LONG_LINE_CLUSTER, max(shapes.LONG_LINE_JOBS))
        self.assertEqual(shapes.LONG_LINE_JOBS, (1, 4))

    def test_the_bound_is_i71s(self):
        # `MaxAllocSize - 1`, `MaxAllocSize` being `0x3fffffff`.
        self.assertEqual(inputs.LONG_LINE_BOUND, 0x3FFFFFFF - 1)

    def test_the_allowance_holds_the_widest_legs_lines_at_their_charge(self):
        mib = 1 << 20
        # The largest factor's kind, one line at the bound at one partition,
        # at its line and its factor: more than four lines of a query's at
        # once, or its own 512 MiB line.
        widest = (1 + self.FACTORS["textarray"]) * inputs.LONG_LINE_BOUND
        self.assertGreater(widest, 4 * 2 * 512 * mib)
        self.assertEqual(
            shapes.long_line_allowance(self.FACTORS),
            memory_model.stated_allowance(shapes.PARALLEL_BUDGET + widest),
        )
        # Where every factor is small, four lines of the text query at four
        # partitions, each at its line and its factor of one.
        small = dict.fromkeys(self.FACTORS, Fraction(0)) | {"text": Fraction(1)}
        self.assertEqual(
            shapes.long_line_allowance(small),
            memory_model.stated_allowance(shapes.PARALLEL_BUDGET + 4 * 2 * 512 * mib),
        )
        # A parse's charge is its line alone: no factor, four lines.
        none = dict.fromkeys(self.FACTORS, Fraction(0))
        self.assertEqual(
            shapes.long_line_allowance(none),
            memory_model.stated_allowance(shapes.PARALLEL_BUDGET + 4 * 512 * mib),
        )
        # `int8array`'s line at the bound, at its factor, where it binds.
        bound = self.FACTORS | {"int8array": Fraction(11)}
        self.assertEqual(
            shapes.long_line_allowance(bound),
            memory_model.stated_allowance(shapes.PARALLEL_BUDGET + 12 * inputs.LONG_LINE_BOUND),
        )
        # A planned kind whose factor was not read is refused.
        with self.assertRaises(ValueError):
            shapes.long_line_allowance({k: f for k, f in self.FACTORS.items() if k != "jsonb"})
        self.assertFalse(test_support.harness_has("LONG_LINE_WHOLE_VALUE_CHARGE"))

    def test_a_planned_factor_is_read_as_the_instrument_writes_it(self):
        self.assertEqual(long_line.parse_planned_factor("161/16"), Fraction(161, 16))
        self.assertEqual(long_line.parse_planned_factor("none"), Fraction(0))
        for bad in ("", "1", "1/0", "a/b", "-1/2"):
            with self.subTest(text=bad):
                with self.assertRaises(ValueError):
                    long_line.parse_planned_factor(bad)
        # The key the instrument writes it under, which its own test pins.
        source = (config.REPO / "crates/pgdt/src/introspect.rs").read_text()
        self.assertIn('"planned_factor=none\\n"', source)
        self.assertIn('format!("planned_factor={held}/{per}\\n")', source)

    def test_a_kind_is_its_sweeps_each_subtraction_inside_one(self):
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        self.assertEqual(fig.warm_groups, register.long_line_warm_groups())
        for kind in long_lines.KINDS:
            groups = register.long_line_groups(kind)
            name = lambda length: inputs.long_line_input(kind, length)  # noqa: E731
            want = {
                frozenset((name("128"), name("512"))),
                frozenset((name("128"), name("32"))),
                frozenset((name("128"), name("288"))),
            }
            with self.subTest(kind=kind):
                self.assertEqual(groups, (("32", "128", "288", "512"), ("512", "bound")))
                want.add(frozenset((name("512"), name("bound"))))
                for group in groups:
                    names = tuple(name(length) for length in group)
                    self.assertIn(names, fig.warm_groups)
                pairs = {
                    frozenset(s.inputs)
                    for s in fig.subtracts
                    if all(n.startswith(f"long_{kind}_") for n in s.inputs)
                }
                self.assertEqual(pairs, want)
                for a, b in pairs:
                    register.subtraction_sweep(self.FIGURE, a, b)
        with self.assertRaises(ValueError):
            register.long_line_groups("json")

    def test_each_sweep_fits_the_tmpfs_budget(self):
        # The bound's cluster beside the secant's longer end is the largest.
        cfg = config.Config()
        for group in register.SELECTABLE_BY_ID[self.FIGURE].staging_groups:
            with self.subTest(group=group):
                self.assertLessEqual(
                    sum(staging.nominal_size(cfg, name) for name in group), staging.warm_bound(cfg)
                )

    def test_each_parse_states_its_count_allowance_and_request(self):
        for name in ("parse", "strict"):
            for jobs in shapes.LONG_LINE_JOBS:
                with self.subTest(name=name, jobs=jobs):
                    script = shapes._script(f"{shapes.LONG_RSS_FAMILY}{name}-{jobs}")
                    self.assertTrue(script.startswith(f"time {parsing.PEAK_RSS} /pgdt parse "))
                    self.assertIn(f"--jobs {jobs} ", script)
                    self.assertIn(f"--memory {shapes.ALLOWANCE_SLOT} ", script)
                    self.assertNotIn("--max-line-bytes", script)
                    self.assertIn(shapes.GATHER_STATISTICS, script)
                    self.assertEqual(
                        "--postgres-invalid-values strict" in script, name == "strict"
                    )

    def test_the_strict_parse_differs_by_its_flag_alone(self):
        for jobs in shapes.LONG_LINE_JOBS:
            with self.subTest(jobs=jobs):
                default = shapes._script(f"{shapes.LONG_RSS_FAMILY}parse-{jobs}")
                strict = shapes._script(f"{shapes.LONG_RSS_FAMILY}strict-{jobs}")
                self.assertEqual(
                    strict.replace(" --postgres-invalid-values strict", ""), default
                )

    def test_the_query_is_the_providers_under_the_wrapper_after_an_untimed_builder(self):
        for jobs in shapes.LONG_LINE_JOBS:
            with self.subTest(jobs=jobs):
                script = shapes._script(f"{shapes.LONG_RSS_FAMILY}query-{jobs}")
                builder, _, timed = script.partition(" && time ")
                self.assertIn("/pgdt parse", builder)
                self.assertIn(f"--jobs {shapes.SWEEP_JOBS} ", builder)
                self.assertNotIn("--max-line-bytes", builder)
                self.assertNotIn(parsing.PEAK_RSS, builder)
                self.assertTrue(
                    timed.startswith(
                        f"{shapes.DFCLI_PARTITIONS}={jobs} {parsing.PEAK_RSS} {shapes.SQL_SHELL} "
                    )
                )
                self.assertEqual(shapes._dfcli_partitions(script), {str(jobs)})
                self.assertIn(f"SET pgdump.memory = {shapes.ALLOWANCE_SLOT}", timed)
                self.assertNotIn("max_line_bytes", timed)
                self.assertIn(shapes.LONG_LINE_SQL, timed)
                self.assertTrue(script.endswith(shapes.DFCLI_ANSWER))

    def test_the_filter_is_pgdt_query_s_projecting_id_under_the_wrapper(self):
        for jobs in shapes.LONG_LINE_JOBS:
            with self.subTest(jobs=jobs):
                script = shapes._script(f"{shapes.LONG_RSS_FAMILY}filter-{jobs}")
                builder, _, timed = script.partition(" && time ")
                self.assertIn("/pgdt parse", builder)
                self.assertNotIn(parsing.PEAK_RSS, builder)
                self.assertTrue(timed.startswith(f"{parsing.PEAK_RSS} /pgdt query "))
                self.assertIn(f"--filter '{shapes.LONG_LINE_FILTER}'", timed)
                self.assertIn(" --column id ", timed)
                self.assertIn(f"--jobs {jobs} ", timed)
                self.assertIn(f"--memory {shapes.ALLOWANCE_SLOT} ", timed)
                self.assertNotIn("--max-line-bytes", timed)
        # An ordering term over the long column, which every row passes.
        self.assertEqual(shapes.LONG_LINE_FILTER, "v>{}")

    def test_an_instrument_leg_is_its_pass_at_the_sweep_count_with_no_allowance(self):
        for name in shapes.LONG_LINE_PLANNED:
            with self.subTest(name=name):
                factor = shapes._script(f"{shapes.LONG_FACTOR_FAMILY}{name}")
                timed = shapes._script(f"{shapes.LONG_RSS_FAMILY}{name}-{shapes.SWEEP_JOBS}")
                self.assertNotIn(shapes.ALLOWANCE_SLOT, factor)
                self.assertNotIn("--memory", factor)
                self.assertNotIn(parsing.PEAK_RSS, factor)
                # The timed leg's shape, its wrapper and its allowance gone.
                self.assertEqual(
                    timed.replace(f"{parsing.PEAK_RSS} ", "")
                    .replace(f"--memory {shapes.ALLOWANCE_SLOT} ", "")
                    .replace(f'-c "SET pgdump.memory = {shapes.ALLOWANCE_SLOT}" ', "")
                    .replace(f"-c 'SET pgdump.memory = {shapes.ALLOWANCE_SLOT}' ", ""),
                    factor,
                )
        with self.assertRaises(ValueError):
            shapes._script(f"{shapes.LONG_FACTOR_FAMILY}parse")

    def test_the_allowance_reaches_a_leg_only_through_its_spec(self):
        script = shapes._script(f"{shapes.LONG_RSS_FAMILY}parse-1")
        spec = runspec.RunSpec("pgdt", "long_text_32", f"{shapes.LONG_RSS_FAMILY}parse-1", "warm-parallel", "x")
        with self.assertRaises(ValueError):
            timing.with_allowance(script, spec)
        stated = dataclasses.replace(spec, allowance=12345)
        self.assertIn("--memory 12345 ", timing.with_allowance(script, stated))
        self.assertNotIn(shapes.ALLOWANCE_SLOT, timing.with_allowance(script, stated))
        with self.assertRaises(ValueError):
            timing.with_allowance("time /pgdt parse --jobs 1", stated)
        self.assertEqual(timing.with_allowance("time /pgdt parse", spec), "time /pgdt parse")
        # Two allowances are two readings.
        self.assertNotEqual(spec.key(self.FIGURE), stated.key(self.FIGURE))
        self.assertTrue(stated.key(self.FIGURE).endswith("/allowance=12345"))

    def test_every_leg_is_a_shape_and_reads_resident(self):
        specs = long_line._long_line_specs(12345)
        passes = len(("parse", "strict", "query"))
        # A kind of three passes: four lengths at both counts, then the bound
        # and its anchor at one; a term's kind, its one pass at one partition,
        # four lengths, then the bound and its anchor.
        self.assertEqual(len(specs), 3 * passes * (4 * 2 + 2) + 3 * (4 + 2))
        for spec in specs:
            with self.subTest(leg=spec.label):
                self.assertIn(spec.command, shapes.command_shapes())
                # `time_run` reads a peak only off a shape whose name says so.
                self.assertIn("rss", spec.command)
                self.assertEqual(spec.binary, "dfcli" if "-query-" in spec.command else "pgdt")
                self.assertEqual(spec.allowance, 12345)
                self.assertFalse(spec.instrument)
                timing.with_allowance(shapes._script(spec.command), spec)
        self.assertEqual(len({s.key(self.FIGURE) for s in specs}), len(specs))
        self.assertEqual(len({s.label for s in specs}), len(specs))

    def test_each_planned_kind_has_one_instrument_leg_in_its_ladder_sweep(self):
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        specs = long_line._long_factor_specs()
        self.assertEqual(
            [(s.input, s.command) for s in specs],
            [
                (
                    inputs.long_line_input(kind, long_line.LONG_LINE_FACTOR_LENGTH),
                    f"{shapes.LONG_FACTOR_FAMILY}{shapes.long_line_passes(kind)[-1]}",
                )
                for kind in long_lines.KINDS
            ],
        )
        placed = {
            spec.key(self.FIGURE): gi
            for gi, part in staging.split_specs(self.FIGURE, specs, fig.staging_groups)
            for spec in part
        }
        for spec in specs:
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.binary, "introspect")
                self.assertTrue(spec.instrument)
                self.assertIsNone(spec.allowance)
                self.assertIsNone(spec.sweep)
                self.assertIn(spec.command, shapes.command_shapes())
                self.assertNotIn("rss", spec.command)
                kind = spec.input.split("_")[1]
                self.assertEqual(
                    fig.staging_groups[placed[spec.key(self.FIGURE)]][0],
                    inputs.long_line_input(kind, "32"),
                )

    def test_the_bound_and_its_anchor_are_read_at_one_worker_in_their_own_sweep(self):
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        specs = long_line._long_line_specs(12345)
        placed = dict(
            (spec.key(self.FIGURE), gi)
            for gi, part in staging.split_specs(self.FIGURE, specs, fig.staging_groups)
            for spec in part
        )
        for kind in ("text", "escaped", "bytea"):
            bound = inputs.long_line_input(kind, "bound")
            anchor = inputs.long_line_input(kind, "512")
            serial_group = fig.staging_groups.index((anchor, bound))
            with self.subTest(kind=kind):
                serial = [s for s in specs if s.input == bound or s.sweep is not None and s.input == anchor]
                self.assertEqual(len(serial), 2 * 3)
                for spec in serial:
                    self.assertTrue(spec.command.endswith("-1"), spec.command)
                    self.assertEqual(placed[spec.key(self.FIGURE)], serial_group)
                    self.assertEqual(spec.sweep, None if spec.input == bound else serial_group)
                # The 512 MiB input's own legs stay in the ladder's sweep.
                ladder = [s for s in specs if s.input == anchor and s.sweep is None]
                self.assertEqual(len(ladder), 3 * 2)
                for spec in ladder:
                    self.assertEqual(placed[spec.key(self.FIGURE)], serial_group - 1)

    def test_a_count_or_pass_the_figure_does_not_carry_is_refused(self):
        for command in ("long-rss-parse-24", "long-rss-merge-1", "long-rss-query-x", "long-rss-filter-2"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_the_answer_is_every_row_of_the_input(self):
        for name in inputs.LONG_LINE_INPUTS:
            with self.subTest(input=name):
                self.assertEqual(
                    inputs.long_line_answer(name),
                    2 * long_lines.SHORT_ROWS + inputs.LONG_LINE_CLUSTER,
                )
        with self.assertRaises(ValueError):
            inputs.long_line_answer("control")
        with self.assertRaises(ValueError):
            inputs.long_line_answer("long_textarray_1024")

    def test_a_factor_leg_naming_none_or_answering_short_is_refused(self):
        specs = long_line._long_factor_specs()

        class Reports:
            def __init__(self, factor: str | None, rows: int) -> None:
                self.reported = {
                    s.key(LongLineResident.FIGURE): {"result_first": str(rows), "rows_returned": str(rows)}
                    for s in specs
                }
                self.factor = factor

            def instrument_reports(self, figure, spec):
                return [{} if self.factor is None else {"planned_factor": self.factor}]

        rows = long_lines.row_count(inputs.LONG_LINE_CLUSTER)
        read = long_line.long_line_factors(Reports("161/16", rows), specs)
        self.assertEqual(read, dict.fromkeys(long_lines.KINDS, Fraction(161, 16)))
        with self.assertRaises(RuntimeError):
            long_line.long_line_factors(Reports(None, rows), specs)
        with self.assertRaises(RuntimeError):
            long_line.long_line_factors(Reports("1/1", rows - 1), specs)

    def test_the_secant_and_its_residuals(self):
        mib = 1 << 20

        def kib(line: int, fixed: float = 50 * 1024) -> list[float]:
            # Resident a fixed 50 MiB plus three bytes a line byte, in KiB.
            return [fixed + 3 * line / 1024] * 3

        lo, hi = 128 * mib, 512 * mib
        per_byte = long_line.long_line_secant(kib(lo), kib(hi), hi - lo)
        self.assertAlmostEqual(per_byte, 3.0)
        # Below the secant, past it, and anchored in another sweep whose
        # level differs: no residual where resident is linear in the line.
        for at in (32 * mib, 288 * mib, inputs.LONG_LINE_BOUND):
            with self.subTest(at=at):
                self.assertAlmostEqual(
                    long_line.long_line_residual(per_byte, kib(lo), kib(at), at - lo), 0.0, places=6
                )
        shifted = 60 * 1024
        self.assertAlmostEqual(
            long_line.long_line_residual(
                per_byte, kib(hi, shifted), kib(inputs.LONG_LINE_BOUND, shifted),
                inputs.LONG_LINE_BOUND - hi,
            ),
            0.0,
            places=6,
        )
        # A length holding 10 MiB more than the line predicts.
        stepped = [v + 10 * 1024 for v in kib(288 * mib)]
        self.assertAlmostEqual(
            long_line.long_line_residual(per_byte, kib(lo), stepped, 288 * mib - lo),
            10 * 1024,
            places=6,
        )
        # The median, not a rep, is what is read.
        noisy = kib(hi)[:2] + [9999.0 * 1024]
        self.assertAlmostEqual(long_line.long_line_secant(kib(lo), noisy, hi - lo), 3.0)


if __name__ == "__main__":
    unittest.main()
