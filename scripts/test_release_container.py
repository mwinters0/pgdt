#!/usr/bin/env python3
"""Unit tests for release_container.py, run with `uv run python -m unittest test_release_container`.

The context staged from the archives, the two Dockerfiles' agreement, and each
step's commands and refusals, against a stand-in for the container runtime;
the publish workflow's jobs are `test_release_ci`'s `Workflows`.
"""

from __future__ import annotations

import contextlib
import hashlib
import io
import json
import re
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path
from typing import Any, Callable

import release
import release_ci
import release_container as rc
from release import ReleaseError

VERSION = "0.4.1"
SHA = "a" * 40
MTIME = 1_800_000_000
REPO_NAME = "Owner/pgdt"
FILES = ("pgdt", "LICENSE", release.NOTICES_NAME, "README.md")


def make_archives(dist: Path, binaries: dict[str, bytes] | None = None) -> dict[str, str]:
    """Both targets' archives in `dist`, each `pgdt` its target's bytes;
    returns each architecture's binary's SHA-256."""
    src = dist.parent / "src"
    digests = {}
    for target, arch in rc.ARCHES.items():
        d = src / target
        d.mkdir(parents=True)
        for name in FILES:
            (d / name).write_bytes((binaries or {}).get(target, target.encode()) if name == "pgdt" else name.encode())
        release_ci.write_archive(
            dist / release_ci.archive_name(VERSION, target), release_ci.archive_stem(VERSION, target),
            [release_ci.Member(n, d / n, 0o755 if n == "pgdt" else 0o644) for n in sorted(FILES)], 1,
        )  # fmt: skip
        digests[arch] = hashlib.sha256((d / "pgdt").read_bytes()).hexdigest()
    return digests


def index_of(arches: dict[str, str]) -> str:
    return json.dumps({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{"digest": d, "platform": {"os": "linux", "architecture": a}} for a, d in arches.items()],
    })  # fmt: skip


def digest_of(text: str) -> str:
    return "sha256:" + hashlib.sha256(text.encode()).hexdigest()


class FakeRuntime:
    """`subprocess.run` for the container runtime: records each argv and
    answers by the function given."""

    def __init__(self, answer: Callable[[list[str]], str]):
        self.ran: list[list[str]] = []
        self.inputs: list[Any] = []
        self.answer = answer

    def __call__(self, argv, **kwargs):
        self.ran.append(list(argv))
        self.inputs.append(kwargs.get("input"))
        return subprocess.CompletedProcess(argv, 0, stdout=self.answer(list(argv)), stderr="")


@contextlib.contextmanager
def quiet():
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        yield


@contextlib.contextmanager
def at_head(runtime: FakeRuntime, container: str = "docker"):
    with unittest.mock.patch.object(release, "workspace_version", return_value=VERSION), \
            unittest.mock.patch.object(release_ci, "head_commit", return_value=SHA), \
            unittest.mock.patch.object(release_ci, "commit_time", return_value=MTIME), \
            unittest.mock.patch.object(release_ci, "origin_repo", return_value=REPO_NAME), \
            unittest.mock.patch.object(release, "CONTAINER", container), \
            unittest.mock.patch.object(rc.subprocess, "run", runtime), quiet():
        yield


ENV = {"GITHUB_REPOSITORY": REPO_NAME, "GITHUB_ACTOR": "someone", "GH_TOKEN": "t0ken"}


class Names(unittest.TestCase):
    def test_the_name_is_lower_case_and_the_tags_are_the_version_its_minor_line_and_latest(self):
        self.assertEqual(rc.image_name(REPO_NAME), "ghcr.io/owner/pgdt")
        self.assertEqual(rc.release_tags("0.4.1"), ["0.4.1", "0.4", "latest"])
        self.assertEqual(rc.release_tags("1.12.0"), ["1.12.0", "1.12", "latest"])

    def test_every_release_target_has_an_architecture_and_one_is_attested(self):
        self.assertEqual(sorted(rc.ARCHES), sorted(release.TARGETS))
        self.assertIn(rc.ATTESTED_ARCH, rc.ARCHES.values())
        self.assertEqual(rc.ARCHES[release.NATIVE_TARGET], rc.ATTESTED_ARCH, "CI builds the attested one")
        self.assertEqual(rc.platforms(), "linux/amd64,linux/arm64")

    def test_the_builder_s_buildkit_is_pinned_by_digest(self):
        self.assertRegex(rc.BUILDKIT, r"^moby/buildkit:v\d+\.\d+\.\d+@sha256:[0-9a-f]{64}$")

    def test_a_host_names_its_architecture_or_is_refused(self):
        self.assertEqual(rc.host_arch("x86_64"), "amd64")
        self.assertEqual(rc.host_arch("aarch64"), "arm64")
        with self.assertRaises(ReleaseError):
            rc.host_arch("riscv64")


