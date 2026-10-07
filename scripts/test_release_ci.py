#!/usr/bin/env python3
"""Unit tests for release_ci.py, run with `uv run python -m unittest test_release_ci`.

The archive, the checksums, the smoke run's checks and the publication's
refusals are held here against stand-ins for `gh` and the container runtime,
and `Workflows` holds the two committed workflow files to what
`docs/design/roadmap-P29-releases.md` commits them to: no step but a
`release.py` or `release_ci.py` line the parsers accept, no input interpolated
into a shell, every action pinned, every permission earned.
"""

from __future__ import annotations

import contextlib
import hashlib
import io
import re
import shlex
import subprocess
import tarfile
import tempfile
import unittest
import unittest.mock
from pathlib import Path
from typing import Any

import check
import release
import release_ci
from release import ReleaseError

SHA = "a" * 40
OTHER = "b" * 40
VERSION = "0.1.0"
TARGETS = sorted(release.TARGETS)


def release_row(tag: str = "v0.1.0", *, draft: bool = True, commit: str = SHA, assets=None, id_: int = 7) -> dict:
    """A release as the API lists it: by default a complete draft of `SHA`."""
    names = release_ci.expected_assets(tag[1:]) if assets is None else assets
    return {
        "id": id_, "tag_name": tag, "draft": draft, "target_commitish": commit,
        "assets": [{"name": n, "state": "uploaded"} for n in names],
    }  # fmt: skip


def annotated(message: str = "pgdt 0.1.0\n\nThe first release.\n", commit: str = SHA) -> release_ci.Tag:
    return release_ci.Tag("v0.1.0", True, commit, message)


class FakeGh:
    """`release_ci.Gh`'s calls answered from a table, and recorded."""

    def __init__(self, answers: dict[str, Any], releases: list[dict] | None = None):
        self.answers, self._releases, self.calls = answers, releases or [], []

    def api(self, path: str, **kwargs: Any) -> Any:
        self.calls.append((path, kwargs))
        return self.answers[path]

    def releases(self) -> list[dict]:
        return self._releases

    def create_draft(self, tag: str, sha: str, files) -> None:
        self.calls.append(("create_draft", {"tag": tag, "sha": sha, "files": [f.name for f in files]}))


class Names(unittest.TestCase):
    def test_an_archive_is_named_for_its_version_and_target(self):
        self.assertEqual(
            release_ci.archive_name("0.2.0", "aarch64-unknown-linux-gnu"),
            "pgdt-v0.2.0-aarch64-unknown-linux-gnu.tar.xz",
        )
        self.assertEqual(release_ci.archive_stem("0.2.0", "t"), "pgdt-v0.2.0-t")

    def test_a_release_holds_an_archive_a_target_and_the_checksums(self):
        assets = release_ci.expected_assets(VERSION)
        self.assertEqual(len(assets), len(release.TARGETS) + 1)
        self.assertEqual(assets[-1], "SHA256SUMS")
        for target in release.TARGETS:
            self.assertIn(f"pgdt-v{VERSION}-{target}.tar.xz", assets)

    def test_a_ref_must_be_the_checked_out_full_commit(self):
        self.assertEqual(release_ci.check_ref(SHA, SHA), SHA)
        for bad in (None, "", "main", SHA[:7], SHA.upper(), SHA + "0"):
            with self.assertRaisesRegex(ReleaseError, "full 40-hex"):
                release_ci.check_ref(bad, SHA)
        with self.assertRaisesRegex(ReleaseError, "not the requested"):
            release_ci.check_ref(OTHER, SHA)


