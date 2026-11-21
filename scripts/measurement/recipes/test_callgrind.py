"""Tests of `measurement.recipes.callgrind`."""

from __future__ import annotations

import contained
import inspect
import re
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import count_pair
from measurement import host
from measurement import inputs
from measurement import shapes
from measurement.recipes import callgrind


class CallgrindRecipe(unittest.TestCase):
    """What places a rise `--count-pair` resolved (`decisions.md`, "D108").
    Every mistake below still returns a placement, of some other workload than
    the one the pair counted, so the gate would be read against the wrong
    number."""

    def _recipe(self) -> str:
        return callgrind.callgrind_recipe(config.Config())

    @staticmethod
    def _counted(lines: list[str]) -> list[int]:
        """The index of each callgrind run's first line."""
        return [i for i, ln in enumerate(lines) if f" {callgrind.CALLGRIND} --tool=callgrind " in ln]

    def test_the_legs_are_the_binaries_the_pair_counted(self):
        # A rebuild is another link: the rise placed is the one counted in
        # those two files, which the pair leaves where this reads them.
        cfg = config.Config()
        lines = self._recipe().splitlines()
        self.assertIn("count_pair_leg_binary(", inspect.getsource(count_pair.count_pair_binary))
        first = self._counted(lines)[0]
        for leg in count_pair.COUNT_PAIR_LEGS:
            binary = count_pair.count_pair_leg_binary(cfg, leg)
            with self.subTest(leg=leg):
                self.assertEqual(binary, cfg.out_dir / f"pgdt-count-pair-{leg}")
                asked = next(i for i, ln in enumerate(lines) if f"-- {binary} --version" in ln)
                self.assertIn("'(profile: release)'", lines[asked])
                self.assertLess(asked, first)
        recipe = "\n".join(lines)
        self.assertNotIn("release.py build", recipe)
        self.assertNotIn("cargo build", recipe)

    def test_each_row_runs_as_the_pair_counts_it(self):
        """`_script`'s timed line and builder, flag for flag, less the stated
        allowance: a moved flag would place a rise in a workload never
        counted."""
        for command, _, _, _ in count_pair.COUNT_PAIR_SHAPES:
            with self.subTest(shape=command):
                env, argv, builder = callgrind.callgrind_invocation(
                    command, "/dump.sql", "/tmp/x.pgdt", 1 << 29
                )
                stated = ["--memory", str(1 << 29)]
                if argv[0] == "sql":
                    # Ahead of the query, so the scan it plans draws against it.
                    at = argv.index(f"SET pgdump.memory = {1 << 29}")
                    self.assertEqual(argv[at - 1 : at + 2 : 2], ["-c", "-c"])
                    argv = [*argv[: at - 1], *argv[at + 1 :]]
                else:
                    self.assertEqual(argv[-2:], stated)
                    argv = argv[:-2]
                script = shapes._script(command)
                before, _, timed = script.partition("time ")
                timed = timed.split(" >", 1)[0]
                self.assertEqual(
                    f"{' '.join([*env, '/pgdt'])} {' '.join(argv)}".lstrip(),
                    timed.replace("'", "").lstrip(),
                )
                if builder is None:
                    self.assertEqual(before, "")
                    continue
                self.assertEqual(builder[-2:], stated)
                built = before.removesuffix(" && ").split()
                self.assertEqual(built[0], "/pgdt")
                self.assertEqual(builder[:-2], [w for w in built[1:] if w != ">/dev/null"])
        with self.assertRaises(ValueError):
            callgrind.callgrind_invocation("parse-rss", "/d", "/c", 1)

    def test_each_run_states_the_allowance_its_pair_container_gave(self):
        # The container here is larger, to hold valgrind; a discovered
        # allowance would move the readers and what statistics may hold (D85).
        cfg = config.Config()
        lines = re.sub(r"\\\n", " ", self._recipe()).splitlines()
        prefix = callgrind.callgrind_prefix(cfg)
        for command, label, memory, _ in count_pair.COUNT_PAIR_SHAPES:
            allowance = contained.size_bytes(memory or cfg.memory)
            self.assertGreater(contained.size_bytes(callgrind.CALLGRIND_MEMORY), allowance)
            row = lines[lines.index(f"# {label}") + 1:]
            row = row[: next((i for i, ln in enumerate(row) if ln.startswith("# ") or not ln), len(row))]
            pgdt_runs = [
                ln for ln in row
                if ln.startswith(prefix) and "callgrind_annotate" not in ln
            ]
            with self.subTest(shape=command):
                self.assertGreater(len(pgdt_runs), 0)
                self.assertEqual(len(pgdt_runs) % (2 * callgrind.CALLGRIND_REPS), 0)
                for line in pgdt_runs:
                    self.assertTrue(
                        f" --memory {allowance} " in line or f"'SET pgdump.memory = {allowance}'" in line,
                        line,
                    )

    def test_each_row_reads_the_pairs_input_unstaged(self):
        cfg = config.Config()
        recipe = self._recipe()
        lines = recipe.splitlines()
        self.assertNotIn(str(cfg.warm_dir), recipe)
        links = {}
        for name in dict.fromkeys(name for *_, name in count_pair.COUNT_PAIR_SHAPES):
            file = inputs.input_file(name)
            links[name] = cfg.out_dir / "callgrind-count-pair" / file
            self.assertIn(f"ln -sf {cfg.cache_dir / file} {links[name]}", recipe)
        for _, label, _, name in count_pair.COUNT_PAIR_SHAPES:
            row = lines[lines.index(f"# {label}") + 1 :]
            row = row[: next((i for i, ln in enumerate(row) if ln.startswith("# ") or not ln), len(row))]
            sources = [ln for ln in row if "--source " in ln]
            with self.subTest(row=label):
                self.assertGreater(len(sources), 0)
                for line in sources:
                    link = links[name]
                    self.assertTrue(
                        f"--source {link} " in line or f"--source {shapes.DFCLI_CATALOG}={link} " in line,
                        line,
                    )
        self.assertEqual(lines[-len(links):], [f"rm -f {link}" for link in links.values()])

    def test_the_linked_input_is_mounted(self):
        # `contained.py` mounts the directory of each path a line names, so
        # the link's target, which no line names, is mounted by the prefix.
        cfg = config.Config()
        prefix = callgrind.callgrind_prefix(cfg)
        self.assertIn(f" --mount {cfg.cache_dir} ", prefix)
        for line in self._recipe().splitlines():
            if line.startswith("python3 "):
                with self.subTest(line=line):
                    self.assertTrue(line.startswith(prefix), line)

    def test_no_counted_pgdt_is_its_namespaces_init(self):
        """valgrind's guest is its own process, so a `pgdt` valgrind is
        started as init installs a handler on every ending signal, and
        valgrind refuses the real-time one it reserves: the wrapper stays
        init, never `exec`'d, as the pair's `count-instructions` does."""
        lines = self._recipe().splitlines()
        wrapped = f" {callgrind.CALLGRIND_NOT_INIT} "
        for i in self._counted(lines):
            with self.subTest(line=lines[i]):
                head = lines[i].split(f" {callgrind.CALLGRIND} --tool=callgrind ", 1)[0]
                self.assertIn(wrapped, f"{head} ")
        self.assertNotIn("exec", callgrind.CALLGRIND_NOT_INIT)
        # Its child's parent is the wrapper, not the shell that started it:
        # an `exec`'d wrapper would hand its pid to the child.
        run = subprocess.run(
            ["sh", "-c", f"echo $$; {callgrind.CALLGRIND_NOT_INIT} sh -c 'echo $PPID'; true"],
            capture_output=True, text=True, check=True,
        )
        shell, parent = run.stdout.split()
        self.assertNotEqual(shell, parent)

    def test_every_run_starts_from_no_cache(self):
        # The pair's container starts from none, so a run over a cache the
        # last one left would count a read where the pair counted a map.
        lines = self._recipe().splitlines()
        for i in self._counted(lines):
            with self.subTest(line=lines[i]):
                before = lines[i - 1]
                if " parse --source " in before and "--tool" not in before:
                    before = lines[i - 2]
                self.assertTrue(before.startswith("rm -f "), before)

    def test_both_legs_alternate_as_the_pair_orders_its_reps(self):
        self.assertGreaterEqual(callgrind.CALLGRIND_REPS, 2)
        lines = self._recipe().splitlines()
        reps = callgrind.CALLGRIND_REPS
        expected = [
            (leg, rep)
            for rep in range(1, reps + 1)
            for leg in (
                count_pair.COUNT_PAIR_LEGS if rep - 1 < (reps + 1) // 2 else count_pair.COUNT_PAIR_LEGS[::-1]
            )
        ]
        for command, _, _, name in count_pair.COUNT_PAIR_SHAPES:
            row = count_pair.count_pair_row(command, name)
            runs = [
                re.search(rf"/{re.escape(row)}-(\w+)-(\d+)\.out ", lines[i + 1])
                for i in self._counted(lines)
            ]
            got = [(m.group(1), int(m.group(2))) for m in runs if m]
            with self.subTest(shape=row):
                self.assertEqual(got, expected)

    def test_each_count_is_annotated_and_its_rows_totals_listed(self):
        cfg = config.Config()
        recipe = self._recipe()
        work = cfg.out_dir / "callgrind-count-pair"
        for command, _, _, name in count_pair.COUNT_PAIR_SHAPES:
            row = count_pair.count_pair_row(command, name)
            runs = [
                work / f"{row}-{leg}-{rep}"
                for rep in range(1, callgrind.CALLGRIND_REPS + 1)
                for leg in count_pair.COUNT_PAIR_LEGS
            ]
            with self.subTest(shape=row):
                for run in runs:
                    self.assertIn(f"--callgrind-out-file={run}.out ", recipe)
                    self.assertIn(
                        f"callgrind_annotate --inclusive=no --threshold=100 --show-percs=no "
                        f"--auto=no {run}.out > {run}.txt",
                        recipe,
                    )
                # Named, not globbed: `parse-*` would take the data level's.
                self.assertIn(
                    f"grep -H 'PROGRAM TOTALS' {' '.join(f'{r}.txt' for r in runs)}\n", recipe
                )
                for rep in range(1, callgrind.CALLGRIND_REPS + 1):
                    parent, tree = (work / f"{row}-{leg}-{rep}.txt" for leg in count_pair.COUNT_PAIR_LEGS)
                    self.assertIn(f"  {parent} {tree} | sort -rn | cut -f2- ", recipe)

    def test_the_place_is_each_functions_self_ir_change(self):
        """The awk the recipe prints, run on two listings `callgrind_annotate`
        writes: each count right-aligned to the widest, the object path each
        leg's binary has dropped, a name met twice summed, the totals left out,
        an unchanged function absent."""
        listing = (
            "--------------------------------------------------------------------------------\n"
            "Ir             \n"
            "{total}  PROGRAM TOTALS\n\n"
            "Ir             file:function\n"
            "{a:>13}  ???:pgdt_core::copy::unescape_field [{obj}]\n"
            "        1,000  ???:<a::B<C, D> as e::F>::g [{obj}]\n"
            "{c:>13}  ???:pgdt_codec::key::compare_keys [{obj}]\n"
            "{c:>13}  ???:pgdt_codec::key::compare_keys [{obj}]\n"
            "            7  ./string/memcmp.S:__memcmp_avx2_movbe [/usr/lib/x86_64-linux-gnu/libc.so.6]\n"
        )
        with tempfile.TemporaryDirectory() as tmp:
            parent, tree = Path(tmp) / "p.txt", Path(tmp) / "t.txt"
            for path, total, a, c, leg in (
                (parent, "9,000,000,000", "5,007,488,350", "2", "parent"),
                (tree, "9,500,000,000", "4,007,488,350", "5", "tree"),
            ):
                path.write_text(listing.format(total=total, a=a, c=c, obj=f"/r/pgdt-count-pair-{leg}"))
            out = subprocess.run(
                ["awk", callgrind.CALLGRIND_DIFF_AWK, parent, tree],
                capture_output=True, text=True, check=True,
            ).stdout
        got = sorted(out.splitlines(), key=lambda ln: -int(ln.split("\t")[0]))
        self.assertEqual(
            [ln.split("\t", 1)[1] for ln in got],
            ["-1000000000\t???:pgdt_core::copy::unescape_field", "+6\t???:pgdt_codec::key::compare_keys"],
        )

    def test_the_step_numbering_has_no_hole(self):
        numbered = [
            int(ln.split(".")[0][2:]) for ln in self._recipe().splitlines()
            if re.match(r"^# \d+\. ", ln)
        ]
        self.assertEqual(numbered, list(range(len(numbered))))

    def test_the_recipe_never_runs_anything(self):
        with unittest.mock.patch.object(host, "run") as ran:
            with unittest.mock.patch("sys.stdout"):
                callgrind.cmd_callgrind()
        ran.assert_not_called()


if __name__ == "__main__":
    unittest.main()
