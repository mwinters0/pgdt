#!/usr/bin/env python3
"""Generate pg_dump fixture files across PostgreSQL versions and flag combos.

Backs docs/design/roadmap-phase1-mvp.md ("Testing & fixtures"), the
tested/untested matrix in docs/design/pg-dump-compatibility.md, and
docs/design/roadmap-phase2-typed-columns.md ("Fixtures"). Spins up a
throwaway, memory-limited Postgres container per version (never a host-run
process, per this repo's CPU-heavy-machine / glibc-arena caution), loads one
of two fixture schemas, runs pg_dump across each schema's own flag matrix, and
writes the output under fixtures/<major-version>/<schema>/<flag-set>.sql.

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
FIXTURES_DIR = REPO_ROOT / "fixtures"

DOCKER = ["sudo", "-n", "docker"]

# Routine version set: the latest minor release of every PostgreSQL major
# from 13 onward (13 being the oldest still-supported major) -- see
# docs/design/roadmap-phase1-mvp.md ("Testing & fixtures") for the policy.
# Pinned to exact minors (not floating "16-alpine"-style tags) so a
# regeneration is reproducible instead of silently drifting to whatever
# minor the tag resolves to that day. This list changes as new minors ship;
# update it here and say so in the commit message (see the policy doc for
# what that message needs to assert).
ROUTINE_VERSIONS = {
    "13": "postgres:13.23-alpine",
    "14": "postgres:14.24-alpine",
    "15": "postgres:15.19-alpine",
    "16": "postgres:16.15-alpine",
    "17": "postgres:17.11-alpine",
    "18": "postgres:18.6-alpine",
}

DB_NAME = "pgdq_fixture"
DB_USER = "postgres"

# Each schema exercises a different concern and so wants a different flag
# list -- see docs/design/roadmap-phase2-typed-columns.md ("Fixtures") for
# why the split exists and why each gets exactly this set.
SCHEMAS: dict[str, dict[str, list[str]]] = {
    "edge_cases": {
        "default": [],
        "data-only": ["--data-only"],
        "schema-only": ["--schema-only"],
        "no-owner": ["--no-owner", "--no-privileges"],
        "clean-if-exists": ["--clean", "--if-exists"],
        "inserts": ["--inserts"],
        "column-inserts": ["--column-inserts"],
        "binary-upgrade": ["--binary-upgrade"],
    },
    "types": {
        "default": [],
        "data-only": ["--data-only"],
        "binary-upgrade": ["--binary-upgrade"],
    },
}


def schema_file(schema: str) -> Path:
    return SCRIPT_DIR / f"fixture_schema_{schema}.sql"


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


def create_fixture_db(name: str, schema: str, attempts: int = 10, delay: float = 1.0) -> None:
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
    schema_sql = schema_file(schema).read_text()
    run(
        DOCKER
        + ["exec", "-i", name, "psql", "-U", DB_USER, "-d", DB_NAME, "-v", "ON_ERROR_STOP=1"],
        input=schema_sql,
        text=True,
        stdout=subprocess.DEVNULL,
    )


def drop_fixture_db(name: str) -> None:
    run(DOCKER + ["exec", name, "dropdb", "-U", DB_USER, DB_NAME], capture_output=True)


def dump_flag_set(name: str, version: str, schema: str, flag_name: str, flags: list[str]) -> Path:
    out_dir = FIXTURES_DIR / version / schema
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


def generate_for_version(version: str, image: str, schemas: list[str]) -> None:
    print(f"== {version} ({image}) ==")
    name = start_container(version, image)
    try:
        wait_ready(name)
        for schema in schemas:
            create_fixture_db(name, schema)
            for flag_name, flags in SCHEMAS[schema].items():
                out_path = dump_flag_set(name, version, schema, flag_name, flags)
                size = out_path.stat().st_size
                print(f"  {schema}/{flag_name}: {out_path.relative_to(REPO_ROOT)} ({size} bytes)")
            drop_fixture_db(name)
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
    parser.add_argument(
        "--schema",
        action="append",
        dest="schemas",
        choices=sorted(SCHEMAS),
        help="limit to specific fixture schema(s); default: all schemas",
    )
    args = parser.parse_args()
    versions = args.versions or sorted(ROUTINE_VERSIONS)
    schemas = args.schemas or sorted(SCHEMAS)

    for version in versions:
        generate_for_version(version, ROUTINE_VERSIONS[version], schemas)

    print("done.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