class Archives(unittest.TestCase):
    def files(self, tmp: Path, binary: bytes = b"\x7fELF") -> list[release_ci.Member]:
        for name, data in (("bin", binary), ("notices", b"n"), ("LICENSE", b"l"), ("README.md", b"r")):
            (tmp / name).write_bytes(data)
        (tmp / "root").mkdir(exist_ok=True)
        (tmp / "root" / "LICENSE").write_bytes(b"l")
        (tmp / "root" / "README.md").write_bytes(b"r")
        return release_ci.members(tmp / "bin", tmp / "notices", tmp / "root")

    def test_an_archive_is_one_directory_of_four_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            dest = tmp / "out" / "a.tar.xz"
            release_ci.write_archive(dest, "pgdt-v0.1.0-t", self.files(tmp), 1_700_000_000)
            with tarfile.open(dest) as tar:
                release_ci.check_layout(tar.getnames(), "pgdt-v0.1.0-t")
                modes = {m.name: m.mode for m in tar.getmembers()}
                self.assertEqual(modes["pgdt-v0.1.0-t/pgdt"], 0o755)
                self.assertEqual(modes["pgdt-v0.1.0-t/LICENSE"], 0o644)
                for m in tar.getmembers():
                    self.assertEqual((m.uid, m.gid, m.mtime), (0, 0, 1_700_000_000), m.name)
                self.assertEqual(tar.extractfile("pgdt-v0.1.0-t/pgdt").read(), b"\x7fELF")

    def test_the_same_inputs_give_the_same_bytes(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            out = []
            for tmp in (Path(a), Path(b)):
                files = self.files(tmp)
                for f in files:  # file times and a second write must not show
                    f.source.touch()
                dest = tmp / "a.tar.xz"
                release_ci.write_archive(dest, "s", files, 5)
                out.append(dest.read_bytes())
            self.assertEqual(out[0], out[1])

    def test_a_missing_piece_is_refused_by_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            self.files(tmp)
            (tmp / "notices").unlink()
            with self.assertRaisesRegex(ReleaseError, "THIRD-PARTY-NOTICES is part of every archive"):
                release_ci.members(tmp / "bin", tmp / "notices", tmp / "root")

    def test_a_layout_with_a_stray_or_missing_member_is_refused(self):
        stem = "s"
        good = [stem, f"{stem}/pgdt", f"{stem}/LICENSE", f"{stem}/THIRD-PARTY-NOTICES", f"{stem}/README.md"]
        release_ci.check_layout(good, stem)
        with self.assertRaisesRegex(ReleaseError, "extra"):
            release_ci.check_layout([*good, "s/other"], stem)
        with self.assertRaisesRegex(ReleaseError, "missing"):
            release_ci.check_layout(good[:-1], stem)
        with self.assertRaisesRegex(ReleaseError, "extra"):
            release_ci.check_layout([n.replace("s", "t", 1) for n in good], stem)

    def test_the_archive_step_reads_what_the_build_and_the_notices_wrote(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            target_dir, dist = tmp / "target", tmp / "dist"
            for target in release.TARGETS:
                out = target_dir / target / "release"
                out.mkdir(parents=True)
                (out / "pgdt").write_bytes(target.encode())
                (out / release.NOTICES_NAME).write_text("notices")
            with unittest.mock.patch.object(release_ci, "host_target_dir", return_value=target_dir), \
                    unittest.mock.patch.object(release_ci, "DIST", dist), \
                    unittest.mock.patch.object(release_ci, "commit_time", return_value=1), \
                    unittest.mock.patch.object(release.subprocess, "run"), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(release_ci.step_archive(TARGETS), 0)
            self.assertEqual(
                sorted(p.name for p in dist.iterdir()),
                sorted(release_ci.archive_name(release.workspace_version(), t) for t in release.TARGETS),
            )


class Checksums(unittest.TestCase):
    def archives(self, dist: Path, version: str = VERSION, *, skip: str | None = None) -> None:
        for t in release.TARGETS:
            if t != skip:
                (dist / release_ci.archive_name(version, t)).write_bytes(t.encode())

    def test_the_file_is_sha256sums_own_format_in_name_order(self):
        text = release_ci.checksum_lines({"b": "2" * 64, "a": "1" * 64})
        self.assertEqual(text, f"{'1' * 64}  a\n{'2' * 64}  b\n")

    def test_every_archive_is_summed_and_nothing_else(self):
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp)
            self.archives(dist)
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(release_ci.step_checksums(VERSION, dist), 0)
            lines = (dist / "SHA256SUMS").read_text().splitlines()
            self.assertEqual(len(lines), len(release.TARGETS))
            for line in lines:
                digest, name = line.split("  ")
                self.assertEqual(digest, hashlib.sha256((dist / name).read_bytes()).hexdigest())

    def test_a_missing_or_stray_archive_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp)
            self.archives(dist, skip=release.NATIVE_TARGET)
            with self.assertRaisesRegex(ReleaseError, "not the release's archives"):
                release_ci.step_checksums(VERSION, dist)
            self.archives(dist)
            (dist / "pgdt-v0.0.9-x.tar.xz").write_bytes(b"old")
            with self.assertRaisesRegex(ReleaseError, "not the release's archives"):
                release_ci.step_checksums(VERSION, dist)


