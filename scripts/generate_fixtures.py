#!/usr/bin/env python3
"""Generate pg_dump fixture files across PostgreSQL versions and flag combos.

Backs docs/design/roadmap-phase1-mvp.md ("Testing & fixtures") and the
tested/untested matrix in docs/design/pg-dump-compatibility.md. Spins up a
throwaway, memory-limited Postgres container per version (never a host-run
process, per this repo's CPU-heavy-machine / glibc-arena caution), loads
fixture_schema.sql, runs pg_dump across a flag matrix, and writes the
output under fixtures/<major-version>/<flag-set>.sql.

Requires `docker` (aliased to `nerdctl` in this environment) runnable via
passwordless `sudo`.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
SCHEMA_FILE = SCRIPT_DIR / "fixture_schema.sql"
FIXTURES_DIR = REPO_ROOT / "fixtures"

DOCKER = ["sudo", "-n", "docker"]

# Routine compatibility-matrix subset (docs/design/roadmap-phase1-mvp.md):
# oldest supported, version matching the real koji sample, newest available.
# The full historical worktree sweep (v13.0, v13.23, v14.0, v15.0, v16.0,
# v17.0, v18.0, v18.6) is a separate, manual/occasional exercise -- these
# images are convenient stand-ins for "major version N", not an attempt to hit
# exact patch levels.
ROUTINE_VERSIONS = {
    "13": "postgres:13-alpine",
    "16": "postgres:16-alpine",
    "18": "postgres:18-alpine",
}

DB_NAME = "pgdq_fixture"
DB_USER = "postgres"

FLAG_SETS: dict[str, list[str]] = {
    "default": [],
    "data-only": ["--data-only"],
    "schema-only": ["--schema-only"],
    "no-owner": ["--no-owner", "--no-privileges"],
    "clean-if-exists": ["--clean", "--if-exists"],
    "inserts": ["--inserts"],
    "column-inserts": ["--column-inserts"],
}


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=True, **kwargs)


def container_name(version: str) -> str:
    return f"pgdq-fixture-{version}"


def start_container(version: str, image: str) -> str:
    name = container_name(version)
    subprocess.run(DOCKER + ["rm", "-f", name], capture_output=True)
    run(
        DOCKER
        + [
            "run",
            "-d",
            "--name",
            name,
            "--memory",
            "512m",
            "-e",
            "MALLOC_ARENA_MAX=2",
            "-e",
            "POSTGRES_HOST_AUTH_METHOD=trust",
            image,
        ],
        stdout=subprocess.DEVNULL,
    )
    return name


def wait_ready(name: str, timeout: float = 30.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = subprocess.run(
            DOCKER + ["exec", name, "pg_isready", "-U", DB_USER],
            capture_output=True,
        )
        if result.returncode == 0:
            return
        time.sleep(0.5)
    raise TimeoutError(f"postgres in container {name} did not become ready in {timeout}s")


def create_fixture_db(name: str, attempts: int = 10, delay: float = 1.0) -> None:
    # The official postgres image briefly starts a *temporary* instance to
    # run init scripts before restarting for real; pg_isready can succeed
    # against that transient instance. Retry the actual DDL-capable command
    # rather than trusting pg_isready alone.
    last_error: subprocess.CalledProcessError | None = None
    for _ in range(attempts):
        try:
            run(DOCKER + ["exec", name, "createdb", "-U", DB_USER, DB_NAME], capture_output=True)
            last_error = None
            break
        except subprocess.CalledProcessError as exc:
            last_error = exc
            time.sleep(delay)
    if last_error is not None:
        raise last_error
    schema_sql = SCHEMA_FILE.read_text()
    run(
        DOCKER
        + ["exec", "-i", name, "psql", "-U", DB_USER, "-d", DB_NAME, "-v", "ON_ERROR_STOP=1"],
        input=schema_sql,
        text=True,
        stdout=subprocess.DEVNULL,
    )


def dump_flag_set(name: str, version: str, flag_name: str, flags: list[str]) -> Path:
    out_dir = FIXTURES_DIR / version
    out_dir.mkdir(parents=True, exist_ok=True)
    out_path = out_dir / f"{flag_name}.sql"
    result = run(
        DOCKER + ["exec", name, "pg_dump", "-U", DB_USER, *flags, DB_NAME],
        stdout=subprocess.PIPE,
        text=True,
    )
    out_path.write_text(result.stdout)
    return out_path


def stop_container(name: str) -> None:
    subprocess.run(DOCKER + ["rm", "-f", name], capture_output=True)


def generate_for_version(version: str, image: str) -> None:
    print(f"== {version} ({image}) ==")
    name = start_container(version, image)
    try:
        wait_ready(name)
        create_fixture_db(name)
        for flag_name, flags in FLAG_SETS.items():
            out_path = dump_flag_set(name, version, flag_name, flags)
            size = out_path.stat().st_size
            print(f"  {flag_name}: {out_path.relative_to(REPO_ROOT)} ({size} bytes)")
    finally:
        stop_container(name)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--version",
        action="append",
        dest="versions",
        choices=sorted(ROUTINE_VERSIONS),
        help="limit to specific major version(s); default: all routine versions",
    )
    args = parser.parse_args()
    versions = args.versions or sorted(ROUTINE_VERSIONS)

    for version in versions:
        generate_for_version(version, ROUTINE_VERSIONS[version])

    print("done.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