class Bases(unittest.TestCase):
    def test_the_committed_bases_share_a_codename_and_the_container_s_is_slim_and_pinned(self):
        self.assertEqual(rc.check_codenames(release.DOCKERFILE.read_text(), rc.DOCKERFILE.read_text()), "trixie")
        self.assertRegex(release.pinned_base(rc.DOCKERFILE.read_text()), r"^debian:[a-z]+-slim@sha256:")

    def test_two_codenames_are_refused(self):
        build = "FROM debian:trixie@sha256:" + "1" * 64 + "\n"
        container = "FROM debian:forky-slim@sha256:" + "2" * 64 + "\n"
        with self.assertRaisesRegex(ReleaseError, "forky.*trixie"):
            rc.check_codenames(build, container)

    def test_the_dockerfile_copies_what_the_context_stages_and_runs_nothing(self):
        text = rc.DOCKERFILE.read_text()
        self.assertNotRegex(text, r"(?m)^RUN\b", "nothing is built or emulated")
        self.assertIn(f"COPY ${{TARGETARCH}}/pgdt {rc.IMAGE_BINARY}", text)
        for arg in ("VERSION", "REVISION", "SOURCE"):
            self.assertRegex(text, rf"(?m)^ARG {arg}$")
        self.assertIn('ENTRYPOINT ["pgdt"]', text)
        self.assertIn("WORKDIR /data", text)


class Context(unittest.TestCase):
    def test_each_archive_is_staged_under_its_architecture_at_the_commit_s_time(self):
        with tempfile.TemporaryDirectory() as tmp:
            dist, ctx = Path(tmp) / "dist", Path(tmp) / "ctx"
            ctx.mkdir(parents=True)
            dist.mkdir()
            want = make_archives(dist)
            got = rc.stage_context(dist, VERSION, ctx, MTIME)
            self.assertEqual(got, want)
            self.assertEqual(sorted(p.name for p in ctx.iterdir()), ["Dockerfile", "amd64", "arm64"])
            for target, arch in rc.ARCHES.items():
                self.assertEqual(sorted(p.name for p in (ctx / arch).iterdir()), sorted(FILES))
                self.assertEqual((ctx / arch / "pgdt").read_bytes(), target.encode())
                self.assertTrue((ctx / arch / "pgdt").stat().st_mode & 0o100)
                for p in (ctx / arch, *(ctx / arch).iterdir()):
                    self.assertEqual(int(p.stat().st_mtime), MTIME, p)
            self.assertEqual((ctx / "Dockerfile").read_text(), rc.DOCKERFILE.read_text())

    def test_a_missing_archive_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            dist, ctx = Path(tmp) / "dist", Path(tmp) / "ctx"
            ctx.mkdir(parents=True)
            dist.mkdir()
            make_archives(dist)
            (dist / release_ci.archive_name(VERSION, release.CROSS_TARGET)).unlink()
            with self.assertRaisesRegex(ReleaseError, "both archives"):
                rc.stage_context(dist, VERSION, ctx, MTIME)

    def test_the_build_arguments_name_the_version_the_commit_and_the_repository(self):
        args = rc.build_args(VERSION, SHA, REPO_NAME, MTIME)
        self.assertEqual(args[1::2], [
            f"SOURCE_DATE_EPOCH={MTIME}", f"VERSION={VERSION}", f"REVISION={SHA}",
            f"SOURCE=https://github.com/{REPO_NAME}",
        ])  # fmt: skip
        self.assertEqual(set(args[0::2]), {"--build-arg"})