class Smoke(unittest.TestCase):
    def test_a_release_names_its_version_and_is_not_unreleased(self):
        ok = "pgdt 0.1.0 (allocator: mimalloc) (datafusion: 55.1.0)\n"
        release_ci.check_version_line(ok, "0.1.0")
        for bad, why in (
            ("pgdt 0.1.0 (unreleased) (allocator: mimalloc)\n", "unreleased"),
            ("pgdt 0.2.0 (allocator: mimalloc)\n", "not pgdt 0.1.0"),
            ("", "not pgdt 0.1.0"),
            ("pgdt 0.1.01 (x)\n", "not pgdt 0.1.0"),
        ):
            with self.assertRaisesRegex(ReleaseError, why):
                release_ci.check_version_line(bad, "0.1.0")

    def test_an_output_missing_what_it_must_say_is_refused_naming_it(self):
        rows = "id\tname\n1\talpha\tx\n5\t\tempty name above\t\\N\n"
        release_ci.check_output_has("query", rows, release_ci.SMOKE_QUERY_ROWS)
        with self.assertRaisesRegex(ReleaseError, "empty name above"):
            release_ci.check_output_has("query", "1\talpha\tx\n", release_ci.SMOKE_QUERY_ROWS)

    def test_the_run_is_a_fresh_image_as_the_user_with_the_binary_read_only(self):
        argv = release_ci.smoke_argv(
            ["docker"], "debian:trixie@sha256:x", 1000, 100, Path("/t/stem"), Path("/t/data"), ["parse"]
        )
        joined = " ".join(argv)
        self.assertEqual(argv[:3], ["docker", "run", "--rm"])
        self.assertIn("--user 1000:100", joined)
        self.assertIn("-v /t/stem:/pgdt:ro", joined)
        self.assertIn("-v /t/data:/data", joined)
        self.assertEqual(argv[argv.index("debian:trixie@sha256:x") :], ["debian:trixie@sha256:x", "/pgdt/pgdt", "parse"])
        self.assertNotIn("--privileged", argv)

    def test_the_fixture_still_says_what_the_smoke_run_expects(self):
        text = (release.REPO / release_ci.SMOKE_FIXTURE).read_text()
        copy = re.search(
            rf"^COPY {re.escape(release_ci.SMOKE_TABLE)} \(.*\) FROM stdin;\n(.*?)^\\\.$", text, re.M | re.S
        )
        self.assertIsNotNone(copy, "the smoke table's COPY block")
        rows = copy.group(1).splitlines()
        self.assertEqual(release_ci.SMOKE_PARSE_LINE, f"{release_ci.SMOKE_TABLE} ({len(rows)} rows)")
        first, last = release_ci.SMOKE_QUERY_ROWS
        self.assertTrue(rows[0].startswith(first), "the scan's first row")
        self.assertTrue(rows[-1].startswith(last), "the scan's last row")

    def test_a_smoke_run_extracts_the_archive_and_runs_three_commands_in_the_base_image(self):
        version = release.workspace_version()
        target = release.NATIVE_TARGET
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            dist = tmp / "dist"
            stem = release_ci.archive_stem(version, target)
            src = tmp / "src"
            src.mkdir()
            for name in ("pgdt", "LICENSE", release.NOTICES_NAME, "README.md"):
                (src / name).write_text(name)
            release_ci.write_archive(
                dist / release_ci.archive_name(version, target), stem,
                [release_ci.Member(n, src / n, 0o755 if n == "pgdt" else 0o644)
                 for n in sorted(("pgdt", "LICENSE", release.NOTICES_NAME, "README.md"))],
                1,
            )  # fmt: skip
            ran: list[list[str]] = []

            def fake(argv, **_kwargs):
                ran.append(list(argv))
                out = {
                    "--version": f"pgdt {version} (allocator: mimalloc)\n",
                    "parse": f"{release_ci.SMOKE_PARSE_LINE}\n",
                    "query": "id\tname\n1\talpha\tx\n5\t\tempty name above\t\\N\n",
                }[argv[argv.index("/pgdt/pgdt") + 1]]
                return subprocess.CompletedProcess(argv, 0, stdout=out, stderr="")

            with unittest.mock.patch.object(release_ci.subprocess, "run", fake), \
                    unittest.mock.patch.object(release, "CONTAINER", "docker"), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(release_ci.step_smoke(target, dist), 0)
            self.assertEqual([a[a.index("/pgdt/pgdt") + 1] for a in ran], ["--version", "parse", "query"])
            base = release.pinned_base(release.DOCKERFILE.read_text())
            for argv in ran:
                self.assertIn(base, argv)

            def failing(argv, **_kwargs):
                return subprocess.CompletedProcess(argv, 3, stdout="", stderr="boom")

            with unittest.mock.patch.object(release_ci.subprocess, "run", failing), \
                    unittest.mock.patch.object(release, "CONTAINER", "docker"), \
                    contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaisesRegex(ReleaseError, "exited 3"):
                    release_ci.step_smoke(target, dist)


