"""Tests of `measurement.memory_model`."""

from __future__ import annotations

import contextlib
import inspect
import io
import unittest
import unittest.mock

from measurement import check
from measurement import config
from measurement import inputs
from measurement import memory_model
from measurement import parsing
from measurement import runspec
from measurement import shapes
from measurement import test_support
from measurement import timing
from measurement.figures import parallel
from measurement.figures import register
from measurement.figures import reserve
from measurement.figures import throughput
from measurement.figures import xz_decode


class CompressedAccount(unittest.TestCase):
    """The three families `reserve` grew when the compressed path got an account.

    The figure publishes a **pair** beside its model check — a fixed term and a
    per-reader term — so every assertion here is a way to get a plausible pair
    of the wrong thing. Four fail silently and are the reason this class
    exists. A **flagless leg that states a flag** measures a stated arrangement
    under a heading that says discovered, which `worker_count_problems` cannot
    see because that family is exempt from it. Two **legs differing only in
    their container limit** would share a reading, the limit not being an argv
    fact — which is `_attribution_specs`' failure with a new cause. A **fit over
    one point** is an intercept asserted as a measurement, which is the
    extrapolation `.claude/skills/evidence/SKILL.md`'s second rule names. And a
    **mechanism leg at its
    own limit or block size** would read as a mechanism moving a term when what
    moved was the arrangement.
    """

    def _fig(self):
        return register.SELECTABLE_BY_ID["reserve"]

    # -- the flagless axis -------------------------------------------------

    def test_the_flagless_axis_runs_at_both_block_sizes(self):
        # A compressed reader's charge bills a unit, and the pool a unit a
        # reader past `POOL_DEPTH`, so a unit-shaped error in it is multiplied by
        # the reader count: a fit at one block size
        # cannot tell a term that scales with the unit from one that does not.
        self.assertEqual(
            [name for name, _, _ in shapes.RESERVE_FLAGLESS_INPUTS],
            ["control_xz", "control_xz128"],
        )
        for name, _, unit in shapes.RESERVE_FLAGLESS_INPUTS:
            with self.subTest(input=name):
                self.assertEqual(inputs.INPUTS[name].suffix, ".xz")
                self.assertIn(name, self._fig().warm_inputs)
                self.assertGreater(unit, 0)
        units = {unit for _, _, unit in shapes.RESERVE_FLAGLESS_INPUTS}
        self.assertEqual(len(units), len(shapes.RESERVE_FLAGLESS_INPUTS))

    def test_the_block_sizes_are_the_ones_the_other_compressed_figure_reads(self):
        # Two block sizes is a claim about files, not about this figure, so the
        # pair is the one `parallel-peak-rss` already publishes — a third size
        # here would be a second answer to the same question.
        self.assertEqual(
            [name for name, _, _ in shapes.RESERVE_FLAGLESS_INPUTS],
            [name for name, _ in parallel.PARALLEL_RSS_LEGS],
        )

    def test_the_flagless_shapes_state_neither_flag(self):
        # The exemption `_NO_FLAGS` opens is from *stating a count*; a shape
        # that quietly acquired one would still pass the count reconciliation,
        # being exempt, and would publish a stated arrangement as a discovered
        # one.
        self.assertEqual(shapes.flagless_flag_problems(), [])
        for token, _, _ in shapes.RESERVE_ARENAS:
            with self.subTest(arena=token):
                script = shapes._script(f"{shapes.RESERVE_FLAGLESS}{token}")
                self.assertNotIn("--jobs", script)
                self.assertNotIn("--memory", script)
                self.assertIn("/pgdt parse --source /dump.sql", script)

    def test_a_flagless_shape_that_states_a_flag_is_reported(self):
        # The check must fail loudly: the shape it would pass still runs and
        # still produces a row.
        with unittest.mock.patch.object(
            shapes,
            "_script",
            lambda c: "time /pgdt parse --source /dump.sql --jobs 4 --memory 99",
        ):
            reported = shapes.flagless_flag_problems()
        self.assertEqual(
            sorted(reported),
            sorted(
                f"{c} states --jobs 4, --memory 99"
                for c in shapes.command_shapes()
                if c.startswith(shapes.RESERVE_FLAGLESS)
            ),
        )

    def test_the_exemption_is_declared_at_both_ends(self):
        # Declared as its own prefix rather than appended to `_NO_WORKERS`,
        # whose rule is "not a run of ours" — this is exactly a run of ours,
        # stating nothing on purpose.
        self.assertIn(shapes.RESERVE_FLAGLESS, shapes._NO_FLAGS)
        self.assertNotIn(shapes.RESERVE_FLAGLESS, shapes._NO_WORKERS)
        self.assertEqual(shapes.worker_count_problems(), [])
        self.assertEqual(shapes.pinned_count_problems(), [])

    def test_the_limit_rides_on_the_spec_and_two_legs_never_share_a_reading(self):
        # The container limit is a `nerdctl run` argument, so it cannot be in
        # the command shape; these legs share one shape and are told apart by
        # `RunSpec.memory`, which `RunSpec.key` has to carry or they collapse
        # into one another's reps.
        specs = reserve._reserve_flagless_specs()
        self.assertEqual(
            len(specs),
            len(shapes.RESERVE_FLAGLESS_INPUTS) * len(shapes.RESERVE_LIMITS),
        )
        keys = [s.key("reserve") for s in specs]
        self.assertEqual(len(set(keys)), len(keys))
        tokens = {token for token, _ in shapes.RESERVE_LIMITS}
        for spec in specs:
            with self.subTest(leg=spec.label):
                self.assertIn(spec.memory, tokens)
                self.assertIn(f"m={spec.memory}", spec.key("reserve"))

    def test_a_spec_that_states_no_limit_keys_as_it_always_did(self):
        # Appended rather than always present, so a past sitting's `raw.json`
        # still renders: every spec outside this figure keys exactly as before.
        spec = runspec.RunSpec("pgdt", "control", "parse-rss", "warm", "x")
        self.assertEqual(spec.key("peak-rss"), "peak-rss/pgdt/control/parse-rss/warm")
        self.assertEqual(spec.memory, None)

    def test_nothing_outside_this_figure_states_a_per_spec_limit(self):
        # A per-spec limit exists because one figure's axis *is* the limit.
        # Anywhere else it would be a container departure no table states.
        others = [
            *reserve._reserve_specs(),
            *test_support._peak_rss_specs(),
            *parallel._parallel_rss_specs(),
            *parallel._parallel_specs(),
            *xz_decode._decode_specs(),
            *(spec for _, small, big in register._attribution_specs() for spec in (small, big)),
            register._RESERVE_BASELINE,
        ]
        for spec in others:
            with self.subTest(leg=spec.label):
                self.assertIsNone(spec.memory)

    def test_the_limits_span_the_curve_rather_than_its_worst_end(self):
        # 512 MiB is the bottom of the range, where the rule's own headroom is
        # worst; a set clustered there would fit a line through the worst end
        # alone.
        self.assertEqual(shapes.RESERVE_LIMITS[0], ("512m", 512 << 20))
        byte_values = [n for _, n in shapes.RESERVE_LIMITS]
        self.assertEqual(sorted(byte_values), byte_values)
        self.assertGreaterEqual(len(shapes.RESERVE_LIMITS), 3)

    def test_the_pool_term_is_billed_at_every_reader_count(self):
        # *Rejected:* `max(0, POOL_DEPTH - jobs) x unit`, which clamps to zero
        # at four readers or more, so only limits under the clamp evaluate it.
        # Stated as what the pool holds it is billed everywhere, and past
        # `POOL_DEPTH` it is the larger half of the charge — so there is no
        # window to miss.
        for _name, label, unit in shapes.RESERVE_FLAGLESS_INPUTS:
            with self.subTest(block_size=label):
                for jobs in (1, 2, 4, 5, 24):
                    self.assertEqual(
                        memory_model.pool_bytes(unit, jobs),
                        (max(memory_model.LIBRARY_POOL_DEPTH, jobs) - 1) * unit,
                    )
                    self.assertGreater(memory_model.pool_bytes(unit, jobs), 0)
                # And the charge is exactly one unit under what the two-unit
                # per-reader term plus a decaying floor billed, at every count.
                for jobs in (1, 2, 3, 4, 9, 24):
                    was = jobs * (memory_model.reader_bytes(unit) + unit) + max(
                        0, memory_model.LIBRARY_POOL_DEPTH - jobs
                    ) * unit
                    self.assertEqual(memory_model.charge_bytes(unit, jobs), was - unit)

    def test_the_registered_axis_can_reach_three_distinct_reader_counts(self):
        # Fit-ability is asked of the same registered limits, because an
        # axis that can only ever publish a secant should fail before a sitting
        # is spent rather than after. Necessary and not sufficient — a host with
        # few cores collapses distinct fits onto one count, which is the
        # per-sitting residue the secant covers.
        self.assertEqual(reserve.reserve_axis_problems(), [])
        for _name, label, unit in shapes.RESERVE_FLAGLESS_INPUTS:
            with self.subTest(block_size=label):
                counts = {
                    memory_model.afforded_readers(unit, memory_model.discovered_budget(limit))
                    for _, limit in shapes.RESERVE_LIMITS
                } - {0}
                self.assertGreaterEqual(len(counts), shapes.RESERVE_FIT_MIN_COUNTS)

    def test_the_check_fires_where_the_axis_cannot_be_fitted(self):
        # The two-sided half of the fit-ability check: dropping the limits that
        # afford the 128 MiB family its distinct counts has to turn it red, and
        # the line has to say which counts are left.
        kept = tuple(
            row for row in shapes.RESERVE_LIMITS if row[0] not in ("1536m", "2g")
        )
        with unittest.mock.patch.object(shapes, "RESERVE_LIMITS", kept):
            problems = reserve.reserve_axis_problems()
        fitness = [line for line in problems if "distinct reader count(s)" in line]
        self.assertEqual(len(fitness), 1)
        self.assertIn("128 MiB blocks", fitness[0])
        self.assertIn(f"under the {shapes.RESERVE_FIT_MIN_COUNTS}", fitness[0])

    def test_check_fails_where_the_axis_cannot_be_fitted(self):
        # Wired into `--check`, not only available to be called: the whole point
        # is that an axis that can only ever publish a secant should fail before
        # a sitting is spent rather than after.
        kept = tuple(
            row for row in shapes.RESERVE_LIMITS if row[0] not in ("1536m", "2g")
        )
        with unittest.mock.patch.object(shapes, "RESERVE_LIMITS", kept):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = check.cmd_check(config.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("distinct reader counts a two-term fit needs", out.getvalue())
        self.assertIn("distinct reader count(s)", out.getvalue())

    def test_check_fails_where_the_axis_cannot_be_fitted(self):
        # The same wiring for the fit-ability half: one refusal, two questions,
        # and `--check` has to carry both or the second is a function nobody
        # calls.
        kept = tuple(
            row for row in shapes.RESERVE_LIMITS if row[0] not in ("1536m", "2g")
        )
        with unittest.mock.patch.object(shapes, "RESERVE_LIMITS", kept):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = check.cmd_check(config.REPO / "docs/design/measurements.md")
        self.assertEqual(code, 1)
        self.assertIn("distinct reader count(s)", out.getvalue())

    def test_a_stated_allowance_leaves_the_budget_the_figure_registered(self):
        # `--memory` states resident, so a figure registered against a buffer
        # budget has to state that budget plus the reserve or it measures a
        # different apparatus under the same heading.
        for budget in (*shapes.RESERVE_BUDGETS, *shapes.RESERVE_STEP_BUDGETS,
                       shapes.PARALLEL_BUDGET):
            with self.subTest(budget=budget):
                allowance = memory_model.stated_allowance(budget)
                self.assertEqual(
                    allowance - memory_model.LIBRARY_MEMORY_RESERVE,
                    budget,
                    "the carve's cap has to land back on the registered budget",
                )
        # And the step pair stays one byte apart, which is the whole of what
        # that family measures.
        step = [memory_model.stated_allowance(b) for b in shapes.RESERVE_STEP_BUDGETS]
        self.assertEqual(abs(step[0] - step[1]), 1)

    def test_the_discovered_budget_is_the_library_rule_mirrored(self):
        # `limit.bytes.saturating_sub(MEMORY_RESERVE)`, read off the source
        # rather than trusted, since the whole point of the check above is that
        # it reasons in the library's own arithmetic.
        src = test_support.read_path_source()
        self.assertIn("allowance.saturating_sub(MEMORY_RESERVE)", src)
        self.assertEqual(
            memory_model.discovered_budget(512 << 20), (512 << 20) - memory_model.LIBRARY_MEMORY_RESERVE
        )
        # Saturating, not negative: a container at or under the reserve.
        self.assertEqual(memory_model.discovered_budget(memory_model.LIBRARY_MEMORY_RESERVE), 0)
        self.assertEqual(memory_model.discovered_budget(0), 0)

    def test_every_flagless_leg_is_read_under_the_uncapped_arena(self):
        # The shipped constant comes from the operator who capped nothing; the
        # capped arrangement is a mechanism leg, not the axis.
        shape = f"{shapes.RESERVE_FLAGLESS}{reserve.RESERVE_UNCAPPED}"
        for spec in reserve._reserve_flagless_specs():
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.command, shape)
        self.assertNotIn("MALLOC_ARENA_MAX", shapes._script(shape))

    # -- the mechanism legs ------------------------------------------------

    def test_the_mechanism_legs_are_one_block_size_and_one_allocation(self):
        # Crossing them with the limits and the block sizes buys a second cross
        # of an expensive axis for no question anybody asked.
        legs = reserve._reserve_mechanism_specs()
        self.assertEqual(len(legs), 1)
        for label, spec in legs:
            with self.subTest(leg=label):
                self.assertEqual(spec.input, shapes.RESERVE_MECHANISM_INPUT)
                self.assertEqual(spec.memory, reserve.RESERVE_MECHANISM_LIMIT)
        self.assertIn(
            reserve.RESERVE_MECHANISM_LIMIT, [token for token, _ in shapes.RESERVE_LIMITS]
        )
        self.assertIn(
            shapes.RESERVE_MECHANISM_INPUT,
            [name for name, _, _ in shapes.RESERVE_FLAGLESS_INPUTS],
        )

    def test_the_reference_is_a_leg_of_the_axis_rather_than_a_re_take(self):
        # Each mechanism leg is one reading, compared against the flagless leg
        # at the same input and limit — which the axis above already measures,
        # so nothing here is measured twice.
        reference = runspec.RunSpec(
            "pgdt",
            shapes.RESERVE_MECHANISM_INPUT,
            f"{shapes.RESERVE_FLAGLESS}{reserve.RESERVE_UNCAPPED}",
            "warm-parallel",
            "",
            memory=reserve.RESERVE_MECHANISM_LIMIT,
        )
        keys = [s.key("reserve") for s in reserve._reserve_flagless_specs()]
        self.assertIn(reference.key("reserve"), keys)

    def test_no_leg_of_this_figure_swaps_the_allocator(self):
        # The two allocator legs are **dropped, not re-aimed**: jemalloc and
        # mimalloc do not have glibc's dynamic mmap threshold, so swapping them
        # removes the mechanism under test instead of measuring it. A later
        # session re-adding one would be re-running the sitting whose
        # foreseeable outcome was "the three legs could not attribute it".
        every = [
            *reserve._reserve_flagless_specs(),
            *reserve._reserve_instrument_specs(),
            *reserve._reserve_diagnostic_specs(),
            *(s for _, s in reserve._reserve_mechanism_specs()),
            *reserve._reserve_step_specs(),
            *reserve._reserve_specs(),
            register._RESERVE_BASELINE,
        ]
        for spec in every:
            with self.subTest(leg=spec.label):
                self.assertFalse(spec.binary.startswith("alloc:"))

    def test_the_instrument_legs_are_the_flagless_shape_on_the_instrument_build(self):
        # The attribution and the check must be the *same arrangement* measured
        # two ways, so the shape is the flagless one and the only difference is
        # the binary — which `key` carries, so neither can be read as a rep of
        # the other.
        flagless = {s.key("reserve"): s for s in reserve._reserve_flagless_specs()}
        legs = reserve._reserve_instrument_specs()
        self.assertEqual(len(legs), len(reserve.RESERVE_INSTRUMENT_LIMITS) + 1)
        for spec in legs:
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.binary, "introspect")
                self.assertTrue(spec.instrument)
                self.assertEqual(spec.input, shapes.RESERVE_MECHANISM_INPUT)
                self.assertTrue(spec.command.startswith(shapes.RESERVE_FLAGLESS))
                self.assertNotIn(spec.key("reserve"), flagless)
        # Every limit of the axis, so the instrument's decomposition can be read
        # against the black-box fit leg for leg rather than at one cell.
        uncapped = [s for s in legs if s.command == reserve._flagless_shape()]
        self.assertEqual(
            [s.memory for s in uncapped], list(reserve.RESERVE_INSTRUMENT_LIMITS)
        )
        # And the capped leg, at the same pair the mechanism row takes, so the
        # black-box delta and the introspective one describe one arrangement.
        capped = [s for s in legs if s.command == reserve._flagless_shape(reserve.RESERVE_CAPPED)]
        self.assertEqual([s.memory for s in capped], [reserve.RESERVE_MECHANISM_LIMIT])

    def test_the_diagnostic_legs_are_the_failing_arrangement_on_the_instrument_build(self):
        # The arrangement the gate failed on the shipped build alone, and the
        # `bound`-band cell beside it: one block size, the flagless shape, and
        # limits the black-box axis already reads, so each has a shipped leg of
        # the same input and limit to be checked against.
        flagless = {(s.input, s.memory): s for s in reserve._reserve_flagless_specs()}
        legs = reserve._reserve_diagnostic_specs()
        self.assertEqual([s.memory for s in legs], list(reserve.RESERVE_DIAGNOSTIC_LIMITS))
        unit = dict((name, unit) for name, _, unit in shapes.RESERVE_FLAGLESS_INPUTS)[
            reserve.RESERVE_DIAGNOSTIC_INPUT
        ]
        self.assertNotEqual(unit, shapes.RESERVE_MECHANISM_UNIT)
        for spec in legs:
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.binary, "introspect")
                self.assertTrue(spec.instrument)
                self.assertTrue(timing.kill_tolerant(spec.command))
                self.assertIn("128 MiB blocks", spec.label)
                shipped = flagless[(spec.input, spec.memory)]
                self.assertEqual(spec.command, shipped.command)
                self.assertNotEqual(spec.key("reserve"), shipped.key("reserve"))
        # A second family, never the counter's: its line is a fit at one unit.
        self.assertFalse(
            {s.key("reserve") for s in legs}
            & {s.key("reserve") for s in reserve._reserve_instrument_specs()}
        )

    def test_an_instrument_leg_is_kill_tolerant_like_the_axis_it_mirrors(self):
        # It runs the arrangement the rule aims *at* the allocation, so it sits
        # against its own ceiling exactly as the flagless axis does — and the
        # report is written at exit, so a kill is also the one legitimate
        # absence of the report `RunSpec.instrument` otherwise makes an error.
        for spec in reserve._reserve_instrument_specs():
            with self.subTest(leg=spec.label):
                self.assertTrue(timing.kill_tolerant(spec.command))

    def test_the_arena_leg_is_the_flagless_shape_with_the_cap_and_nothing_else(self):
        # One mechanism at a time: the capped leg differs from the reference by
        # the environment the wrapper starts pgdt in and by nothing on the command
        # line.
        arena = next(
            label for token, _, label in shapes.RESERVE_ARENAS if token == reserve.RESERVE_CAPPED
        )
        spec = dict(reserve._reserve_mechanism_specs())[arena]
        self.assertEqual(spec.binary, "pgdt")
        script = shapes._script(spec.command)
        self.assertIn(f"time MALLOC_ARENA_MAX=2 {parsing.PEAK_RSS} /pgdt", script)
        self.assertNotIn("--jobs", script)
        self.assertNotIn("--memory", script)

    # -- the path step -----------------------------------------------------

    def test_the_step_is_one_byte_either_side_of_what_a_reader_costs(self):
        # `block_path_afforded` is the comparison, so the pair straddles it and
        # differs in nothing else. A wider gap would be a budget change with a
        # path change inside it — and a pair straddling `reader_bytes` instead
        # would run the streaming decoder on *both* legs, one reader's share of
        # the pool's retention list being inside `BlockCache::affordable`.
        afforded, declined = shapes.RESERVE_STEP_BUDGETS
        self.assertEqual(afforded, memory_model.charge_bytes(shapes.RESERVE_MECHANISM_UNIT, 1))
        self.assertEqual(declined, afforded - 1)
        self.assertTrue(memory_model.block_path_afforded(shapes.RESERVE_MECHANISM_UNIT, afforded))
        self.assertFalse(memory_model.block_path_afforded(shapes.RESERVE_MECHANISM_UNIT, declined))
        self.assertGreater(afforded, memory_model.reader_bytes(shapes.RESERVE_MECHANISM_UNIT))
        for spec in reserve._reserve_step_specs():
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.input, shapes.RESERVE_MECHANISM_INPUT)
                self.assertEqual(spec.memory, reserve.RESERVE_MECHANISM_LIMIT)
                self.assertIn(f"--jobs {shapes.RESERVE_JOBS}", shapes._script(spec.command))

    def test_the_step_budgets_fit_inside_the_allocation_they_run_in(self):
        # A stated budget above the container's own limit would measure the
        # container, not the path.
        limit = dict(shapes.RESERVE_LIMITS)[reserve.RESERVE_MECHANISM_LIMIT]
        for budget in shapes.RESERVE_STEP_BUDGETS:
            with self.subTest(budget=budget):
                self.assertLess(budget, limit)

    def test_a_step_budget_the_figure_does_not_carry_is_an_error(self):
        with self.assertRaises(ValueError):
            shapes._script(f"{shapes.RESERVE_STEP_FAMILY}99")

    # -- the reader charge, mirrored ---------------------------------------

    def test_the_reader_charge_is_the_librarys_own_three_terms(self):
        # Hand-computed on `QUERY_SUBSTREAM_CAP`'s argument, so the mirror is
        # checked here rather than trusted: one block slot — the block being
        # decoded, which is the block then retained — the chunk buffer
        # and the decoder's retention, which is 34.03 MiB at koji's block size.
        self.assertEqual(memory_model.reader_bytes(24 << 20), 35_686_176)
        self.assertEqual(
            memory_model.reader_bytes(128 << 20),
            (128 << 20) + memory_model.LIBRARY_CHUNK_BYTES + memory_model.XZ_DECODE_FOOTPRINT,
        )
        self.assertEqual(memory_model.LIBRARY_CHUNK_BYTES, throughput.CHUNK_DEFAULT)

    def test_the_mirrored_chunk_size_is_the_librarys_default(self):
        # The one term of the three that is a library constant rather than a
        # property of the file or of the decoder.
        src = test_support.read_path_source()
        self.assertIn(
            f"pub const SCAN_CHUNK_DEFAULT_SIZE_BYTES: usize = {memory_model.LIBRARY_CHUNK_BYTES >> 20} << 20;",
            src,
        )

    def test_the_mechanism_unit_is_read_off_the_input_registry(self):
        # Written twice, the unit the step's budgets are computed from and the
        # unit the flagless table prints would be free to disagree.
        self.assertEqual(
            shapes.RESERVE_MECHANISM_UNIT,
            next(
                unit
                for name, _, unit in shapes.RESERVE_FLAGLESS_INPUTS
                if name == shapes.RESERVE_MECHANISM_INPUT
            ),
        )

    # -- the block-path line, mirrored -------------------------------------

    def test_the_path_line_is_the_charge_at_one_reader_not_the_reader_term(self):
        # `BlockCache::affordable` compares the budget against what one reader
        # *costs the rule* — the per-reader term plus that one reader's share of
        # the pool's retention list. `reader_bytes` alone is a different line,
        # and at both registered block sizes the two are far apart, so a mirror
        # comparing against it labels whole cells with the wrong mechanism
        # rather than getting an edge case wrong.
        for unit in (24 << 20, 128 << 20):
            with self.subTest(unit=unit):
                line = memory_model.charge_bytes(unit, 1)
                self.assertEqual(line, memory_model.reader_bytes(unit) + 3 * unit)
                self.assertTrue(memory_model.block_path_afforded(unit, line))
                self.assertFalse(memory_model.block_path_afforded(unit, line - 1))
                # The number the old mirror compared against is now well inside
                # the declined side.
                self.assertFalse(memory_model.block_path_afforded(unit, memory_model.reader_bytes(unit)))

    def test_the_path_line_is_the_comparison_the_library_makes(self):
        # Mirrored by hand on `QUERY_SUBSTREAM_CAP`'s argument, so the source is
        # read here rather than the arithmetic trusted: `affordable` asks
        # `worker_memory(..).at(1) <= budget`, and `worker_memory` is the
        # per-reader term `pooling`ed at `POOL_DEPTH`.
        src = test_support.read_path_source()
        self.assertIn(
            "self.worker_memory(chunk_bytes, decode_bytes).at(1) <= budget",
            src,
        )
        self.assertIn(".pooling(self.unit as u64, POOL_DEPTH)", src)
        self.assertIn(f"const POOL_DEPTH: usize = {memory_model.LIBRARY_POOL_DEPTH};", src)

    def test_the_renderer_asks_the_path_question_in_one_way_only(self):
        # Sites asking the question different ways make a table whose cells
        # silently change mechanism while reading as one series. A comparison of a budget
        # against `reader_bytes` anywhere in the renderer is that failure
        # returning.
        source = inspect.getsource(reserve.run_reserve)
        self.assertNotIn("budget >= reader_bytes", source)
        self.assertNotIn("budget < reader_bytes", source)
        self.assertNotIn("budget >= charge_bytes", source)
        self.assertNotIn("budget < charge_bytes", source)
        # Five: the instrument account's line drops a declined leg
        # by the same question, because a leg holding no block pool has no
        # retention list for `_depooled` to subtract.
        self.assertEqual(source.count("block_path_afforded("), 5)

    def test_the_smallest_allocation_splits_the_two_inputs_across_the_line(self):
        # The CLI grants
        # `limit - MEMORY_RESERVE`, so a `512m` container affords 128 MiB
        # against a line of 106.03 MiB at 24 MiB blocks and 522.03 at 128. The
        # bottom row of the axis therefore carries one cell of each path, which
        # is exactly the case a table printing them as one series gets wrong.
        token, limit = shapes.RESERVE_LIMITS[0]
        self.assertEqual(token, "512m")
        granted = limit - memory_model.LIBRARY_MEMORY_RESERVE
        paths = {
            label: memory_model.block_path_afforded(unit, granted)
            for _name, label, unit in shapes.RESERVE_FLAGLESS_INPUTS
        }
        self.assertEqual(paths, {"24 MiB blocks": True, "128 MiB blocks": False})

    # -- the charge model, and the cells it was seeded from ----------------

    #: Readings from the reserve constant's five-build grid, off
    #: `control_xz128` at a 384 MiB reserve (`docs/design/decisions.md`,
    #: "I/O, memory and parallelism"): reader count, worst rep in MiB, the
    #: budget the run reported under the charge that build carried, and the
    #: residual computed by hand against that charge.
    #:
    #: The model is *seeded* from them rather than fitted to them: every number
    #: in the middle two columns is arithmetic the harness does at the cell,
    #: and this is the check that it reproduces what was computed by hand — up
    #: to the one unit that build's charge double-counted, which is a statement
    #: the seeding makes rather than a reason to re-seed.
    SEED_UNIT = 128 << 20
    SEED_CELLS = (
        (2, 802.1, 532.1, 14.0),
        (3, 939.9, 798.1, 13.8),
        (4, 1077.7, 1064.1, 13.6),
        (5, 1342.6, 1330.2, 12.4),
        (6, 1607.5, 1596.2, 11.3),
    )

    def test_the_model_reproduces_the_readings_it_was_seeded_from(self):
        for jobs, worst, budget, residual in self.SEED_CELLS:
            with self.subTest(jobs=jobs):
                billed, pool, unnamed = memory_model.charge_model(
                    self.SEED_UNIT, jobs, worst * config.MIB
                )
                # The seed's budget column is what that build's rule resolved:
                # two units a reader, with the pool unbilled. Today's charge
                # bills the pool and one unit a reader, so today's bill is that
                # column plus the pool less one unit — and the residual computed
                # by hand rises by exactly that unit, which is the double count
                # seen from the readings rather than from the source.
                self.assertAlmostEqual(
                    (billed - pool + jobs * self.SEED_UNIT) / config.MIB, budget, places=1
                )
                self.assertEqual(pool, memory_model.pool_bytes(self.SEED_UNIT, jobs))
                self.assertAlmostEqual(
                    unnamed / config.MIB, residual + self.SEED_UNIT / config.MIB, places=1
                )
                self.assertIsNone(
                    memory_model.charge_model_problem(self.SEED_UNIT, jobs, worst * config.MIB)
                )

    def test_the_pool_term_is_named_rather_than_left_in_the_remainder(self):
        # The finding the check exists to surface, and what the charge bills: at
        # two readers of a 128 MiB block file the pool retains 384 MiB no
        # per-reader term carries, which is nearly three times the remainder
        # left over. Folded into that remainder it would read as a term nothing
        # accounts for, and as a breach on a 512 MiB-block file.
        jobs, worst, _, _ = self.SEED_CELLS[0]
        billed, pool, unnamed = memory_model.charge_model(
            self.SEED_UNIT, jobs, worst * config.MIB
        )
        self.assertGreater(pool, 2 * unnamed)
        self.assertEqual(pool, (memory_model.LIBRARY_POOL_DEPTH - 1) * self.SEED_UNIT)
        # And it never goes away: past the depth the pool clamps to, every
        # further reader brings a slot of it, which is what stops it being a
        # floor.
        self.assertEqual(
            memory_model.pool_bytes(self.SEED_UNIT, memory_model.LIBRARY_POOL_DEPTH),
            (memory_model.LIBRARY_POOL_DEPTH - 1) * self.SEED_UNIT,
        )
        self.assertEqual(memory_model.pool_bytes(self.SEED_UNIT, 99), 98 * self.SEED_UNIT)

    def test_an_over_bill_is_a_fault_and_says_which_way_it_went(self):
        # The half a grid search over reserve constants cannot report: a charge
        # that is too large shows up there as headroom.
        billed = memory_model.charge_bytes(self.SEED_UNIT, 4)
        fault = memory_model.charge_model_problem(self.SEED_UNIT, 4, billed - 32 * config.MIB)
        assert fault is not None
        self.assertEqual(fault.band, memory_model.BAND_OVER_BILL)
        self.assertIn("over-billed", fault.text)
        self.assertIn("fewer readers than the allocation affords", fault.text)
        # And it refutes: the `bound` band alone is released, an over-bill
        # carrying no remedy the sitting discharges.
        self.assertTrue(fault.refutes)

    def test_a_remainder_above_the_reserve_is_a_fault_and_names_the_constant(self):
        billed = memory_model.charge_bytes(self.SEED_UNIT, 4)
        held = billed + memory_model.LIBRARY_MEMORY_RESERVE + config.MIB
        fault = memory_model.charge_model_problem(self.SEED_UNIT, 4, held)
        assert fault is not None
        self.assertEqual(fault.band, memory_model.BAND_RULE)
        self.assertIn("unnamed", fault.text)
        self.assertIn("MEMORY_RESERVE", fault.text)
        self.assertIn("rule does not hold", fault.text)
        # And exactly at the reserve it is the *inner* fault only: the criterion
        # is what the constant promises, not a margin inside it, so the rule
        # still holds there and the bound is what is named.
        at_reserve = memory_model.charge_model_problem(
            self.SEED_UNIT, 4, billed + memory_model.LIBRARY_MEMORY_RESERVE
        )
        assert at_reserve is not None
        self.assertIn("MEMORY_UNPOOLED_BOUND", at_reserve.text)
        self.assertNotIn("rule does not hold", at_reserve.text)

    def test_the_bound_band_alone_is_released(self):
        # A fault carries its band so the verdict can read them by name. A bound
        # fault is released — the remainder is inside `MEMORY_RESERVE` and the
        # sitting re-derives the constant it overran — and every other band
        # refutes, the over-bill included: it cannot be apparatus scatter and
        # it leaves the sitting nothing to repair.
        billed = memory_model.charge_bytes(self.SEED_UNIT, 4)
        bars = {
            memory_model.BAND_OVER_BILL: billed - config.MIB,
            memory_model.BAND_BOUND: billed + memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND + config.MIB,
            memory_model.BAND_RULE: billed + memory_model.LIBRARY_MEMORY_RESERVE + config.MIB,
        }
        for band, held in bars.items():
            with self.subTest(band=band):
                fault = memory_model.charge_model_problem(self.SEED_UNIT, 4, held)
                assert fault is not None
                self.assertEqual(fault.band, band)
                self.assertEqual(fault.refutes, band != memory_model.BAND_BOUND)

    def test_a_band_with_no_stance_recorded_refutes_and_is_reported(self):
        # The rule is an enumeration, so a fault line added later does not
        # inherit the released half by being unmentioned. `refutes` defaults to
        # refuting, and `--check` names the band until somebody records a
        # stance for it.
        self.assertIsNotNone(memory_model.band_refutes("a-line-nobody-has-argued"))
        self.assertTrue(memory_model.ChargeFault("a-line-nobody-has-argued", "…").refutes)
        self.assertEqual(memory_model.charge_band_problems(), [])
        with unittest.mock.patch.object(memory_model, "BAND_INVENTED", "invented", create=True):
            reported = memory_model.charge_band_problems()
        self.assertEqual(len(reported), 1, reported)
        self.assertIn("BAND_INVENTED", reported[0])
        self.assertIn("BAND_STANCE", reported[0])

    def test_every_refuting_band_says_why_it_refutes(self):
        # The verdict prints the band's clause beside the cells, so a stance
        # cannot be recorded without the reason being written in the same place
        # — where one sentence over the whole refuting stanza would assert a
        # single band's reason over every cell in it.
        for band, why in memory_model.BAND_STANCE.items():
            with self.subTest(band=band):
                if why is None:
                    self.assertIsNone(memory_model.band_refutes(band))
                else:
                    self.assertTrue(why.strip())
                    self.assertEqual(memory_model.band_refutes(band), why)
        self.assertIsNone(memory_model.BAND_STANCE[memory_model.BAND_BOUND])

    def test_the_bound_is_re_derived_on_the_grid_it_was_read_off(self):
        # The shipped 256 MiB is the next 64 MiB step above the five-build
        # grid's 238.6 MiB worst remainder. The re-derivation is that arithmetic
        # over whatever sitting is in hand, which is what lets a cell above the
        # bound publish with its finding instead of owing a re-take.
        self.assertEqual(
            memory_model.rederived_unpooled_bound(238.6 * config.MIB),
            memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND,
        )
        # On a step exactly, the step itself covers it; above it, the next one.
        step = memory_model.UNPOOLED_BOUND_STEP
        self.assertEqual(memory_model.rederived_unpooled_bound(4 * step), 4 * step)
        self.assertEqual(memory_model.rederived_unpooled_bound(4 * step + 1), 5 * step)
        # And it is never zero: a sitting whose worst remainder is negative is
        # an over-bill throughout, not a bound of nothing.
        self.assertEqual(memory_model.rederived_unpooled_bound(-1.0), step)

    def test_the_two_ceilings_are_distinguishable_at_every_cell(self):
        # The reason for two lines rather than one: between them the
        # allocation holds and the number the count is predicted against is
        # wrong; above the outer one the allocation does not. A single threshold
        # cannot say which, whichever of the two it is set at.
        billed = memory_model.charge_bytes(self.SEED_UNIT, 4)
        inner = memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND
        outer = memory_model.LIBRARY_MEMORY_RESERVE
        self.assertLess(inner, outer)
        # Exactly at the bound: inside both, so no fault at all.
        self.assertIsNone(memory_model.charge_model_problem(self.SEED_UNIT, 4, billed + inner))
        between = memory_model.charge_model_problem(self.SEED_UNIT, 4, billed + inner + config.MIB)
        assert between is not None
        self.assertIn("finding about the bound, not the rule", between.text)
        above = memory_model.charge_model_problem(self.SEED_UNIT, 4, billed + outer + config.MIB)
        assert above is not None
        self.assertIn("rule does not hold", above.text)

    def test_the_mirrored_pool_depth_and_constants_are_the_librarys_own(self):
        # All four are hardcoded on `QUERY_SUBSTREAM_CAP`'s argument, so the
        # mirror is checked here rather than trusted. The two byte constants
        # especially: they are the model's two upper bounds, so one that moved
        # in the library and not here would check the rule against a promise it
        # no longer makes — and the margin is the line `reserve`'s gate
        # reports each leg against.
        src = test_support.read_path_source()
        self.assertIn(f"const POOL_DEPTH: usize = {memory_model.LIBRARY_POOL_DEPTH};", src)
        self.assertIn(
            f"pub const MEMORY_RESERVE: u64 = {memory_model.LIBRARY_MEMORY_RESERVE >> 20} << 20;",
            src,
        )
        self.assertIn(
            "pub const MEMORY_UNPOOLED_BOUND: u64 = "
            f"{memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND >> 20} << 20;",
            src,
        )
        self.assertIn(
            f"pub const MEMORY_MARGIN_PERCENT: u64 = {memory_model.LIBRARY_MEMORY_MARGIN_PERCENT};",
            src,
        )
        # And the margin predicts with the second rather than the first.
        # *Rejected:* one number for both — the criterion is then applied twice,
        # and the doubling is invisible in every reading the harness takes.
        self.assertIn("saturating_sub(MEMORY_UNPOOLED_BOUND)", src)

    def test_the_recommendation_and_the_affordability_charge_are_one_number(self):
        # What `reader_bytes`' docstring asserts, and what the model rests on:
        # the charge the rule solves an allowance against is the charge the gate
        # then compares a budget to. Split, they are two numbers 7 MiB apart
        # and the mirror is a model of neither. Both come off one composition
        # site, which is what is pinned here.
        src = test_support.read_path_source()
        self.assertIn(
            "fn default_worker_memory(&self) -> Option<WorkerMemory> {\n"
            "        self.budget.block_worker_memory()",
            src,
        )
        self.assertIn(
            "fn affordable(&self, chunk_bytes: u64, decode_bytes: u64, budget: u64) -> bool {\n"
            "        self.worker_memory(chunk_bytes, decode_bytes).at(1) <= budget",
            src,
        )

    def test_the_library_bills_the_pool_term_the_model_names(self):
        # `charge_bytes` is `WorkerMemory::at` mirrored, so the pool term's
        # shape has to be the library's: a pool that clamped somewhere else
        # would make every cell of the check arithmetic about a rule nothing
        # implements.
        src = test_support.read_path_source()
        self.assertIn(
            "(self.pool_depth.max(workers).saturating_sub(1) as u64)"
            ".saturating_mul(self.pool_unit)",
            src,
        )
        self.assertIn(".pooling(self.unit as u64, POOL_DEPTH)", src)

    # -- the resolution, read back off the run -----------------------------

    def test_the_resolved_arrangement_is_read_off_the_runs_own_log(self):
        # The count a flagless run resolves exists nowhere else: the harness
        # cannot compute it without reimplementing the rule under test.
        log = (
            "2026-09-11T03:33:02Z  INFO running inside a stated memory allocation "
            "limit_bytes=536870912 limit_read_from=/sys/fs/cgroup/memory.max "
            "jobs_flag=(not stated) memory_flag=(not stated)\n"
            "2026-09-11T03:33:02Z  INFO resolved the arrangement "
            "jobs=3 (recommended by the source; lowered from 24 by the allocation) "
            "memory_bytes=204576096 (discovered: /sys/fs/cgroup/memory.max states a "
            "limit of 536870912 byte(s))\n"
            "2026-09-11T03:33:02Z  INFO preamble scan started bytes=3221227790 "
            "chunk_size=1048576 jobs=3 memory_bytes=204576096\n"
            "2026-09-11T03:33:18Z  INFO scan started bytes=3221227790 resumed_from=425 "
            "chunk_size=1048576 jobs=3 memory_bytes=204576096\n"
            "maxrss_kib=485786\n"
        )
        self.assertEqual(
            parsing.parse_resolution(log),
            {"resolved_jobs": "3", "resolved_budget": "204576096"},
        )

    def test_the_line_it_reads_still_carries_that_pair_in_that_order(self):
        # The flagless axis rests on a log line, which is the one input here
        # that is not a constant: a field renamed or reordered in `forward.rs`
        # turns every reader count into a missing key, and the figure would fail
        # a sitting in rather than at its first second.
        src = (config.REPO / "crates/pgdt-mapping/src/forward.rs").read_text()
        event = src[: src.index('"scan started"')]
        jobs = event.rindex("jobs = ")
        budget = event.rindex("memory_bytes = ")
        self.assertLess(jobs, budget)
        self.assertNotIn("\n\n", event[jobs:])

    def test_a_run_that_reports_no_arrangement_reports_nothing(self):
        # Every shape that is not a scan, `dd` and the decode instrument among
        # them. `{}` rather than a guess: a renderer that wants the pair says so
        # by failing on its absence.
        self.assertEqual(parsing.parse_resolution("real 0m1.000s\n"), {})

    # -- the fit ------------------------------------------------------------

    def test_the_fit_recovers_a_line_exactly(self):
        fixed, per_reader = reserve._least_squares([(1, 110.0), (2, 120.0), (4, 140.0)])
        self.assertAlmostEqual(fixed, 100.0)
        self.assertAlmostEqual(per_reader, 10.0)

    def test_a_fit_through_one_reader_count_is_refused(self):
        # An intercept asserted as a measurement is the extrapolation the
        # evidence skill's second rule names, and the failure this figure
        # exists to stop repeating.
        with self.assertRaises(ValueError):
            reserve._least_squares([(3, 474.0), (3, 503.0)])

    # A block size of zero makes `pool_bytes` vanish, so the three guard tests
    # below are about the guard alone and not about `_depooled`'s subtraction; the
    # de-pooling has its own tests after them.
    NO_POOL = 0

    def test_the_publication_guard_sits_in_front_of_the_arithmetic(self):
        # Every fitted line in this harness crosses `_fit_or_secant`, so
        # a fourth call site cannot skip the rule by not remembering it.
        fixed, slope = reserve._fit_or_secant(
            [(1, 110.0), (2, 120.0), (4, 140.0)], self.NO_POOL
        )
        self.assertAlmostEqual(fixed, 100.0)
        self.assertAlmostEqual(slope, 10.0)

    def test_below_the_guard_the_slope_survives_and_the_intercept_does_not(self):
        # The slope is the same number either way — at two distinct abscissae
        # the least-squares slope *is* the secant between the group means — so
        # withholding the intercept withholds the model's claim and nothing the
        # axis measured.
        points = [(1, 110.0), (1, 114.0), (4, 200.0)]
        fixed, slope = reserve._fit_or_secant(points, self.NO_POOL)
        self.assertIsNone(fixed)
        self.assertAlmostEqual(slope, (200.0 - 112.0) / 3)
        self.assertAlmostEqual(slope, reserve._least_squares(points)[1])

    def test_a_secant_needs_two_points_as_much_as_a_fit_does(self):
        with self.assertRaises(ValueError):
            reserve._fit_or_secant([(3, 474.0), (3, 503.0)], self.NO_POOL)

    # -- the kink the fit must not cross (`_depooled`) ----------------------

    def test_the_term_taken_off_each_point_is_the_librarys_own(self):
        # Not a second spelling of the pool's arithmetic: `_depooled` subtracts
        # `pool_bytes`, which is `WorkerMemory::pool_bytes` mirrored, so the
        # thing removed from the ordinate is exactly the thing the charge bills.
        unit = shapes.RESERVE_MECHANISM_UNIT
        for jobs in (1, 2, 3, 4, 5, 11, 29):
            with self.subTest(jobs=jobs):
                (_, left), = reserve._depooled([(jobs, 1000.0)], unit)
                self.assertAlmostEqual(
                    1000.0 - left, memory_model.pool_bytes(unit, jobs) / config.MIB
                )

    def test_the_fit_recovers_the_two_unknown_terms_across_the_kink(self):
        # The synthetic mechanism, exactly: a fixed cost, a per-reader cost and
        # the pool's retention list, read at counts either side of `POOL_DEPTH`.
        # A line over the remainder recovers the two terms the sitting does not
        # know; a line over the resident set does not, and cannot, because the
        # quantity it is fitting has a bend in it.
        unit, fixed, per_reader = shapes.RESERVE_MECHANISM_UNIT, 120.0, 31.0
        counts = [1, 2, 11, 12, 20, 29]
        held = [
            (k, fixed + per_reader * k + memory_model.pool_bytes(unit, k) / config.MIB)
            for k in counts
        ]
        got_fixed, got_per_reader = reserve._fit_or_secant(held, unit)
        self.assertAlmostEqual(got_fixed, fixed)
        self.assertAlmostEqual(got_per_reader, per_reader)
        raw_fixed, raw_per_reader = reserve._least_squares(held)
        # 25 MiB of it at these counts, which is the bend measured from the true
        # fixed term; the published 49 MiB is the same bias measured from the
        # above-kink regime's own intercept of minus one unit.
        self.assertGreater(raw_fixed - fixed, 20.0)
        self.assertGreater(raw_per_reader, per_reader)

    def test_the_bias_a_straight_line_across_the_kink_carries(self):
        # The arithmetic behind the published bias, over the charge's held-unit
        # values alone, at
        # the reader counts the registered axis resolves. It is why the
        # subtraction exists, and it is checkable without a sitting.
        for unit, counts, bias, slope_units in (
            (24 << 20, [1, 2, 11, 12, 20, 29], 49.0, 1.90),
            (128 << 20, [1, 2, 4, 6], 421.0, 1.37),
        ):
            with self.subTest(unit=unit):
                held = [
                    (k, (k * unit + memory_model.pool_bytes(unit, k)) / config.MIB)
                    for k in counts
                ]
                intercept, slope = reserve._least_squares(held)
                # The regime the axis mostly sits in holds `2k - 1` units, so a
                # faithful line there has an intercept of minus one unit.
                self.assertAlmostEqual(
                    intercept + unit / config.MIB, bias, delta=1.0
                )
                self.assertAlmostEqual(slope / (unit / config.MIB), slope_units, places=2)
                # And the remainder is a line through the origin with a slope of
                # exactly one unit, at every count on both sides of the bend.
                self.assertEqual(
                    [round(y, 6) for _, y in reserve._depooled(held, unit)],
                    [round(k * unit / config.MIB, 6) for k in counts],
                )

    def test_no_line_is_fitted_without_crossing_the_de_pooling(self):
        # The same argument `RESERVE_FIT_MIN_COUNTS` makes about the guard: the
        # subtraction is a property of the mechanism, not of one renderer, so
        # `_least_squares` is reached through `_fit_or_secant` and nowhere else.
        source = test_support.harness_source()
        calls = [
            ln for ln in source.splitlines()
            if "_least_squares(" in ln and not ln.lstrip().startswith("def ")
        ]
        self.assertEqual(
            [ln.strip() for ln in calls],
            ["fixed, slope = _least_squares(remainder)"],
            "a fitted line that does not cross `_depooled` publishes the charge's own bend "
            "as an intercept (`_depooled`)",
        )

    # -- the edges it owes --------------------------------------------------

    def test_the_only_reading_it_shares_is_peak_rsss_control_row(self):
        """What it owes `peak-rss` when it is published, and what it does not owe
        `rss-attribution`.

        The edge is mechanical rather than asserted: a shared reading is two
        figures keying the same run, so the overlap is computable. `peak-rss`
        carries exactly one of these runs — the serial-default baseline.
        `rss-attribution` carries none, and that is a reading of the leg set
        rather than an omission: its every leg runs over the two block-count
        shapes whose axis it is, where nothing here does. What the three share
        is a *sitting*, which one sweep takes."""
        mine = {
            spec.key("")
            for spec in (
                *reserve._reserve_specs(),
                *reserve._reserve_flagless_specs(),
                *(s for _, s in reserve._reserve_mechanism_specs()),
                *reserve._reserve_step_specs(),
                register._RESERVE_BASELINE,
            )
        }
        peak = {spec.key("") for spec in test_support._peak_rss_specs()}
        attribution = {
            spec.key("")
            for _, small, big in register._attribution_specs()
            for spec in (small, big)
        }
        self.assertEqual(mine & peak, {register._RESERVE_BASELINE.key("")})
        self.assertEqual(mine & attribution, set())


if __name__ == "__main__":
    unittest.main()
