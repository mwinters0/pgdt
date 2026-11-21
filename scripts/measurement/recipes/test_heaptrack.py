"""Tests of `measurement.recipes.heaptrack`."""

from __future__ import annotations

import contained
import re
import shlex
import unittest
import unittest.mock

from measurement import config
from measurement import host
from measurement import inputs
from measurement import shapes
from measurement import test_support
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import gzip_resident
from measurement.figures import reserve
from measurement.recipes import heaptrack
from measurement.recipes import profile


class HeaptrackRecipe(unittest.TestCase):
    """The libc-level instrument, printed here for the same reason the sampling
    profile is. Its failure mode is the profile's, not koji's: every mistake
    below returns a report that looks fine and describes something else — and
    one of them already has, an unexplained 67.11 MB that was a merged frame's
    summed peak read as one allocation."""

    def _recipe(self) -> str:
        return heaptrack.heaptrack_recipe(config.Config())

    def _records(self) -> list[str]:
        return [
            ln for ln in self._recipe().splitlines()
            if f"{heaptrack.HEAPTRACK} --record-only" in ln and not ln.startswith("#")
        ]

    def _reports(self) -> list[str]:
        return [
            ln for ln in self._recipe().splitlines()
            if f"{heaptrack.HEAPTRACK}_print " in ln and not ln.startswith("#")
        ]

    def test_the_profiling_binary_is_the_one_recorded(self):
        # heaptrack resolves symbols off either build, so this one fails
        # quietly: `release` yields a report with no `.rs:` reference anywhere
        # in it — 3,627 against 0 on the same recording — and every Rust frame
        # is a bare name with no file behind it.
        recipe = self._recipe()
        self.assertIn("/profiling/pgdt ", recipe)
        self.assertNotIn("/release/pgdt", recipe)
        self.assertIn("--profile=profiling", recipe)

    def test_the_recorded_build_links_the_system_allocator(self):
        # heaptrack counts requests at `malloc`, and the shipped build's
        # mimalloc is linked without `override`: a recording of the default
        # build names C's allocations and reads as though Rust made none.
        cfg = config.Config()
        recipe = heaptrack.heaptrack_recipe(cfg)
        self.assertEqual(heaptrack.HEAPTRACK_LEG, "system")
        self.assertIn(heaptrack.HEAPTRACK_LEG, leg_binaries.ALLOCATOR_LEGS)
        self.assertIn(
            f"--no-default-features --features={heaptrack.HEAPTRACK_LEG}", recipe
        )
        self.assertIn(f"(allocator: {heaptrack.HEAPTRACK_LEG})", recipe)
        target = image.image_target_dir(cfg, f"heaptrack-{heaptrack.HEAPTRACK_LEG}")
        binary = heaptrack.HEAPTRACK_BUILD.binary(target)
        c = profile.contained_prefix(cfg, contained.DEFAULT_MEMORY)
        self.assertIn(f"{c} {binary} --version", recipe.splitlines())
        recorded = [ln for ln in recipe.splitlines() if f" {binary} parse " in ln]
        self.assertEqual(len(recorded), len(heaptrack.HEAPTRACK_SHAPES))

    def test_the_recorded_build_has_a_target_dir_of_its_own(self):
        # A `--features` build under `--profile profiling` in the default
        # target dir overwrites `target/profiling/pgdt`, the binary the profile
        # recipe records; and none of the allocator legs' or instruments' dirs
        # may be shared either, a sweep building one while this one builds.
        cfg = config.Config()
        recipe = heaptrack.heaptrack_recipe(cfg)
        target = image.image_target_dir(cfg, f"heaptrack-{heaptrack.HEAPTRACK_LEG}")
        self.assertIn(f"--target-dir {target})", recipe)
        self.assertNotIn(str(config.REPO / "target"), recipe)
        others = {*leg_binaries.ALLOCATOR_LEGS, "introspect", "dfcli-introspect"}
        self.assertNotIn(target.name, others)

    def test_frame_pointers_are_not_asked_for(self):
        # A stated non-requirement, not an omission: heaptrack unwinds
        # `.eh_frame` where `perf` needs frame pointers, so the flag buys
        # nothing and would fingerprint a second build of the same source.
        commands = [
            ln for ln in self._recipe().splitlines() if ln and not ln.startswith("#")
        ]
        for line in commands:
            with self.subTest(line=line):
                self.assertNotIn("force-frame-pointers", line)
                self.assertNotIn("--rustflags", line)
        self.assertTrue(any("--profile=profiling" in ln for ln in commands))

    def test_the_gui_is_never_launched(self):
        # Without `--record-only` heaptrack hands the finished file to
        # `heaptrack_gui`, which need not be installed and, where it is, may
        # not start. The recording is written either way, so the failure is
        # cosmetic — and a recipe ending in an error message is one a session
        # stops trusting.
        self.assertTrue(self._records())
        for line in self._records():
            with self.subTest(line=line):
                self.assertIn("--record-only", line)

    def test_the_report_does_not_merge_backtraces(self):
        # `heaptrack_print` merges by default and its own --help says the
        # merged peak consumption is not correct: a merged frame prints the
        # *summed* peak of every backtrace under it beside a call count
        # belonging to the merge, so one decoder's 8.39 MB can read as
        # "8.39M over 2 calls" and eight of them as 67.11 MB.
        self.assertTrue(self._reports())
        for line in self._reports():
            with self.subTest(line=line):
                self.assertIn("--merge-backtraces=0", line)

    def test_the_rust_frames_are_demangled(self):
        # This build's symbols are v0 and this heaptrack cannot demangle them:
        # its Rust support is post-1.5.0 and no binary of it references
        # `rustc_demangle`. Without the pipe the C frames read fine and the
        # Rust frames above them are noise.
        recipe = self._recipe()
        piped = [ln for ln in recipe.splitlines() if ".txt" in ln]
        self.assertEqual(
            len(piped), len(heaptrack.HEAPTRACK_SHAPES) + len(heaptrack.HEAPTRACK_PAIRS)
        )
        for line in piped:
            with self.subTest(line=line):
                self.assertIn(f"| {heaptrack.HEAPTRACK_DEMANGLE} >", line)

    def test_each_report_reads_the_file_the_recorder_writes(self):
        # The image's heaptrack is built without zstd and writes `<stem>.gz`;
        # a printer handed `<stem>.zst` reads nothing and the report is empty.
        self.assertEqual(heaptrack.HEAPTRACK_SUFFIX, ".gz")
        for line in self._reports():
            with self.subTest(line=line):
                inner = shlex.split(line.partition(" sh -c ")[2])[0]
                read = [w for w in inner.split() if w.startswith("/") and "heaptrack-" in w]
                self.assertTrue(read)
                self.assertTrue(all(w.endswith(heaptrack.HEAPTRACK_SUFFIX) for w in read[:-1]), read)

    def test_each_report_pipes_inside_one_container(self):
        # A container's output reaches the host through the runtime's logger,
        # which drops what is unwritten when the container exits: a printer
        # piped to a second container's demangler lost the tail of every
        # report (`runs/p15.18/heaptrack/`). So the pipe and the file it
        # writes are both inside one `sh -c`, nothing crossing a container.
        cfg = config.Config()
        for line in self._reports():
            with self.subTest(line=line):
                head, sep, inner = line.partition(" sh -c ")
                self.assertTrue(sep, line)
                self.assertIn(head, {profile.contained_prefix(cfg, m) for _, m, _ in heaptrack.HEAPTRACK_PAIRS})
                words = shlex.split(inner)
                self.assertEqual(len(words), 1, line)
                self.assertIn(f"| {heaptrack.HEAPTRACK_DEMANGLE} > ", words[0])

    def test_each_recording_starts_from_no_cache(self):
        # `parse` resumes from a cache, so a pair sharing one would record a
        # full scan and then a no-op — two recordings whose difference is the
        # whole workload rather than the one byte of budget between them.
        lines = self._recipe().splitlines()
        starts = [i for i, ln in enumerate(lines) if f"{heaptrack.HEAPTRACK} --record-only" in ln]
        self.assertEqual(len(starts), len(heaptrack.HEAPTRACK_SHAPES))
        for i in starts:
            with self.subTest(line=lines[i]):
                self.assertTrue(lines[i - 1].startswith("rm -f "), lines[i - 1])
                self.assertIn("heaptrack.pgdt", lines[i - 1])

    def test_each_pair_is_read_as_a_difference(self):
        # The whole reason there are two recordings a pair: a single one names
        # what a run holds, and only the difference names what the admitted
        # reader, or the added workers, asked for. Both recordings must appear
        # in that one invocation, the first less the second.
        recipe = self._recipe()
        diff = [ln for ln in recipe.splitlines() if "--diff" in ln]
        self.assertEqual(len(diff), len(heaptrack.HEAPTRACK_PAIRS))
        for line, (axis, _, name) in zip(diff, heaptrack.HEAPTRACK_PAIRS):
            with self.subTest(diff=name):
                (first, first_input), (second, second_input) = axis
                suffix = heaptrack.HEAPTRACK_SUFFIX
                first_at = line.index(f"heaptrack-{first}-{first_input}{suffix}")
                second_at = line.index(f"--diff {config.Config().out_dir}/heaptrack-{second}-{second_input}{suffix}")
                self.assertLess(first_at, second_at)
                self.assertIn(f"> {config.Config().out_dir / name}.txt", line)

    def test_the_first_pass_pair_differs_only_in_its_worker_count(self):
        """The first-pass pair's difference is twenty added workers only if
        the two argvs are otherwise identical, the allowance included: an
        allowance moving with the count would put a second variable in it. The
        four-job shape is the second, so the diff is the larger count less
        the smaller, and both read the limit leg."""
        axis = heaptrack.HEAPTRACK_FIRST_PASS_AXIS
        self.assertEqual({name for _, name in axis}, {gzip_resident.GZIP_LIMIT_LEG})
        first, second = (
            heaptrack.heaptrack_argv(shape, "/dump.gz", "/tmp/x.pgdt") for shape, _ in axis
        )
        differing = [i for i, (a, b) in enumerate(zip(first, second)) if a != b]
        self.assertEqual(len(first), len(second))
        self.assertEqual(len(differing), 1, f"{first} vs {second}")
        self.assertEqual(first[differing[0] - 1], "--jobs")
        self.assertGreater(int(first[differing[0]]), int(second[differing[0]]))

    def test_the_pair_differs_only_in_its_budget(self):
        """A difference read frame by frame is only a difference if the two
        argvs are otherwise identical — a worker count or a cache path that
        moved with the budget would put a second variable in the one reading
        this pair exists to isolate."""
        argvs = [
            heaptrack.heaptrack_argv(shape, "/dump.sql", "/tmp/x.pgdt")
            for shape, _ in heaptrack.HEAPTRACK_AXIS
        ]
        self.assertEqual(len(argvs), 2)
        first, second = argvs
        self.assertEqual(len(first), len(second))
        differing = [i for i, (a, b) in enumerate(zip(first, second)) if a != b]
        self.assertEqual(len(differing), 1, f"{first} vs {second}")
        self.assertEqual(first[differing[0] - 1], "--memory")
        at = differing[0]
        self.assertEqual(abs(int(first[at]) - int(second[at])), 1)

    def test_a_recorded_shape_is_the_shape_the_sweep_times(self):
        """The reconciliation that keeps an attribution readable against the
        figure it explains — `profile_argv`'s, over the wrapped shapes.

        `_script` builds a container command line with a timer and the RSS
        wrapper in front of it, so the two cannot be one function; a flag that
        moves in one and not the other gives a recording of something no figure
        measures, and nothing else would notice."""
        for shape, _ in heaptrack.HEAPTRACK_SHAPES:
            with self.subTest(shape=shape):
                script = shapes._script(shape)
                head, sep, rest = script.partition("/pgdt ")
                self.assertTrue(sep, script)
                timed = [w for w in rest.split() if w != ">/dev/null"]
                recorded = heaptrack.heaptrack_argv(shape, "/dump.sql", "/tmp/x.pgdt")
                self.assertEqual(recorded, timed)

    def test_every_recorded_invocation_states_its_worker_count(self):
        # The apparatus rule, and here it is also what makes the pair a pair:
        # the count a source recommends moves with the budget, so a recording
        # that inherited one would differ from its partner in two things.
        argv_lines = [
            ln for ln in self._recipe().splitlines() if "/profiling/pgdt parse" in ln
        ]
        self.assertEqual(len(argv_lines), len(heaptrack.HEAPTRACK_SHAPES))
        for line, (shape, _) in zip(argv_lines, heaptrack.HEAPTRACK_SHAPES):
            with self.subTest(line=line):
                jobs = (
                    shapes.RESERVE_JOBS
                    if shape.startswith(shapes.RESERVE_STEP_FAMILY)
                    else int(shape.removeprefix(shapes.GZRSS_FAMILY))
                )
                self.assertIn(f"--jobs {jobs} ", line)

    def test_the_input_is_staged_and_torn_down(self):
        # tmpfs is 16 G and this input does not fit beside a sweep's.
        cfg = config.Config()
        recipe = heaptrack.heaptrack_recipe(cfg)
        for _, name in heaptrack.HEAPTRACK_SHAPES:
            staged = cfg.warm_dir / inputs.input_file(name)
            with self.subTest(input=name):
                self.assertIn(f"cp -n {cfg.cache_dir / inputs.input_file(name)}", recipe)
                self.assertIn(f"--source {staged}", recipe)
                self.assertIn(str(staged), recipe.splitlines()[-1])

    def test_every_binary_runs_in_a_container(self):
        # Every run of a built binary is a reading in a memory-limited
        # container (`contained.py`), the recordings' included.
        self.assertEqual(test_support.recipe_uncontained(self._recipe()), [])

    def test_a_recording_is_held_to_its_figures_container_plus_one_term(self):
        # A recorded shape is a timed one, so it is recorded in the container
        # its figure times it under, plus heaptrack's term: one stated GiB
        # whatever the shape, its bookkeeping growing with allocations and
        # backtraces rather than with what the run holds. The step pair's
        # container is `reserve`'s per-spec limit, the first-pass pair's
        # `gzip-resident`'s figure-wide one.
        self.assertEqual(heaptrack.HEAPTRACK_TERM, 1 << 30)
        timed = {
            "heaptrack-step-diff": reserve.RESERVE_MECHANISM_LIMIT,
            "heaptrack-first-pass-diff": shapes.GZPASS_MEMORY,
        }
        self.assertEqual({name for _, _, name in heaptrack.HEAPTRACK_PAIRS}, set(timed))
        cfg = config.Config()
        records = self._records()
        for axis, memory, name in heaptrack.HEAPTRACK_PAIRS:
            with self.subTest(pair=name):
                self.assertEqual(
                    contained.size_bytes(memory),
                    contained.size_bytes(timed[name]) + heaptrack.HEAPTRACK_TERM,
                )
                prefix = profile.contained_prefix(cfg, memory)
                for shape, input_name in axis:
                    stem = f"heaptrack-{shape}-{input_name}"
                    line = next(ln for ln in records if f" -o {cfg.out_dir / stem} " in ln)
                    self.assertTrue(line.startswith(f"{prefix} sh -c "), line)

    def test_both_pairs_fit_half_a_sixteen_gib_machine(self):
        # The rule's portability: a figure's container works on any machine
        # the harness runs on, and so does it plus one term.
        for _, memory, name in heaptrack.HEAPTRACK_PAIRS:
            with self.subTest(pair=name):
                contained.check_memory(memory, 16 << 30)

    def test_the_container_is_the_one_its_figure_states(self):
        # Read off the sweep's own specs, so a figure moving its container
        # moves the recording's with it, and a shape its figure does not time
        # has no container to be recorded in.
        specs = reserve._reserve_step_specs()
        self.assertEqual(
            heaptrack.heaptrack_memory("reserve", specs, heaptrack.HEAPTRACK_AXIS),
            f"{(contained.size_bytes(specs[0].memory) + heaptrack.HEAPTRACK_TERM) >> 20}m",
        )
        with self.assertRaises(ValueError):
            heaptrack.heaptrack_memory("reserve", specs, heaptrack.HEAPTRACK_FIRST_PASS_AXIS)

    def test_each_recording_writes_its_containers_peak_beside_it(self):
        # The term is stated, not read: no reading of it exists, the timed
        # cell is another build, and `heaptrack_interpret` runs in the same
        # cgroup. So each recording's own container writes its `memory.peak`
        # from inside its `sh -c`, beside the `.gz`, keeping the recorder's
        # exit status; the line before removes a stale one.
        cfg = config.Config()
        lines = self._recipe().splitlines()
        records = [i for i, ln in enumerate(lines) if f"{heaptrack.HEAPTRACK} --record-only" in ln]
        self.assertEqual(len(records), len(heaptrack.HEAPTRACK_SHAPES))
        for i, (shape, name) in zip(records, heaptrack.HEAPTRACK_SHAPES):
            with self.subTest(shape=shape):
                peak = cfg.out_dir / f"heaptrack-{shape}-{name}{heaptrack.HEAPTRACK_PEAK_SUFFIX}"
                _, sep, inner = lines[i].partition(" sh -c ")
                self.assertTrue(sep, lines[i])
                words = shlex.split(inner)
                self.assertEqual(len(words), 1, lines[i])
                self.assertIn(f">/dev/null; s=$?; cat {heaptrack.CGROUP_MEMORY_PEAK} > {peak};", words[0])
                self.assertTrue(words[0].endswith("; exit $s"), words[0])
                self.assertIn(str(peak), lines[i - 1].split())

    def test_the_step_numbering_has_no_hole(self):
        numbered = [
            int(ln.split(".")[0][2:]) for ln in self._recipe().splitlines()
            if re.match(r"^# \d+\. ", ln)
        ]
        self.assertEqual(numbered, list(range(len(numbered))))

    def test_the_recipe_never_runs_anything(self):
        # The rule koji's and the profile's recipes obey: this prints, and a
        # session runs it by hand.
        with unittest.mock.patch.object(host, "run") as ran:
            with unittest.mock.patch("sys.stdout"):
                heaptrack.cmd_heaptrack()
        ran.assert_not_called()


if __name__ == "__main__":
    unittest.main()