class Draft(unittest.TestCase):
    def test_a_version_with_any_release_is_refused(self):
        self.assertEqual(release_ci.plan_draft("0.1.0", [release_row("v0.0.9", draft=False)]), "v0.1.0")
        with self.assertRaisesRegex(ReleaseError, "a draft"):
            release_ci.plan_draft("0.1.0", [release_row()])
        with self.assertRaisesRegex(ReleaseError, "published"):
            release_ci.plan_draft("0.1.0", [release_row(draft=False)])

    def test_the_draft_holds_the_releases_assets_and_names_its_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp)
            with self.assertRaisesRegex(ReleaseError, "SHA256SUMS"):
                release_ci.draft_files(VERSION, dist)
            for name in release_ci.expected_assets(VERSION):
                (dist / name).write_bytes(b"x")
            gh = FakeGh({}, releases=[])
            with unittest.mock.patch.object(release, "workspace_version", return_value=VERSION), \
                    unittest.mock.patch.object(release_ci, "head_commit", return_value=SHA), \
                    contextlib.redirect_stdout(io.StringIO()):
                release_ci.step_draft(gh, SHA, dist)
            (call, args), = gh.calls
            self.assertEqual((call, args["tag"], args["sha"]), ("create_draft", "v0.1.0", SHA))
            self.assertEqual(sorted(args["files"]), sorted(release_ci.expected_assets(VERSION)))

    def test_a_draft_of_another_checkout_or_an_existing_version_creates_nothing(self):
        gh = FakeGh({}, releases=[release_row()])
        with unittest.mock.patch.object(release, "workspace_version", return_value=VERSION), \
                unittest.mock.patch.object(release_ci, "head_commit", return_value=SHA):
            with self.assertRaisesRegex(ReleaseError, "not the requested"):
                release_ci.step_draft(gh, OTHER)
            with self.assertRaisesRegex(ReleaseError, "already has a Release"):
                release_ci.step_draft(gh, SHA)
        self.assertEqual(gh.calls, [])

    def test_pages_of_releases_are_one_list(self):
        self.assertEqual(release_ci.flatten_pages([[{"a": 1}], [{"b": 2}], []]), [{"a": 1}, {"b": 2}])
        self.assertEqual(release_ci.flatten_pages([{"a": 1}]), [{"a": 1}])
        self.assertEqual(release_ci.flatten_pages([]), [])
        with self.assertRaises(ReleaseError):
            release_ci.flatten_pages({"message": "Not Found"})


