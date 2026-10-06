#!/usr/bin/env python3
"""Unit tests for release.py, run with `uv run python -m unittest test_release`.

The floor check is held to `readelf` text of the shape a real binary prints,
and `ThisRepo` holds the committed Dockerfile, toolchain pin and suite to what
`docs/design/roadmap-P29-releases.md` commits them to.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path

import check
import release

# The version-needs section and two symbol lines, as `readelf --version-info
# --dyn-syms --wide` prints them for a host build of `pgdt`.
READELF = """\
Symbol table '.dynsym' contains 3 entries:
   Num:    Value          Size Type    Bind   Vis      Ndx Name
    59: 0000000000000000     0 FUNC    GLOBAL DEFAULT  UND atan2f@GLIBC_2.43 (10)
    60: 0000000000000000     0 FUNC    GLOBAL DEFAULT  UND cosh@GLIBC_2.44 (12)
    61: 0000000000000000     0 FUNC    GLOBAL DEFAULT  UND read@GLIBC_2.2.5 (3)
    62: 0000000000000000     0 FUNC    GLOBAL DEFAULT  UND _Unwind_Resume@GCC_3.0 (4)

Version symbols section '.gnu.version' contains 3 entries:
 Addr: 0x0000000000001000  Offset: 0x00001000  Link: 7 (.dynsym)
  000:   0 (*local*)       2 (GLIBC_2.34)

Version needs section '.gnu.version_r' contains 3 entries:
 Addr: 0x0000000000001ca4  Offset: 0x00001ca4  Link: 8 (.dynstr)
  000000: Version: 1  File: libgcc_s.so.1  Cnt: 1
  0x0040:   Name: GCC_3.0  Flags: none  Version: 4
  0x0010: Version: 1  File: libm.so.6  Cnt: 3
  0x0070:   Name: GLIBC_2.2.5  Flags: none  Version: 9
  0x00a0:   Name: GLIBC_2.43  Flags: none  Version: 10
  0x00b0:   Name: GLIBC_2.44  Flags: none  Version: 12
  0x0020: Version: 1  File: libc.so.6  Cnt: 2
  0x00c0:   Name: GLIBC_2.2.5  Flags: none  Version: 3
  0x01e0:   Name: GLIBC_2.34  Flags: none  Version: 2

