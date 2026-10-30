#!/usr/bin/env python3
"""Unit tests for contained.py, run with `uv run python -m unittest test_contained`.

The classifier the hook refuses by is held here to the commands a session
actually writes, both ways: each reading outside a container refused, and the
look-alikes — a binary's name in a commit message, a heredoc, `ls` of a build —
let through. The container line is built against a fake image, nothing run.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import contained

REFUSED = [
    "target/release/pgdt parse --source /dev/shm/pgdt/control.sql",
    "./target/profiling/pgdt info --dtcache x",
    "FOO=1 time /mnt/x/pgdt parse --source a",
    "env A=1 B=2 runs/pgdt-alloc-system parse --source a",
    "cd .. && timeout 60 ./target/debug/deps/pgdt_gzip_seek-0123abcd",
    "pgdt parse --source dump.sql",
    "cargo run -p pgdt -- parse --source x",
    "cargo +nightly bench -p pgdt-codec",
    "perf record -F 4999 -- target/profiling/pgdt parse",
    "perf stat -e instructions:u -- ls",
    "heaptrack --record-only -o x pgdt",
    "strace -f -e trace=openat target/release/pgdt parse",
    "valgrind ls",
    "echo $(heaptrack ls)",
    "bash -c 'target/release/pgdt parse --source x'",
    "sudo sh -c 'perf record -- ls'",
    "ls; target/release/pgdt parse",
    "ls\ntarget/release/pgdt parse",
    "setsid -f nohup target/release/pgdt parse > runs/x.log 2>&1 &",
]

ALLOWED = [
    "sudo nerdctl run --rm -m 512m img /pgdt parse --source /dump.sql",
    "docker run --rm img perf record -- /pgdt parse",
    "python3 scripts/contained.py --memory 4g -- perf record -- target/profiling/pgdt parse",
    "cd scripts && uv run measure.py --figure gzip-resident",
    "cd scripts && uv run release.py build --profile profiling",
    'git commit -m "a; target/release/pgdt parse x"',
    "rg 'perf record' scripts/",
    "ls -la target/release/pgdt",
    "objdump -T target/profiling/pgdt | head",
    "target/release/pgdt --version",
    "runs/pgdt-release-thin --help",
    "perf report -i runs/profile.data --stdio",
    "perf diff a.data b.data",
    "heaptrack_print --merge-backtraces=0 x.zst | c++filt",
    "cargo build --release -p pgdt",
    "cargo nextest run -p pgdt-gzip-seek",
    "cat <<'EOF' > runs/x.sh\ntarget/release/pgdt parse\nperf record -- ls\nEOF\nchmod +x runs/x.sh",
    "echo 'unbalanced",
]


class Uncontained(unittest.TestCase):
    def test_each_reading_outside_a_container_is_refused(self):
        for command in REFUSED:
            with self.subTest(command=command):
                self.assertIsNotNone(contained.uncontained(command))

    def test_each_look_alike_is_let_through(self):
        for command in ALLOWED:
            with self.subTest(command=command):
                self.assertIsNone(contained.uncontained(command))

    def test_the_refusal_names_the_simple_command_alone(self):
        self.assertEqual(
            contained.uncontained("cd /x && FOO=1 target/release/pgdt parse | tail -1"),
            "FOO=1 target/release/pgdt parse",
        )


class Hook(unittest.TestCase):
    def event(self, command: str, tool: str = "Bash") -> str:
        return json.dumps({"tool_name": tool, "tool_input": {"command": command}})

    def test_a_refused_command_exits_2_with_the_remedy(self):
        with contextlib.redirect_stderr(io.StringIO()) as err:
            code = contained.hook(self.event("perf record -- ls"))
        self.assertEqual(code, 2)
        self.assertIn("scripts/contained.py", err.getvalue())
        self.assertIn("perf record -- ls", err.getvalue())

    def test_anything_else_exits_0(self):
        self.assertEqual(contained.hook(self.event("ls")), 0)
        self.assertEqual(contained.hook(self.event("perf record -- ls", tool="Read")), 0)
        self.assertEqual(contained.hook("not json"), 0)

    def test_settings_run_this_hook_on_bash(self):
        settings = json.loads((contained.REPO / ".claude/settings.json").read_text())
        hooks = [
            h["command"]
            for entry in settings["hooks"]["PreToolUse"]
            if entry.get("matcher") == "Bash"
            for h in entry["hooks"]
        ]
        self.assertTrue(any("scripts/contained.py" in h and "--hook" in h for h in hooks), hooks)


class Memory(unittest.TestCase):
    def test_sizes(self):
        self.assertEqual(contained.size_bytes("512m"), 512 * 1024**2)
        self.assertEqual(contained.size_bytes("4g"), 4 * 1024**3)
        self.assertEqual(contained.size_bytes("1073741824"), 1024**3)
        with self.assertRaises(ValueError):
            contained.size_bytes("lots")

    def test_a_limit_near_the_machine_s_is_refused(self):
        contained.check_memory("4g", 32 * 1024**3)
        with self.assertRaises(ValueError):
            contained.check_memory("20g", 32 * 1024**3)

    def test_main_refuses_it_before_building_anything(self):
        with unittest.mock.patch.object(contained, "argv") as built, \
                contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(contained.main(["--memory", "100000g", "--", "true"]), 2)
        built.assert_not_called()


class Argv(unittest.TestCase):
    def line(self, command, **kw):
        return contained.argv(command, image="img", container=["docker"], **kw)

    def test_the_limit_is_memory_and_swap_both(self):
        line = self.line(["true"], memory="2g")
        self.assertEqual(line[line.index("-m") + 1], "2g")
        self.assertEqual(line[line.index("--memory-swap") + 1], "2g")
        self.assertEqual(line[line.index("--user") + 1], f"{os.getuid()}:{os.getgid()}")
        self.assertEqual(line[-2:], ["img", "true"])

    def test_every_path_named_is_mounted_by_its_directory_at_its_own_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / "data").mkdir()
            (root / "data/dump.sql").write_text("")
            line = self.line(
                ["pgdt", "parse", f"--source={root}/data/dump.sql", "--dtcache", f"{root}/out/new.pgdt"],
                cwd=root,
            )
        binds = {line[i + 1] for i, w in enumerate(line) if w == "-v"}
        self.assertIn(f"{root}:{root}", binds)
        self.assertIn(f"{contained.REPO}:{contained.REPO}", binds)
        # The file's directory, never the file: a cache is saved by a rename.
        self.assertNotIn(f"{root}/data/dump.sql:{root}/data/dump.sql", binds)

    def test_a_link_is_mounted_at_its_target_and_where_named(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / "real").mkdir()
            (root / "link").symlink_to(root / "real")
            binds = contained.mounts([f"{root}/link/x"], Path("/nonexistent"))
        self.assertIn((root / "real", root / "real"), binds)

    def test_a_path_inside_a_shell_string_is_mounted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            binds = contained.mounts(["sh", "-c", f"cat {root}/f > /dev/null"], Path("/nonexistent"))
        self.assertIn((root, root), binds)

    def test_a_path_under_no_mountable_directory_mounts_nothing(self):
        binds = contained.mounts(["/dump.sql", "/tmp/result.csv"], contained.REPO)
        self.assertFalse([b for b in binds if b[1] in contained.UNMOUNTABLE])

    def test_nothing_under_proc_sys_or_dev_is_mounted_but_staging(self):
        binds = contained.mounts(
            ["cat /proc/self/status /sys/fs/cgroup/memory.max /dev/null", "/dev/shm/pgdt/x"],
            Path("/nonexistent"),
        )
        dests = [str(d) for _, d in binds]
        self.assertFalse([d for d in dests if d.startswith(("/proc", "/sys"))], dests)
        self.assertFalse([d for d in dests if d.startswith("/dev") and not d.startswith("/dev/shm/")], dests)

    def test_the_environment_reaches_the_container_and_its_paths_are_mounted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            line = self.line(["true"], env=[f"OUT={root}/report.txt"])
        self.assertIn(f"OUT={root}/report.txt", line)
        self.assertIn(f"{root}:{root}", line)


if __name__ == "__main__":
    unittest.main()