class Index(unittest.TestCase):
    ARCH_DIGESTS = {"amd64": "sha256:" + "a" * 64, "arm64": "sha256:" + "b" * 64}

    def test_an_index_of_the_release_s_platforms_gives_each_manifest(self):
        raw = index_of(self.ARCH_DIGESTS)
        self.assertEqual(rc.platform_digests(raw, digest_of(raw)), self.ARCH_DIGESTS)

    def test_an_index_read_back_as_other_bytes_is_refused(self):
        raw = index_of(self.ARCH_DIGESTS)
        with self.assertRaisesRegex(ReleaseError, "read back"):
            rc.platform_digests(raw, "sha256:" + "0" * 64)

    def test_an_index_of_other_platforms_or_more_entries_is_refused(self):
        for arches in ({"amd64": "sha256:" + "a" * 64}, {**self.ARCH_DIGESTS, "riscv64": "sha256:" + "c" * 64}):
            raw = index_of(arches)
            with self.assertRaises(ReleaseError):
                rc.platform_digests(raw, digest_of(raw))
        attested = json.loads(index_of(self.ARCH_DIGESTS))
        attested["manifests"].append({"digest": "sha256:" + "d" * 64, "platform": {"os": "unknown", "architecture": "unknown"}})
        raw = json.dumps(attested)
        with self.assertRaisesRegex(ReleaseError, "alone"):
            rc.platform_digests(raw, digest_of(raw))


class Push(unittest.TestCase):
    def test_push_logs_in_builds_both_platforms_untagged_and_outputs_the_digests(self):
        raw = index_of(Index.ARCH_DIGESTS)
        digest = digest_of(raw)

        def answer(argv: list[str]) -> str:
            if "--metadata-file" in argv:
                Path(argv[argv.index("--metadata-file") + 1]).write_text(json.dumps({"containerimage.digest": digest}))
            if argv[1:4] == ["buildx", "imagetools", "inspect"]:
                return raw
            return ""

        runtime = FakeRuntime(answer)
        with tempfile.TemporaryDirectory() as tmp:
            dist, outputs = Path(tmp) / "dist", Path(tmp) / "outputs"
            dist.mkdir()
            make_archives(dist)
            with at_head(runtime):
                self.assertEqual(rc.step_push({**ENV, "GITHUB_OUTPUT": str(outputs)}, dist=dist), 0)
            self.assertEqual(
                outputs.read_text().splitlines(),
                ["name=ghcr.io/owner/pgdt", f"digest={digest}", f"amd64_digest={Index.ARCH_DIGESTS['amd64']}"],
            )
            self.assertEqual(sorted(p.name for p in dist.iterdir()), sorted(
                release_ci.archive_name(VERSION, t) for t in release.TARGETS
            ), "the context is gone")  # fmt: skip
        login, create, build, inspect = runtime.ran
        self.assertEqual(login, ["docker", "login", "ghcr.io", "--username", "someone", "--password-stdin"])
        self.assertEqual(runtime.inputs[0], "t0ken", "the token goes by stdin, never argv")
        self.assertNotIn("t0ken", " ".join(a for argv in runtime.ran for a in argv))
        self.assertEqual(create[-2:], ["--driver-opt", f"image={rc.BUILDKIT}"])
        self.assertEqual(build[1:5], ["buildx", "build", "--builder", rc.BUILDER])
        self.assertIn("--provenance=false", build)
        output = build[build.index("--output") + 1]
        for attr in ("type=image", "name=ghcr.io/owner/pgdt", "push-by-digest=true", "push=true", "rewrite-timestamp=true"):
            self.assertIn(attr, output.split(","))
        self.assertIn(f"SOURCE_DATE_EPOCH={MTIME}", build)
        self.assertEqual(inspect[-1], f"ghcr.io/owner/pgdt@{digest}")

    def test_push_is_refused_but_by_docker(self):
        with at_head(FakeRuntime(lambda _: ""), container="sudo nerdctl"):
            with self.assertRaisesRegex(ReleaseError, "docker buildx"):
                rc.step_push(ENV)

    def test_push_without_a_token_is_refused_before_building(self):
        runtime = FakeRuntime(lambda _: "")
        with tempfile.TemporaryDirectory() as tmp, at_head(runtime):
            with self.assertRaisesRegex(ReleaseError, "GH_TOKEN"):
                rc.step_push({"GITHUB_REPOSITORY": REPO_NAME}, dist=Path(tmp))
        self.assertEqual(runtime.ran, [])