"""


def with_needs(*names: str, file: str = "libc.so.6") -> str:
    lines = [
        "Version needs section '.gnu.version_r' contains 1 entries:",
        f"  000000: Version: 1  File: {file}  Cnt: {len(names)}",
        *(f"  0x0010:   Name: {n}  Flags: none  Version: 2" for n in names),
        "",
    ]
    return "\n".join(lines)


class Needs(unittest.TestCase):
    def test_every_glibc_need_is_read_with_its_library(self):
        needs = release.glibc_needs(READELF)
        self.assertEqual(
            needs,
            [
                release.Need("libm.so.6", "GLIBC_2.2.5"),
                release.Need("libm.so.6", "GLIBC_2.43"),
                release.Need("libm.so.6", "GLIBC_2.44"),
                release.Need("libc.so.6", "GLIBC_2.2.5"),
                release.Need("libc.so.6", "GLIBC_2.34"),
            ],
        )

    def test_another_librarys_versions_are_not_glibcs(self):
        self.assertNotIn("GCC_3.0", [n.name for n in release.glibc_needs(READELF)])

    def test_the_version_symbols_section_is_not_read_as_needs(self):
        # `.gnu.version` names GLIBC_2.34 too, outside the needs section.
        text = READELF.split("Version needs section")[0]
        self.assertEqual(release.glibc_needs(text), [])

    def test_a_release_is_compared_numerically(self):
        self.assertEqual(release.Need("libc.so.6", "GLIBC_2.2.5").version, (2, 2, 5))
        self.assertLess(
            release.Need("libc.so.6", "GLIBC_2.9").version,
            release.Need("libc.so.6", "GLIBC_2.10").version,
        )
        self.assertIsNone(release.Need("libc.so.6", "GLIBC_PRIVATE").version)

    def test_symbols_are_grouped_by_version(self):
        syms = release.symbols_by_version(READELF)
        self.assertEqual(syms["GLIBC_2.43"], ["atan2f"])
        self.assertEqual(syms["GLIBC_2.44"], ["cosh"])
        self.assertNotIn("GCC_3.0", syms)


class Floor(unittest.TestCase):
    def test_a_binary_at_or_below_the_floor_passes(self):
        ok, lines = release.check_floor(READELF, (2, 44))
        self.assertTrue(ok)
        self.assertEqual(lines, ["needs GLIBC_2.44 at most (libm.so.6); floor glibc 2.44: ok"])

    def test_a_need_past_the_floor_fails_naming_its_symbols(self):
        ok, lines = release.check_floor(READELF, (2, 41))
        self.assertFalse(ok)
        self.assertEqual(
            lines,
            [
                "needs past the floor glibc 2.41:",
                "  GLIBC_2.43 (libm.so.6): atan2f",
                "  GLIBC_2.44 (libm.so.6): cosh",
            ],
        )

    def test_glibc_private_fails_at_any_floor(self):
        ok, lines = release.check_floor(with_needs("GLIBC_2.2.5", "GLIBC_PRIVATE"), (9, 99))
        self.assertFalse(ok)
        self.assertIn("GLIBC_PRIVATE", lines[1])

    def test_a_binary_with_no_glibc_needs_fails(self):
        # A static binary, or text that is not readelf's: the check proves
        # nothing about it, so it does not pass it.
        ok, lines = release.check_floor("Symbol table '.dynsym' contains 0 entries:\n", (2, 41))
        self.assertFalse(ok)
        self.assertIn("not a dynamically linked glibc binary", lines[0])

    def test_a_debian_package_version_names_its_glibc(self):
        self.assertEqual(release.deb_glibc("2.41-12+deb13u4"), (2, 41))
        self.assertEqual(release.deb_glibc("2.41-11cross1"), (2, 41))
        self.assertEqual(release.deb_glibc("1:2.36-9\n"), (2, 36))
        with self.assertRaises(release.ReleaseError):
            release.deb_glibc("unknown")

    def test_a_stated_floor_is_a_release(self):
        self.assertEqual(release.parse_floor("2.41"), (2, 41))
        for bad in ("2", "2.41a", "trixie"):
            with self.assertRaises(release.ReleaseError):
                release.parse_floor(bad)

    @unittest.skipUnless(shutil.which("readelf"), "readelf is not installed")
    def test_readelf_is_read_as_it_prints(self):
        # Whatever this interpreter links, its needs parse and it passes a
        # floor of its own glibc's release or later.
        import os

        binary = Path(os.path.realpath(shutil.which("python3") or "/bin/sh"))
        text = release.readelf_of(binary)
        needs = release.glibc_needs(text)
        self.assertTrue(needs, "a dynamically linked binary names a GLIBC_ version")
        top = max(n.version for n in needs if n.version)
        self.assertTrue(release.check_floor(text, top)[0])


class Image(unittest.TestCase):
    INPUTS = {"Dockerfile": b"FROM x\n", "rust-toolchain.toml": b"a", "mise.toml": b"b"}

    def test_the_tag_moves_with_each_input(self):
        tag = release.image_tag(self.INPUTS)
        self.assertRegex(tag, r"^pgdt-release:[0-9a-f]{16}$")
        self.assertEqual(release.image_tag(dict(self.INPUTS)), tag)
        for name in self.INPUTS:
            moved = dict(self.INPUTS, **{name: self.INPUTS[name] + b"\n"})
            self.assertNotEqual(release.image_tag(moved), tag, name)

    def test_two_files_cannot_trade_bytes_for_one_tag(self):
        a = {"Dockerfile": b"ab", "mise.toml": b""}
        b = {"Dockerfile": b"a", "mise.toml": b"b"}
        self.assertNotEqual(release.image_tag(a), release.image_tag(b))

    def test_the_base_must_be_pinned_by_digest(self):
        pinned = "debian:trixie@sha256:" + "0" * 64
        self.assertEqual(release.pinned_base(f"# c\nFROM {pinned}\nRUN x\n"), pinned)
        for bad in ("FROM debian:trixie\n", f"FROM {pinned}\nFROM {pinned}\n", "RUN x\n"):
            with self.assertRaises(release.ReleaseError):
                release.pinned_base(bad)

    def test_a_step_runs_as_the_user_under_a_limit_with_state_of_its_own(self):
        with tempfile.TemporaryDirectory() as tmp:
            argv = release.run_in_image(
                ["docker"], "pgdt-release:x", ["suite"], state=Path(tmp), memory="8g", uid=1000, gid=100
            )
        joined = " ".join(argv)
        self.assertEqual(argv[:3], ["docker", "run", "--rm"])
        self.assertIn("-m 8g --memory-swap 8g", joined)
        self.assertIn("--user 1000:100", joined)
        self.assertIn(f"-v {release.REPO}:/work", joined)
        self.assertIn(f"-v {Path(tmp).resolve()}/target:/state/target", joined)
        self.assertIn("-e CARGO_TARGET_DIR=/state/target", joined)
        self.assertIn("-e UV_PROJECT_ENVIRONMENT=/state/uv/venv", joined)
        self.assertEqual(argv[-5:], ["pgdt-release:x", "python3", "scripts/release.py", "suite", "--inside"])

    def test_a_step_may_unshare_and_reads_the_git_view_it_is_given(self):
        with tempfile.TemporaryDirectory() as tmp:
            plain = release.run_in_image(["docker"], "t", ["suite"], state=Path(tmp))
            viewed = release.run_in_image(
                ["docker"], "t", ["suite"], state=Path(tmp), git=Path(tmp) / "git"
            )
        self.assertIn("--security-opt seccomp=unconfined", " ".join(plain))
        self.assertNotIn("/work/.git", " ".join(plain))
        self.assertIn(f"-v {Path(tmp).resolve()}/git:/work/.git", " ".join(viewed))

    def test_the_git_view_drops_only_an_unreadable_extension(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, state = Path(tmp) / "repo", Path(tmp) / "state"
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            self.assertIsNone(release.git_view(state, repo))
            config = repo / ".git" / "config"
            subprocess.run(
                ["git", "config", "--file", str(config), "extensions.relativeworktrees", "true"],
                check=True,
            )
            before = config.read_text()
            view = release.git_view(state, repo)
            self.assertEqual(view, state / "git")
            self.assertEqual(config.read_text(), before, "the original is never touched")
            got = lambda key, f: subprocess.run(
                ["git", "config", "--file", str(f), "--get", key], capture_output=True, text=True
            ).stdout.strip()  # noqa: E731
            self.assertEqual(got("extensions.relativeworktrees", view / "config"), "")
            self.assertEqual(got("core.bare", view / "config"), got("core.bare", config))
            self.assertTrue((view / "HEAD").is_file())

    def test_inside_refuses_an_image_built_from_other_pins(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, copies = Path(tmp) / "repo", Path(tmp) / "copies"
            repo.mkdir()
            copies.mkdir()
            for name in release.CONTEXT_FILES:
                (repo / name).write_text(name)
                (copies / name).write_text(name)
            release.check_inside(copies, repo)
            (copies / "mise.toml").write_text("moved")
            with self.assertRaisesRegex(release.ReleaseError, "another mise.toml"):
                release.check_inside(copies, repo)
            (copies / "mise.toml").unlink()
            with self.assertRaisesRegex(release.ReleaseError, "not the release image"):
                release.check_inside(copies, repo)


class ThisRepo(unittest.TestCase):
    def test_the_dockerfile_builds_from_one_debian_digest(self):
        base = release.pinned_base(release.DOCKERFILE.read_text())
        self.assertTrue(base.startswith("debian:trixie@sha256:"), base)

    def test_the_dockerfile_copies_exactly_the_tagged_inputs(self):
        copied = re.findall(r"^COPY\s+(\S+)\s", release.DOCKERFILE.read_text(), re.MULTILINE)
        self.assertEqual(sorted(copied), sorted(release.CONTEXT_FILES))

    def test_every_target_has_its_cross_package_installed(self):
        text = release.DOCKERFILE.read_text()
        for package in release.TARGETS.values():
            self.assertRegex(text, rf"\s{re.escape(package)}\s")

    def test_the_toolchain_is_an_exact_release_carrying_every_cross_target(self):
        tc = tomllib.loads((release.REPO / "rust-toolchain.toml").read_text())["toolchain"]
        self.assertRegex(tc["channel"], r"^\d+\.\d+\.\d+$")
        for target in release.TARGETS:
            if target != release.NATIVE_TARGET:
                self.assertIn(target, tc["targets"])

    def test_the_suite_is_the_rounds(self):
        names = {c.argv: c.name for c in check.CHECKS}
        self.assertEqual([names[a] for a in release.SUITE], ["nextest", "doctest"])

    def test_the_build_is_locked_and_per_target(self):
        argv = release.build_argv("aarch64-unknown-linux-gnu")
        self.assertIn("--locked", argv)
        self.assertEqual(argv[argv.index("--target") + 1], "aarch64-unknown-linux-gnu")

    def test_help_runs(self):
        out = subprocess.run(
            ["python3", "release.py", "--help"],
            cwd=Path(__file__).parent, capture_output=True, text=True, check=True,
        ).stdout  # fmt: skip
        self.assertIn("floor", out)


if __name__ == "__main__":
    unittest.main()
