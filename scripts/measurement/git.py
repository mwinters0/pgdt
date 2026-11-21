"""What the harness asks git: a revision's commit, the commits touching a path, the working tree's
changed paths."""

from __future__ import annotations

import subprocess
from typing import Iterable

from measurement import config
from measurement import host


def _git(args: list[str]) -> str | None:
    """`git`, with a failure reported as `None` rather than raised.

    Every caller here is asking a question whose unanswerable case is already
    "not comment-only", so a missing blob, an unresolvable sha or a path that
    did not exist yet all take the same route as a file the scanner cannot
    read."""
    try:
        return subprocess.run(
            ["git", *args], cwd=config.REPO, capture_output=True, text=True, check=True
        ).stdout
    except (subprocess.CalledProcessError, OSError):
        return None


def commits_touching(paths: Iterable[str], since: str) -> dict[str, list[str]]:
    """The commits in `since..HEAD` that changed each path."""
    out: dict[str, list[str]] = {}
    for path in paths:
        log = host.run(
            ["git", "log", "--format=%H", f"{since}..HEAD", "--", path],
            cwd=config.REPO,
            capture=True,
        )
        out[path] = [line.strip() for line in log.splitlines() if line.strip()]
    return out


def dirty_paths() -> set[str]:
    status = host.run(["git", "status", "--porcelain"], cwd=config.REPO, capture=True)
    return {line[3:].strip() for line in status.splitlines() if line.strip()}


def is_ancestor(earlier: str, later: str) -> bool:
    proc = subprocess.run(
        ["git", "merge-base", "--is-ancestor", earlier, later],
        cwd=str(config.REPO),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return proc.returncode == 0


def changed_paths(since: str) -> list[str]:
    diff = host.run(["git", "diff", "--name-only", since], cwd=config.REPO, capture=True)
    status = host.run(["git", "status", "--porcelain"], cwd=config.REPO, capture=True)
    paths = [line.strip() for line in diff.splitlines() if line.strip()]
    paths += [line[3:].strip() for line in status.splitlines() if line.strip()]
    return sorted(set(paths))


def resolve_commit(rev: str) -> str | None:
    """The commit `rev` names, or `None` where it names none."""
    try:
        return host.run(
            ["git", "rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}"],
            cwd=config.REPO,
            capture=True,
        ).strip()
    except subprocess.CalledProcessError:
        return None