class Publish(unittest.TestCase):
    def plan(self, tag=None, releases=None, name="v0.1.0"):
        return release_ci.plan_publish(name, tag or annotated(), [release_row()] if releases is None else releases)

    def test_a_matching_annotated_tag_publishes_its_draft_with_its_message(self):
        plan = self.plan()
        self.assertEqual(plan, release_ci.Publication(7, "pgdt 0.1.0\n\nThe first release."))

    def test_each_mismatch_between_tag_and_draft_is_refused_before_anything_is_changed(self):
        cases = {
            "not a release tag": dict(name="v0.1"),
            "lightweight": dict(tag=release_ci.Tag("v0.1.0", False, SHA, "")),
            "annotation is empty": dict(tag=annotated(" \n")),
            "no draft Release is named": dict(releases=[release_row("v0.2.0")]),
            "Releases are named": dict(releases=[release_row(), release_row(id_=8)]),
            "already published": dict(releases=[release_row(draft=False)]),
            "was built from": dict(releases=[release_row(commit=OTHER)]),
            "holds": dict(releases=[release_row(assets=release_ci.expected_assets(VERSION)[:-1])]),
            "not all uploaded": dict(
                releases=[{**release_row(), "assets": [
                    {"name": n, "state": "starter"} for n in release_ci.expected_assets(VERSION)
                ]}]
            ),
        }  # fmt: skip
        for why, kwargs in cases.items():
            with self.subTest(why), self.assertRaisesRegex(ReleaseError, why):
                self.plan(**kwargs)

    def test_a_draft_built_from_a_branch_name_is_not_a_commit(self):
        with self.assertRaisesRegex(ReleaseError, "was built from 'main'"):
            self.plan(releases=[release_row(commit="main")])

    def test_a_tag_naming_another_version_finds_no_draft(self):
        with self.assertRaisesRegex(ReleaseError, "no draft Release is named v0.1.1"):
            self.plan(name="v0.1.1", tag=release_ci.Tag("v0.1.1", True, SHA, "text"))

    def test_the_draft_with_extra_assets_is_refused(self):
        names = [*release_ci.expected_assets(VERSION), "pgdt-evil.tar.xz"]
        with self.assertRaisesRegex(ReleaseError, "pgdt-evil"):
            self.plan(releases=[release_row(assets=names)])

    def test_a_signature_block_is_not_release_text(self):
        signed = "Release text\n-----BEGIN PGP SIGNATURE-----\n\nabc\n-----END PGP SIGNATURE-----\n"
        self.assertEqual(release_ci.annotation_body(signed), "Release text")
        ssh = "Text\n-----BEGIN SSH SIGNATURE-----\nabc\n-----END SSH SIGNATURE-----"
        self.assertEqual(release_ci.annotation_body(ssh), "Text")
        self.assertEqual(release_ci.annotation_body("A\n\nB\n"), "A\n\nB")

    def test_the_tag_is_read_through_its_ref_and_its_object(self):
        gh = FakeGh({
            "git/ref/tags/v0.1.0": {"object": {"type": "tag", "sha": "t" * 40}},
            f"git/tags/{'t' * 40}": {"message": "text\n", "object": {"type": "commit", "sha": SHA}},
            "git/ref/tags/v0.0.1": {"object": {"type": "commit", "sha": OTHER}},
            "git/ref/tags/vtree": {"object": {"type": "tree", "sha": OTHER}},
        })  # fmt: skip
        self.assertEqual(release_ci.fetch_tag(gh, "v0.1.0"), release_ci.Tag("v0.1.0", True, SHA, "text\n"))
        self.assertEqual(release_ci.fetch_tag(gh, "v0.0.1"), release_ci.Tag("v0.0.1", False, OTHER, ""))
        with self.assertRaisesRegex(ReleaseError, "not a commit or an annotated tag"):
            release_ci.fetch_tag(gh, "vtree")

    def test_a_tag_of_a_tag_is_refused(self):
        gh = FakeGh({
            "git/ref/tags/v0.1.0": {"object": {"type": "tag", "sha": "t" * 40}},
            f"git/tags/{'t' * 40}": {"message": "m", "object": {"type": "tag", "sha": OTHER}},
        })  # fmt: skip
        with self.assertRaisesRegex(ReleaseError, "not of a commit"):
            release_ci.fetch_tag(gh, "v0.1.0")

    def test_publishing_patches_the_draft_once_with_the_annotation(self):
        gh = FakeGh(
            {
                "git/ref/tags/v0.1.0": {"object": {"type": "tag", "sha": "t" * 40}},
                f"git/tags/{'t' * 40}": {"message": "text\n", "object": {"type": "commit", "sha": SHA}},
                "releases/7": None,
            },
            releases=[release_row()],
        )
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(release_ci.step_publish(gh, "v0.1.0"), 0)
        patches = [(p, k) for p, k in gh.calls if k.get("method") == "PATCH"]
        self.assertEqual(patches, [("releases/7", {"method": "PATCH", "payload": {"draft": False, "body": "text"}})])

    def test_a_refused_publication_changes_nothing(self):
        gh = FakeGh(
            {
                "git/ref/tags/v0.1.0": {"object": {"type": "tag", "sha": "t" * 40}},
                f"git/tags/{'t' * 40}": {"message": "text", "object": {"type": "commit", "sha": OTHER}},
            },
            releases=[release_row()],
        )
        with self.assertRaisesRegex(ReleaseError, "was built from"):
            release_ci.step_publish(gh, "v0.1.0")
        self.assertFalse([1 for _, k in gh.calls if k.get("method") == "PATCH"])
        with self.assertRaisesRegex(ReleaseError, "no tag"):
            release_ci.step_publish(gh, None)


