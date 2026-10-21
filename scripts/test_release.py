#!/usr/bin/env python3
"""Unit tests for release.py, run with `uv run python -m unittest test_release`.

The floor check is held to `readelf` text of the shape a real binary prints,
and `ThisRepo` holds the committed Dockerfile, toolchain pin and suite to what
`docs/design/decisions.md`, "D104", commits them to.
"""

from __future__ import annotations

import contextlib
import io
import json
import re
import shutil
import subprocess
import tempfile
import tomllib
import unittest
import unittest.mock
from pathlib import Path
from typing import Sequence

import check
import release

#: The round's nextest run, which the release suite starts with.
NEXTEST: tuple[str, ...] = next(c.argv for c in check.CHECKS if c.name == "nextest")

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

    def test_the_host_names_nothing_to_a_step_but_what_it_is_asked_to(self):
        with tempfile.TemporaryDirectory() as tmp:
            bare = release.run_in_image(["docker"], "t", ["build"], state=Path(tmp))
            named = release.run_in_image(
                ["docker"], "t", ["build"], state=Path(tmp), env={release.RELEASE_VARIABLE: "0.1.0"}
            )
        self.assertNotIn(release.RELEASE_VARIABLE, " ".join(bare))
        self.assertIn(f"-e {release.RELEASE_VARIABLE}=0.1.0", " ".join(named))
        self.assertEqual(named[-5:], bare[-5:], "the step is the same, and still last")

    def test_a_step_may_unshare_and_reads_the_git_view_it_is_given(self):
        with tempfile.TemporaryDirectory() as tmp:
            plain = release.run_in_image(["docker"], "t", ["suite"], state=Path(tmp))
            viewed = release.run_in_image(
                ["docker"], "t", ["suite"], state=Path(tmp), git=Path(tmp) / "git"
            )
        self.assertIn("--security-opt seccomp=unconfined", " ".join(plain))
        self.assertNotIn("/work/.git", " ".join(plain))
        self.assertIn(f"-v {Path(tmp).resolve()}/git:/work/.git", " ".join(viewed))

    def test_another_checkout_is_mounted_where_the_tree_would_be(self):
        with tempfile.TemporaryDirectory() as tmp:
            argv = release.run_in_image(
                ["docker"], "t", ["build"], state=Path(tmp), tree=Path(tmp) / "worktree"
            )
        joined = " ".join(argv)
        self.assertIn(f"-v {Path(tmp) / 'worktree'}:/work", joined)
        self.assertNotIn(f"-v {release.REPO}:/work", joined)
        self.assertEqual(argv[-5:], ["t", "python3", "scripts/release.py", "build", "--inside"])

    def test_cpu_lists_round_trip(self):
        self.assertEqual(release.parse_cpu_list("0-2,12-14"), frozenset({0, 1, 2, 12, 13, 14}))
        self.assertEqual(release.parse_cpu_list("5"), frozenset({5}))
        self.assertEqual(release.cpu_list({3, 4, 5, 15, 16, 17}), "3-5,15-17")
        self.assertEqual(release.cpu_list({1, 3}), "1,3")

    @staticmethod
    def smt_sysfs(root: Path, cores: int) -> frozenset[int]:
        """A sysfs of `cores` two-thread cores, siblings `n` and `n + cores`,
        as this machine's are."""
        for core in range(cores):
            for cpu in (core, core + cores):
                topology = root / f"cpu{cpu}" / "topology"
                topology.mkdir(parents=True)
                (topology / "thread_siblings_list").write_text(f"{core},{core + cores}\n")
        return frozenset(range(2 * cores))

    def test_a_run_leaves_the_last_physical_core_whole(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cpus = self.smt_sysfs(root, 12)
            self.assertEqual(release.physical_cores(cpus, root)[-1], frozenset({11, 23}))
            given = release.run_cpus(1, cpus, root)
        self.assertEqual(release.cpu_list(given), "0-10,12-22")
        self.assertEqual(release.run_cpus(0, cpus, root), None, "a timed run is given every CPU")

    def test_a_reserve_leaving_no_core_gives_every_cpu(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cpus = self.smt_sysfs(root, 1)
            self.assertEqual(release.run_cpus(1, cpus, root), None)
            self.assertEqual(release.run_cpus(1, frozenset({0, 1}), root / "absent"), frozenset({0}))

    def test_a_reserve_is_a_cpuset_and_none_is_no_cpuset(self):
        with tempfile.TemporaryDirectory() as tmp:
            reserved = release.run_in_image(["docker"], "t", ["suite"], state=Path(tmp), reserve=1)
            timed = release.run_in_image(["docker"], "t", ["suite"], state=Path(tmp), reserve=0)
        self.assertNotIn("--cpuset-cpus", timed)
        given = release.run_cpus(1)
        if given is None:
            self.assertNotIn("--cpuset-cpus", reserved)
        else:
            at = reserved.index("--cpuset-cpus")
            self.assertEqual(reserved[at + 1], release.cpu_list(given))
        self.assertEqual(reserved[-5:], timed[-5:], "the step is the same, and still last")

    def test_a_target_dir_of_its_own_replaces_the_state_s_alone(self):
        with tempfile.TemporaryDirectory() as tmp:
            own = Path(tmp) / "legs" / "system"
            argv = release.run_in_image(
                ["docker"], "t", ["build"], state=Path(tmp) / "state", target=own
            )
        joined = " ".join(argv)
        self.assertIn(f"-v {own.resolve()}:/state/target", joined)
        self.assertNotIn(f"{(Path(tmp) / 'state').resolve()}/target:", joined)
        self.assertIn(f"-v {(Path(tmp) / 'state').resolve()}/cargo-home:/state/cargo-home", joined)

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


def about(*licenses: tuple[str, str, list[tuple[str, str]]], crates: Sequence[dict] = ()) -> dict:
    """`cargo-about`'s JSON, as much of it as the notices read: each licence
    text with its id and the crates it was reproduced for."""
    return {
        "licenses": [
            {"id": id_, "text": text, "used_by": [{"crate": {"name": n, "version": v}} for n, v in users]}
            for id_, text, users in licenses
        ],
        "crates": list(crates),
    }


def package(name: str, version: str, manifest: str = "/x/Cargo.toml", **extra) -> dict:
    return {"package": {"name": name, "version": version, "manifest_path": manifest, **extra}}


C = release.Crate


class Notices(unittest.TestCase):
    def test_the_closure_is_read_out_of_cargo_tree(self):
        tree = "pgdt v0.1.0 (/work/pgdt)\nserde v1.0.229\nserde v1.0.229 (*)\nzstd-sys v2.1.0+zstd.1.5.7\n\n"
        self.assertEqual(
            release.parse_tree(tree),
            {C("pgdt", "0.1.0"), C("serde", "1.0.229"), C("zstd-sys", "2.1.0+zstd.1.5.7")},
        )

    def test_texts_are_cut_to_the_closure_and_kept_once_each(self):
        reading = about(
            ("Apache-2.0", "ATTRIBUTION", [("aws-lc-sys", "0.45.0")]),
            ("ISC", "ATTRIBUTION", [("aws-lc-sys", "0.45.0")]),
            ("MIT", "MIT TEXT", [("a", "1.0.0"), ("macro", "1.0.0")]),
            ("MIT", "ONLY A MACRO", [("macro", "1.0.0")]),
        )
        closure = {C("aws-lc-sys", "0.45.0"), C("a", "1.0.0")}
        texts, licences = release.licence_texts(reading, closure)
        self.assertEqual([(t.text, t.licenses, t.crates) for t in texts], [
            ("ATTRIBUTION", {"Apache-2.0", "ISC"}, {C("aws-lc-sys", "0.45.0")}),
            ("MIT TEXT", {"MIT"}, {C("a", "1.0.0")}),
        ])  # fmt: skip
        self.assertEqual(licences, {C("aws-lc-sys", "0.45.0"): {"Apache-2.0", "ISC"}, C("a", "1.0.0"): {"MIT"}})

    def test_a_linked_crate_with_no_text_is_refused(self):
        reading = about(("MIT", "MIT TEXT", [("a", "1.0.0")]))
        with self.assertRaisesRegex(release.ReleaseError, r"b 2\.0\.0"):
            release.licence_texts(reading, {C("a", "1.0.0"), C("b", "2.0.0")})

    def test_every_notice_at_a_linked_crate_s_root_is_kept_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            dirs = {}
            for name, files in (
                ("arrow", {"NOTICE.txt": "ASF", "LICENSE.txt": "L"}),
                ("arrow-array", {"NOTICE.txt": "ASF"}),
                ("object_store", {"notice": "OBJ"}),
                ("unlinked", {"NOTICE": "NOT SHIPPED"}),
            ):
                root = Path(tmp) / name
                root.mkdir()
                for file, text in files.items():
                    (root / file).write_text(text)
                dirs[C(name, "1.0.0")] = root
            closure = {C("arrow", "1.0.0"), C("arrow-array", "1.0.0"), C("object_store", "1.0.0")}
            got = release.notice_texts(dirs, closure)
        self.assertEqual([(t.text, t.crates) for t in got], [
            ("ASF", {C("arrow", "1.0.0"), C("arrow-array", "1.0.0")}),
            ("OBJ", {C("object_store", "1.0.0")}),
        ])  # fmt: skip

    def test_a_source_pointer_names_each_crate_under_a_licence_asking_for_one(self):
        reading = about(crates=[package("option-ext", "0.2.0", repository="https://example.org/oe")])
        licences = {C("option-ext", "0.2.0"): {"MPL-2.0"}, C("a", "1.0.0"): {"MIT"}}
        self.assertEqual(
            release.source_pointers(reading, licences),
            ["option-ext 0.2.0 (MPL-2.0): https://crates.io/crates/option-ext/0.2.0, repository https://example.org/oe"],
        )

    def test_an_election_for_a_crate_no_longer_linked_is_refused(self):
        self.assertEqual(release.elections({C("zstd-sys", "2.1.0")}), [release.ELECTIONS["zstd-sys"]])
        with self.assertRaisesRegex(release.ReleaseError, "zstd-sys"):
            release.elections({C("a", "1.0.0")})

    def test_the_notices_carry_every_section_inside_the_width(self):
        texts = [release.Text("MIT TEXT", {"MIT"}, {C(f"crate-{i}", "1.0.0") for i in range(30)})]
        notices = [release.Text("ASF NOTICE", set(), {C("arrow", "59.2.0")})]
        out = release.render_notices(
            "0.1.0", "x86_64-unknown-linux-gnu", {C("arrow", "59.2.0"): {"Apache-2.0", "MIT"}},
            texts, notices, ["option-ext 0.2.0 (MPL-2.0): https://crates.io/crates/option-ext/0.2.0"],
            [release.ELECTIONS["zstd-sys"]],
        )  # fmt: skip
        self.assertTrue(out.startswith("Third-party notices for pgdt 0.1.0, x86_64-unknown-linux-gnu\n"))
        for section in ("Components", "Elections", "Source code", "Licence texts", "NOTICE files"):
            self.assertIn(f"\n{section}\n", out)
        self.assertIn("arrow 59.2.0: Apache-2.0 AND MIT", out)
        self.assertIn("MIT TEXT", out)
        self.assertIn("ASF NOTICE", out)
        self.assertIn("BSD-3-Clause, not GPL-2.0-only", out.replace("\n", " "))
        # Every line but a crates.io URL wraps; the texts are the crates' own.
        long = [line for line in out.splitlines() if len(line) > release.WIDTH and "https://" not in line]
        self.assertEqual(long, [])

    def test_a_clarified_file_missing_from_the_notices_is_refused(self):
        # `cargo-about` drops a clarification whose checksum moved with only a
        # warning, falling back to the declared licence: the 0BSD text goes.
        crate = C("liblzma-sys", "0.4.8")
        config = {"liblzma-sys": {"clarify": {"files": [
            {"path": "LICENSE-APACHE", "license": "Apache-2.0"},
            {"path": "xz/COPYING.0BSD", "license": "0BSD"},
        ]}}}  # fmt: skip
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "xz").mkdir()
            (root / "LICENSE-APACHE").write_text("APACHE")
            (root / "xz" / "COPYING.0BSD").write_text("ZERO BSD")
            dirs = {crate: root, C("other", "1.0.0"): root}
            whole = [release.Text("APACHE", {"Apache-2.0"}, {crate}), release.Text("ZERO BSD", {"0BSD"}, {crate})]
            release.check_clarified(config, dirs, whole)
            with self.assertRaisesRegex(release.ReleaseError, r"liblzma-sys 0\.4\.8's xz/COPYING\.0BSD"):
                release.check_clarified(config, dirs, whole[:1])
            # Another crate's copy of the text is not this crate's.
            elsewhere = [whole[0], release.Text("ZERO BSD", {"0BSD"}, {C("other", "1.0.0")})]
            with self.assertRaises(release.ReleaseError):
                release.check_clarified(config, dirs, elsewhere)
            config["liblzma-sys"]["clarify"]["files"][0]["start"] = "APA"
            with self.assertRaisesRegex(release.ReleaseError, "whole files"):
                release.check_clarified(config, dirs, whole)

    def test_every_file_this_repo_clarifies_names_a_checksum_and_no_cut(self):
        config = tomllib.loads(release.ABOUT_CONFIG.read_text())
        clarified = {name: t["clarify"] for name, t in config.items() if isinstance(t, dict) and "clarify" in t}
        self.assertEqual(sorted(clarified), ["aws-lc-sys", "liblzma-sys", "ring"])
        for name, clarify in clarified.items():
            for file in clarify["files"]:
                self.assertRegex(file["checksum"], r"^[0-9a-f]{64}$", name)
                self.assertFalse({"start", "end"} & file.keys(), name)

    def test_the_notices_are_written_beside_the_binary(self):
        self.assertEqual(
            release.notices_path(release.NATIVE_TARGET, Path("/t")),
            release.Build().binary(Path("/t")).parent / "THIRD-PARTY-NOTICES",
        )

    def test_a_notices_step_round_trips_through_the_command_line(self):
        seen = []
        with unittest.mock.patch.object(release, "check_inside"), \
                unittest.mock.patch.object(release, "step_notices", lambda t: seen.append(t) or 0):
            self.assertEqual(release.main(["notices", "--inside"]), 0)
            self.assertEqual(release.main(["notices", "--target", release.NATIVE_TARGET, "--inside"]), 0)
        self.assertEqual(seen, [list(release.TARGETS), [release.NATIVE_TARGET]])


#: Licences whose terms reach past attribution: none may be accepted for every
#: crate, so a second crate under one stops a release until it is read.
COPYLEFT = re.compile(r"^(A?GPL|LGPL|MPL|EPL|CDDL|EUPL|OSL|CC-BY-SA)")


class ThisRepo(unittest.TestCase):
    def test_no_copyleft_licence_is_accepted_for_every_crate(self):
        config = tomllib.loads(release.ABOUT_CONFIG.read_text())
        self.assertEqual([lic for lic in config["accepted"] if COPYLEFT.match(lic)], [])
        self.assertEqual(config["option-ext"]["accepted"], ["MPL-2.0"])

    def test_every_mise_tool_is_pinned_exactly_and_is_what_the_image_links(self):
        tools = tomllib.loads((release.REPO / "mise.toml").read_text())["tools"]
        for tool, version in tools.items():
            self.assertRegex(version, r"^\d+\.\d+\.\d+$", tool)
        linked = re.search(r"for tool in ([^;]*);", release.DOCKERFILE.read_text())
        self.assertIsNotNone(linked)
        # A tool's binary is its name's last segment; `uv` ships `uvx` beside it.
        binaries = {re.split(r"[:/]", tool)[-1] for tool in tools}
        self.assertEqual(set(linked.group(1).split()), binaries | {"uvx"})

    def test_every_member_is_apache_and_unpublishable_and_the_root_carries_the_text(self):
        root = tomllib.loads((release.REPO / "Cargo.toml").read_text())
        self.assertEqual(root["workspace"]["package"]["license"], "Apache-2.0")
        self.assertIs(root["workspace"]["package"]["publish"], False)
        members = [
            str(path.relative_to(release.REPO))
            for m in root["workspace"]["members"]
            for path in sorted(release.REPO.glob(m))
        ]
        names = {Path(member).name for member in members}
        for member in members:
            manifest = tomllib.loads((release.REPO / member / "Cargo.toml").read_text())
            self.assertEqual(manifest["package"]["license"], {"workspace": True}, member)
            self.assertEqual(manifest["package"]["publish"], {"workspace": True}, member)
            for table in ("dependencies", "dev-dependencies", "build-dependencies"):
                for dep, spec in manifest.get(table, {}).items():
                    if isinstance(spec, dict) and "path" in spec and dep in names:
                        self.assertNotIn("version", spec, f"{member}'s {table}.{dep}")
        self.assertIn("Apache License\n", (release.REPO / "LICENSE").read_text())

    def test_the_apparatus_the_suite_leaves_out_is_a_dependency_of_no_member(self):
        manifests = {
            path.parent.name: tomllib.loads(path.read_text())
            for path in sorted(release.REPO.glob("crates/*/Cargo.toml"))
        }
        apparatus = {
            name
            for name, m in manifests.items()
            if m.get("package", {}).get("metadata", {}).get("pgdt", {}).get("apparatus") is True
        }
        self.assertTrue(apparatus)
        self.assertNotIn(release.SHIPPED, apparatus)
        for name, m in manifests.items():
            for table in ("dependencies", "dev-dependencies", "build-dependencies"):
                self.assertFalse(apparatus & set(m.get(table, {})), f"{name}'s {table}")

    def test_the_notices_read_the_shipped_package_s_own_manifest(self):
        manifest = tomllib.loads(release.SHIPPED_MANIFEST.read_text())
        self.assertEqual(manifest["package"]["name"], release.SHIPPED)

    def test_the_dockerfile_builds_from_one_debian_digest(self):
        base = release.pinned_base(release.DOCKERFILE.read_text())
        self.assertTrue(base.startswith("debian:trixie@sha256:"), base)

    def test_the_dockerfile_copies_exactly_the_tagged_inputs(self):
        copied = re.findall(r"^COPY\s+(\S+)\s", release.DOCKERFILE.read_text(), re.MULTILINE)
        self.assertEqual(sorted(copied), sorted(release.CONTEXT_FILES))

    def test_every_target_has_its_cross_package_installed(self):
        text = release.DOCKERFILE.read_text()
        for package in release.TARGETS.values():
            self.assertRegex(text, rf"(?<![\w-]){re.escape(package)}(?![\w-])")

    def test_the_toolchain_is_an_exact_release_carrying_every_cross_target(self):
        tc = tomllib.loads((release.REPO / "rust-toolchain.toml").read_text())["toolchain"]
        self.assertRegex(tc["channel"], r"^\d+\.\d+\.\d+$")
        for target in release.TARGETS:
            if target != release.NATIVE_TARGET:
                self.assertIn(target, tc["targets"])

    def test_the_suite_is_the_rounds_its_doctests_run_only_for_a_library_holding_one(self):
        lib = [check.Package("base", "base", frozenset(), (check.Target("base", "lib", ("lib.rs",)),))]
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "lib.rs").write_text("/// A function.\npub fn f() {}\n")
            self.assertEqual(release.suite(Path(tmp), lambda _: lib), [NEXTEST])
            Path(tmp, "lib.rs").write_text("/// ```\n/// assert!(true);\n/// ```\npub fn f() {}\n")
            self.assertEqual(
                release.suite(Path(tmp), lambda _: lib),
                [NEXTEST, check.doctest_check(["base"]).argv],
            )
        names = {c.argv: c.name for c in check.CHECKS}
        self.assertEqual(names[NEXTEST], "nextest")

    def test_the_suite_leaves_out_the_measurement_apparatus(self):
        def lib(name: str, apparatus: bool) -> check.Package:
            target = check.Target(name, "lib", (f"{name}.rs",))
            return check.Package(name, name, frozenset(), (target,), apparatus=apparatus)

        packages = [lib("base", False), lib("meter", True), lib("gauge", True)]
        with tempfile.TemporaryDirectory() as tmp:
            for name in ("base", "meter", "gauge"):
                Path(tmp, f"{name}.rs").write_text("/// ```\n/// assert!(true);\n/// ```\npub fn f() {}\n")
            self.assertEqual(
                release.suite(Path(tmp), lambda _: packages),
                [
                    (*NEXTEST, "--exclude", "meter", "--exclude", "gauge"),
                    check.doctest_check(["base"]).argv,
                ],
            )

    def test_the_build_is_locked_and_per_target(self):
        argv = release.Build("aarch64-unknown-linux-gnu").cargo_argv()
        self.assertIn("--locked", argv)
        self.assertEqual(argv[argv.index("--target") + 1], "aarch64-unknown-linux-gnu")

    def test_a_variant_is_the_release_build_with_its_features_changed(self):
        shipped = release.Build().cargo_argv()
        leg = release.Build(features=("system",), default_features=False).cargo_argv()
        self.assertEqual(leg[: len(shipped)], shipped)
        self.assertEqual(leg[len(shipped):], ["--no-default-features", "--features", "system"])
        example = release.Build(package="pgdt-io", example="xz_decode")
        self.assertIn("--example", example.cargo_argv())
        self.assertEqual(
            example.binary(Path("/t")),
            Path(f"/t/{release.NATIVE_TARGET}/release/examples/xz_decode"),
        )
        self.assertEqual(release.Build().binary(Path("/t")), Path(f"/t/{release.NATIVE_TARGET}/release/pgdt"))

    def test_a_profile_leg_is_the_release_build_under_another_profile(self):
        # Cargo refuses `--release` beside `--profile`, so the one replaces the
        # other and nothing else moves; the binary lands in the profile's own
        # directory, which is where `measure.ensure_profile_binary` copies from.
        shipped = release.Build().cargo_argv()
        leg = release.Build(profile="release-fat").cargo_argv()
        at = shipped.index("--release")
        self.assertEqual(leg, [*shipped[:at], "--profile", "release-fat", *shipped[at + 1:]])
        self.assertEqual(
            release.Build(profile="release-fat").binary(Path("/t")),
            Path(f"/t/{release.NATIVE_TARGET}/release-fat/pgdt"),
        )
        self.assertNotIn("--profile=release", release.build_step([release.Build()]))

    def test_a_build_step_round_trips_through_the_command_line(self):
        for build in (
            release.Build(features=("introspect",)),
            release.Build(profile="release-thin"),
        ):
            with self.subTest(build=build):
                step = release.build_step([build])
                seen = []
                with unittest.mock.patch.object(release, "check_inside"), \
                        unittest.mock.patch.object(release, "step_build", lambda b: seen.extend(b) or 0):
                    self.assertEqual(release.main([*step, "--inside"]), 0)
                self.assertEqual(seen, [build])

    def test_one_step_runs_one_build_per_target(self):
        both = [release.Build(t) for t in release.TARGETS]
        self.assertEqual(release.build_step(both)[1:3], [f"--target={t}" for t in release.TARGETS])
        with self.assertRaises(release.ReleaseError):
            release.build_step([release.Build(), release.Build(features=("system",))])

    def test_a_bench_is_built_and_run_as_the_release_build_is(self):
        bench = release.Bench("pgdt-codec", "decoders", "nested")
        argv = bench.cargo_argv()
        self.assertEqual(argv[:2], ["cargo", "bench"])
        self.assertIn("--locked", argv)
        self.assertEqual(argv[argv.index("--target") + 1], release.NATIVE_TARGET)
        # criterion's filter goes to the harness, after cargo's own flags.
        self.assertEqual(argv[-2:], ["--", "nested"])
        built = bench.cargo_argv(no_run=True)
        self.assertEqual(built[: argv.index("--")], argv[: argv.index("--")])
        self.assertIn("--no-run", built)
        self.assertNotIn("nested", built)
        self.assertEqual(bench.criterion_root(Path("/t")), Path("/t/criterion"))

    def test_a_bench_step_round_trips_through_the_command_line(self):
        for bench in (release.Bench("pgdt-codec", "decoders", "nested"),
                      release.Bench("pgdt-query", "whole_file")):
            seen = []
            with unittest.mock.patch.object(release, "check_inside"), \
                    unittest.mock.patch.object(release, "step_bench", lambda b: seen.append(b) or 0):
                self.assertEqual(release.main([*bench.step(), "--inside"]), 0)
            self.assertEqual(seen, [bench])

    def test_a_bench_s_executable_is_read_out_of_cargo_s_messages(self):
        def artifact(name, kind, executable):
            return json.dumps({
                "reason": "compiler-artifact",
                "target": {"name": name, "kind": [kind]},
                "executable": executable,
            })  # fmt: skip

        messages = "\n".join([
            artifact("pgdt_codec", "lib", None),
            artifact("decoders", "bench", "/t/x/release/deps/decoders-0a1b"),
            artifact("whole_file", "bench", "/t/x/release/deps/whole_file-2c3d"),
            json.dumps({"reason": "build-finished", "success": True}),
            "not json",
        ])  # fmt: skip
        self.assertEqual(
            release.bench_executables(messages, "decoders"),
            [Path("/t/x/release/deps/decoders-0a1b")],
        )
        self.assertEqual(release.bench_executables(messages, "absent"), [])

    def test_a_bench_past_the_floor_is_not_run(self):
        bench = release.Bench("pgdt-codec", "decoders", "nested")
        built = json.dumps({
            "reason": "compiler-artifact",
            "target": {"name": "decoders", "kind": ["bench"]},
            "executable": "/t/decoders-0a1b",
        })  # fmt: skip
        runs = []

        def fake_run(argv, **_kwargs):
            runs.append(list(argv))
            return subprocess.CompletedProcess(argv, 0, stdout=built)

        for floor, status, ran in (((2, 41), 0, 2), ((2, 33), 1, 1)):
            runs.clear()
            with unittest.mock.patch.object(release.subprocess, "run", fake_run), \
                    unittest.mock.patch.object(
                        release, "readelf_of", return_value=with_needs("GLIBC_2.2.5", "GLIBC_2.34")
                    ), \
                    unittest.mock.patch.object(release, "image_floor", return_value=floor), \
                    contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(release.step_bench(bench), status)
            self.assertEqual(len(runs), ran)
            self.assertIn("--no-run", runs[0])
        self.assertEqual(runs[0], bench.cargo_argv(no_run=True))

    def test_a_release_build_hands_the_container_the_workspace_version_and_no_other_does(self):
        ran: list[list[str]] = []

        def host(args: list[str]) -> list[str]:
            ran.clear()
            with tempfile.TemporaryDirectory() as tmp, \
                    unittest.mock.patch.object(release, "ensure_image", return_value="t"), \
                    unittest.mock.patch.object(release, "prepare_state"), \
                    unittest.mock.patch.object(release, "git_view", return_value=None), \
                    unittest.mock.patch.object(release, "STATE", Path(tmp)), \
                    unittest.mock.patch.object(
                        release.subprocess, "run", lambda argv, **_: ran.append(argv) or unittest.mock.Mock(returncode=0)
                    ):
                self.assertEqual(release.main(args), 0)
            return ran[0]

        manifest = tomllib.loads((release.REPO / "Cargo.toml").read_text())["workspace"]["package"]["version"]
        self.assertEqual(release.workspace_version(), manifest)
        self.assertIn(f"{release.RELEASE_VARIABLE}={manifest}", host(["build", "--release"]))
        self.assertNotIn(release.RELEASE_VARIABLE, " ".join(host(["build"])))
        self.assertNotIn(release.RELEASE_VARIABLE, " ".join(host(["suite"])))
        self.assertEqual(release.main(["build", "--release", "--inside"]), 2)

    def test_the_suite_is_one_step_and_the_command_line_takes_no_archive(self):
        seen = []
        with unittest.mock.patch.object(release, "check_inside"), \
                unittest.mock.patch.object(release, "step_suite", lambda: seen.append(1) or 0):
            self.assertEqual(release.main(["suite", "--inside"]), 0)
        self.assertEqual(seen, [1])
        for argv in (["suite", "--archived"], ["suite-archive"]):
            with self.assertRaises(SystemExit):
                release.main(argv)

    def test_a_run_step_runs_its_command_where_it_names_with_only_the_colours_passed(self):
        ran: list[list[str]] = []
        with tempfile.TemporaryDirectory() as tmp, \
                unittest.mock.patch.dict(release.os.environ, {"NO_COLOR": "1", "SECRET": "x"}), \
                unittest.mock.patch.object(release, "ensure_image", return_value="t"), \
                unittest.mock.patch.object(release, "prepare_state"), \
                unittest.mock.patch.object(release, "git_view", return_value=None), \
                unittest.mock.patch.object(release, "STATE", Path(tmp)), \
                unittest.mock.patch.object(
                    release.subprocess, "run", lambda argv, **_: ran.append(argv) or unittest.mock.Mock(returncode=0)
                ):  # fmt: skip
            self.assertEqual(release.main(["run", "--cwd=scripts", "--", "uv", "run", "x.py", "--inside"]), 0)
        (argv,) = ran
        self.assertEqual(
            argv[argv.index("t"):],
            ["t", "python3", "scripts/release.py", "run", "--cwd=scripts", "--inside",
             "--", "uv", "run", "x.py", "--inside"],
        )  # fmt: skip
        self.assertIn("-e NO_COLOR=1", " ".join(argv))
        self.assertNotIn("SECRET", " ".join(argv))
        seen = []
        with unittest.mock.patch.object(release, "check_inside"), \
                unittest.mock.patch.object(release, "step_run", lambda c, cwd: seen.append((c, cwd)) or 0):
            self.assertEqual(release.main(argv[argv.index("scripts/release.py") + 1 :]), 0)
        self.assertEqual(seen, [(["uv", "run", "x.py", "--inside"], "scripts")])
        self.assertEqual(release.main(["run", "--"]), 2)

    def test_a_target_dir_is_the_host_s(self):
        self.assertEqual(release.main(["build", "--target-dir", "/x", "--inside"]), 2)

    def test_help_runs(self):
        out = subprocess.run(
            ["python3", "release.py", "--help"],
            cwd=Path(__file__).parent, capture_output=True, text=True, check=True,
        ).stdout  # fmt: skip
        self.assertIn("floor", out)


if __name__ == "__main__":
    unittest.main()