class Smoke(unittest.TestCase):
    def run_smoke(self, env: dict[str, str], image_binary: bytes | None = None) -> FakeRuntime:
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp) / "dist"
            dist.mkdir()
            digests = make_archives(dist)

            def answer(argv: list[str]) -> str:
                if "sha256sum" in argv:
                    got = hashlib.sha256(image_binary).hexdigest() if image_binary else digests["amd64"]
                    return f"{got}  {rc.IMAGE_BINARY}\n"
                if argv[1] == "login":
                    return ""
                image = next(i for i, a in enumerate(argv) if a.startswith(("pgdt-container:", "ghcr.io/")))
                return {
                    "--version": f"pgdt {VERSION} (allocator: mimalloc)\n",
                    "parse": f"{release_ci.SMOKE_PARSE_LINE}\n",
                    "query": "1\talpha\tx\n5\t\tempty name above\t\\N\n",
                }[argv[image + 1]]

            runtime = FakeRuntime(answer)
            with at_head(runtime), unittest.mock.patch.object(rc, "host_arch", return_value="amd64"):
                rc.step_smoke(env, dist=dist)
        return runtime

    def test_the_local_image_is_held_to_its_archive_and_runs_the_three_commands(self):
        runtime = self.run_smoke({})
        sha, *runs = runtime.ran
        self.assertEqual(sha[-3:], ["sha256sum", f"pgdt-container:{VERSION}", rc.IMAGE_BINARY])
        self.assertEqual([r[r.index(f"pgdt-container:{VERSION}") + 1] for r in runs], ["--version", "parse", "query"])
        for r in (sha, *runs):
            self.assertEqual(r[r.index("--platform") + 1], "linux/amd64")

    def test_a_pushed_digest_is_pulled_after_logging_in(self):
        digest = "sha256:" + "e" * 64
        runtime = self.run_smoke({**ENV, "CONTAINER_DIGEST": digest})
        self.assertEqual(runtime.ran[0][1], "login")
        self.assertIn(f"ghcr.io/owner/pgdt@{digest}", runtime.ran[1])

    def test_an_image_holding_other_bytes_is_refused_before_it_runs(self):
        with self.assertRaisesRegex(ReleaseError, "not its archive's"):
            self.run_smoke({}, image_binary=b"another build")

    def test_a_malformed_digest_is_refused(self):
        with self.assertRaisesRegex(ReleaseError, "CONTAINER_DIGEST"):
            self.run_smoke({**ENV, "CONTAINER_DIGEST": "latest"})


class Tag(unittest.TestCase):
    DIGEST = "sha256:" + "f" * 64

    def run_tag(self, held: Callable[[str], str]) -> FakeRuntime:
        def answer(argv: list[str]) -> str:
            if argv[1:4] == ["buildx", "imagetools", "inspect"]:
                return json.dumps({"digest": held(argv[4])})
            return ""

        runtime = FakeRuntime(answer)
        with at_head(runtime):
            rc.step_tag({**ENV, "CONTAINER_DIGEST": self.DIGEST})
        return runtime

    def test_the_digest_takes_every_tag_and_each_is_read_back(self):
        runtime = self.run_tag(lambda _ref: self.DIGEST)
        login, create, *inspects = runtime.ran
        self.assertEqual(login[1], "login")
        name = "ghcr.io/owner/pgdt"
        self.assertEqual(create[1:4], ["buildx", "imagetools", "create"])
        self.assertEqual(create[-1], f"{name}@{self.DIGEST}")
        tags = [create[i + 1] for i, a in enumerate(create) if a == "--tag"]
        self.assertEqual(tags, [f"{name}:{t}" for t in rc.release_tags(VERSION)])
        self.assertEqual([i[4] for i in inspects], tags)

    def test_a_tag_naming_another_index_is_refused(self):
        with self.assertRaisesRegex(ReleaseError, "latest names"):
            self.run_tag(lambda ref: "sha256:" + "0" * 64 if ref.endswith(":latest") else self.DIGEST)

    def test_no_digest_is_refused(self):
        with at_head(FakeRuntime(lambda _: "")):
            with self.assertRaisesRegex(ReleaseError, "names no pushed index"):
                rc.step_tag(ENV)


class CommandLine(unittest.TestCase):
    def test_help_runs(self):
        out = subprocess.run(
            ["python3", "release_container.py", "--help"],
            cwd=Path(__file__).parent, capture_output=True, text=True, check=True,
        ).stdout  # fmt: skip
        for step in ("build", "push", "smoke", "tag"):
            self.assertRegex(out, rf"\b{step}\b")

    def test_a_refusal_exits_2(self):
        with quiet():
            self.assertEqual(rc.main(["tag"], env={}), 2)


if __name__ == "__main__":
    unittest.main()