class CommandLine(unittest.TestCase):
    def test_the_environment_names_the_repository(self):
        self.assertEqual(release_ci.repo_from_env({"GITHUB_REPOSITORY": "mwinters0/pgdt"}), "mwinters0/pgdt")
        for bad in ({}, {"GITHUB_REPOSITORY": "x"}, {"GITHUB_REPOSITORY": "a/b/c"}, {"GITHUB_REPOSITORY": "a b/c"}):
            with self.assertRaises(ReleaseError):
                release_ci.repo_from_env(bad)

    def test_a_command_needing_the_api_refuses_without_a_repository(self):
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(release_ci.main(["publish"], env={}), 2)

    def test_help_runs(self):
        out = subprocess.run(
            ["python3", "release_ci.py", "--help"],
            cwd=Path(__file__).parent, capture_output=True, text=True, check=True,
        ).stdout  # fmt: skip
        self.assertIn("publish", out)


# --- the workflows ---------------------------------------------------------

WORKFLOWS = release.REPO / ".github" / "workflows"
USES_RE = re.compile(r"^\s*-?\s*uses:\s*(\S+)\s*(?:#\s*(\S+))?\s*$")
RUN_RE = re.compile(r"^\s*-\s*run:\s*(.+?)\s*$")
JOB_RE = re.compile(r"^  ([a-z0-9-]+):\s*$")
NEEDS_RE = re.compile(r"^    needs:\s*(.+?)\s*$")


def lines_of(name: str) -> list[str]:
    return (WORKFLOWS / name).read_text().splitlines()


def jobs_of(name: str) -> dict[str, list[str]]:
    """Each job's lines, by the job's name."""
    jobs: dict[str, list[str]] = {}
    current = None
    inside = False
    for line in lines_of(name):
        if line == "jobs:":
            inside = True
        elif inside and (m := JOB_RE.match(line)):
            current = jobs.setdefault(m.group(1), [])
        elif inside and current is not None:
            current.append(line)
    return jobs


def commands_of(name: str) -> list[tuple[str, list[str]]]:
    """Every `run:` line as (script, its arguments)."""
    out = []
    for line in lines_of(name):
        if m := RUN_RE.match(line):
            argv = shlex.split(m.group(1))
            out.append((argv[1], argv[2:]))
    return out


class Workflows(unittest.TestCase):
    names = ("release-build.yml", "release-publish.yml")

    def test_only_the_two_workflows_exist(self):
        self.assertEqual(sorted(p.name for p in WORKFLOWS.iterdir()), sorted(self.names))

    def test_every_step_is_a_checked_in_script_line_the_parsers_accept(self):
        parsers = {"scripts/release.py": release.build_parser(), "scripts/release_ci.py": release_ci.build_parser()}
        for name in self.names:
            runs = [m.group(1) for line in lines_of(name) if (m := RUN_RE.match(line))]
            self.assertTrue(runs, name)
            for run in runs:
                argv = shlex.split(run)
                self.assertEqual(argv[0], "python3", run)
                self.assertIn(argv[1], parsers, run)
                with contextlib.redirect_stderr(io.StringIO()):
                    parsers[argv[1]].parse_args(argv[2:])  # exits on an unknown step or flag

    def test_no_expression_reaches_a_shell(self):
        for name in self.names:
            for line in lines_of(name):
                if RUN_RE.match(line):
                    self.assertNotIn("${{", line, f"{name}: {line}")

    def test_every_action_is_pinned_by_commit(self):
        for name in self.names:
            seen = 0
            for line in lines_of(name):
                if m := USES_RE.match(line):
                    seen += 1
                    self.assertRegex(m.group(1), r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$", line)
                    self.assertRegex(m.group(2) or "", r"^v\d", f"{line}: name the release it is")
            self.assertTrue(seen, name)

    def test_the_build_is_dispatched_by_hand_only_and_the_publish_by_a_version_tag_only(self):
        build, publish = "\n".join(lines_of("release-build.yml")), "\n".join(lines_of("release-publish.yml"))
        self.assertRegex(build, r"(?m)^on:\n  workflow_dispatch:\n")
        self.assertNotRegex(build, r"(?m)^  (push|pull_request|schedule|release)")
        self.assertRegex(publish, r'(?m)^on:\n  push:\n    tags:\n      - "v\*"\n\n')
        self.assertNotIn("workflow_dispatch", publish)

    def test_nothing_is_granted_by_default_and_each_job_declares_what_it_uses(self):
        for name in self.names:
            self.assertIn("\npermissions: {}\n", "\n".join(lines_of(name)), name)
            for job, body in jobs_of(name).items():
                text = "\n".join(body)
                self.assertRegex(text, r"(?m)^    permissions:\n(      [a-z-]+: (read|write)\n?)+", f"{name} {job}")
        build = jobs_of("release-build.yml")
        for job, body in build.items():
            text = "\n".join(body)
            writes = "contents: write" in text
            self.assertEqual(writes, job in ("preflight", "draft"), job)
            self.assertEqual("attestations: write" in text or "id-token: write" in text, job == "draft", job)
        self.assertIn("contents: write", "\n".join(jobs_of("release-publish.yml")["publish"]))

    def test_the_build_workflow_uses_the_image_runtime_and_the_ref_through_the_environment(self):
        text = "\n".join(lines_of("release-build.yml"))
        self.assertRegex(text, r"(?m)^  PGDT_RELEASE_CONTAINER: docker$")
        self.assertRegex(text, r"(?m)^  INPUT_REF: \$\{\{ inputs\.ref \}\}$")
        for job, body in jobs_of("release-build.yml").items():
            checkout = "\n".join(body)
            self.assertIn("ref: ${{ inputs.ref }}", checkout, job)
            self.assertIn("persist-credentials: false", checkout, job)

    def test_the_draft_waits_for_every_job_that_proves_something(self):
        jobs = jobs_of("release-build.yml")

        def needs(job: str) -> set[str]:
            for line in jobs[job]:
                if m := NEEDS_RE.match(line):
                    return {n.strip() for n in m.group(1).strip("[]").split(",")}
            return set()

        for job in jobs:
            if job not in ("preflight", "draft"):
                self.assertTrue(needs(job), job)
        reached, todo = set(), ["draft"]
        while todo:
            for n in needs(todo.pop()):
                if n not in reached:
                    reached.add(n)
                    todo.append(n)
        self.assertEqual(reached, set(jobs) - {"draft"})

    def test_each_target_is_released_built_noticed_archived_smoked_and_tested(self):
        runs = commands_of("release-build.yml")
        for target in release.TARGETS:
            flag = f"--target={target}"
            self.assertIn(("scripts/release.py", ["build", "--release", flag]), runs, target)
            self.assertIn(("scripts/release.py", ["notices", flag]), runs, target)
            self.assertIn(("scripts/release_ci.py", ["archive", flag]), runs, target)
            self.assertIn(("scripts/release_ci.py", ["smoke", flag]), runs, target)
        self.assertIn(("scripts/release.py", ["suite"]), runs)
        self.assertIn(("scripts/release.py", ["suite-archive"]), runs)
        self.assertIn(("scripts/release.py", ["suite", "--archived"]), runs)
        # Only a release's build is `--release`, and only the release builds.
        builds = [a for s, a in runs if s == "scripts/release.py" and a[0] == "build"]
        self.assertEqual(len(builds), len(release.TARGETS))
        self.assertTrue(all("--release" in a for a in builds))

    def test_the_arm64_suite_runs_on_an_arm64_runner_and_the_rest_on_x86_64(self):
        for job, body in jobs_of("release-build.yml").items():
            runner = re.search(r"(?m)^    runs-on: (\S+)$", "\n".join(body)).group(1)
            self.assertEqual(runner, "ubuntu-24.04-arm" if job == "arm64" else "ubuntu-24.04", job)
        arm = "\n".join(jobs_of("release-build.yml")["arm64"])
        self.assertIn("suite --archived", arm)
        self.assertIn(f"smoke --target={release.CROSS_TARGET}", arm)

    def test_no_workspace_library_holds_a_doctest_the_arm64_suite_would_skip(self):
        """The arm64 suite is nextest alone, a doctest being compiled where it
        runs (`docs/design/roadmap-P29-releases.md`, "The build image"), which
        loses nothing while no library holds one. The first one decides
        whether arm64 runs it, here rather than found later."""
        docs = check.doctest_packages(release.REPO, check.workspace(release.REPO))
        self.assertEqual(
            docs, set(),
            f"{', '.join(sorted(docs))} now holds a doctest, which the x86-64 suite runs and the "
            "arm64 suite does not: it is nextest alone, a doctest being compiled where it runs "
            "(docs/design/roadmap-P29-releases.md, \"The build image\"). Decide whether the "
            "arm64 job runs the doctests, then amend that decision and this test.",
        )  # fmt: skip

    def test_the_artifacts_the_jobs_pass_on_are_the_paths_the_scripts_use(self):
        text = "\n".join(lines_of("release-build.yml"))
        nextest = release.suite_archive_path(release.STATE / "target").relative_to(release.REPO)
        self.assertIn(f"path: {nextest}", text)
        self.assertIn(f"path: {release_ci.DIST.relative_to(release.REPO)}/*.tar.xz", text)
        self.assertEqual(
            release.suite_archive_path(Path("/state/target")),
            Path(f"/state/target/nextest/{release.CROSS_TARGET}.tar.zst"),
        )

    def test_the_attestation_covers_the_archives_after_the_checksums_are_made(self):
        draft = "\n".join(jobs_of("release-build.yml")["draft"])
        self.assertIn("subject-path: dist/*.tar.xz", draft)
        order = [draft.index(s) for s in ("release_ci.py checksums", "actions/attest@", "release_ci.py draft")]
        self.assertEqual(order, sorted(order))

    def test_dist_is_ignored(self):
        self.assertIn("/dist", (release.REPO / ".gitignore").read_text().splitlines())


if __name__ == "__main__":
    unittest.main()
