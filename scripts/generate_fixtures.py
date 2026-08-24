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

# fixture_schema_objects.sql's non-default tablespace (roadmap-phase3-
# object-inventory.md slice 3.1.1) needs a directory that exists and is
# owned by the container's postgres OS user *before* its `CREATE TABLESPACE`
# statement runs -- see prepare_tablespace_dir.
TABLESPACE_DIR = "/var/lib/postgresql/fixture_tablespace"

# Each schema exercises a different concern and so wants a different flag
# list -- see docs/design/roadmap-phase2-typed-columns.md ("Fixtures") for
# why the split exists and why each gets exactly this set.
#
# `None` is a sentinel meaning "run pg_dumpall instead of pg_dump" -- see
# dump_flag_set. It isn't a `pg_dump` flag set at all, so it can't be
# expressed as a flag list.
#
# A flag-set value is normally a plain flags list (or None, above). It can
# also be a `(min_version, flags)` pair restricting the run to versions >=
# min_version -- introduced for objects/stats: `--statistics` (TOC_PREFIX_STATS,
# roadmap-phase3-object-inventory.md slice 3.1.1) is PG18+ only, and the
# routine matrix runs versions 13-18.
FlagSet = list[str] | None
SCHEMAS: dict[str, dict[str, FlagSet | tuple[str, FlagSet]]] = {
    "edge_cases": {
        "default": [],
        "data-only": ["--data-only"],
        "schema-only": ["--schema-only"],
        "no-owner": ["--no-owner", "--no-privileges"],
        "clean-if-exists": ["--clean", "--if-exists"],
        "inserts": ["--inserts"],
        "column-inserts": ["--column-inserts"],
        "binary-upgrade": ["--binary-upgrade"],
        "create": ["--create"],
        "no-comments": ["--no-comments", "--no-security-labels"],
        "dumpall": None,
    },
    "types": {
        "default": [],
        "data-only": ["--data-only"],
        "binary-upgrade": ["--binary-upgrade"],
    },
    # roadmap-phase3-object-inventory.md's slice 3.1: `--verbose` is the one
    # documented way to widen the TOC comment block past three lines (the
    # `-- TOC entry ... (class OID)` / `-- Dependencies: ...` lines), so it's
    # this schema's whole reason for a flag set beyond the default.
    "objects": {
        "default": [],
        "verbose": ["--verbose"],
        "stats": ("18", ["--statistics"]),
    },
    # roadmap-phase3-object-inventory.md's "Mapping and streaming are
    # separate passes": the one shape where a single `COPY <name>` header
    # owns several blocks (I2). `default` already produces it -- pg_dump
    # forces load-via-partition-root for hash-on-enum partitioning with no
    # flag -- and the explicit flag extends it to the LIST-partitioned
    # table too, so both sets are needed to cover both routes.
    "partitions": {
        "default": [],
        "load-via-partition-root": ["--load-via-partition-root"],
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


def prepare_tablespace_dir(name: str) -> None:
    # `docker exec` defaults to root in the official postgres image (the
    # entrypoint drops to the postgres OS user itself via gosu, but that
    # doesn't apply to a fresh exec) -- root can mkdir here, but CREATE
    # TABLESPACE needs the directory owned by the user postgres itself
    # connects as, so chown it explicitly rather than relying on the
    # container's default exec user.
    run(DOCKER + ["exec", name, "mkdir", "-p", TABLESPACE_DIR])
    run(DOCKER + ["exec", name, "chown", "postgres:postgres", TABLESPACE_DIR])


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
    if schema == "objects":
        prepare_tablespace_dir(name)
    schema_sql = schema_file(schema).read_text()
    run(
        DOCKER
        + ["exec", "-i", name, "psql", "-U", DB_USER, "-d", DB_NAME, "-v", "ON_ERROR_STOP=1"],
        input=schema_sql,
        text=True,
        stdout=subprocess.DEVNULL,
    )


def drop_fixture_db(name: str) -> None:
    # fixture_schema_objects.sql creates a subscription, a role and a
    # tablespace, all of which outlive `dropdb` unless cleared explicitly: a
    # subscription blocks dropping its own database outright, and both a
    # role and a tablespace are cluster-global so they'd otherwise leak into
    # whatever schema runs next in this same container. All three commands
    # are IF EXISTS, so they're safe no-ops for every other schema. The
    # subscription's slot_name = NONE (see that file) is what makes DROP
    # SUBSCRIPTION not need a reachable publisher here; the tablespace drops
    # cleanly once `dropdb` below has removed the only objects using it.
    run(
        DOCKER
        + ["exec", name, "psql", "-U", DB_USER, "-d", DB_NAME, "-c", "DROP SUBSCRIPTION IF EXISTS objects_sub"],
        capture_output=True,
    )
    run(DOCKER + ["exec", name, "dropdb", "-U", DB_USER, DB_NAME], capture_output=True)
    run(
        DOCKER + ["exec", name, "psql", "-U", DB_USER, "-c", "DROP ROLE IF EXISTS fixture_reader"],
        capture_output=True,
    )
    run(
        DOCKER + ["exec", name, "psql", "-U", DB_USER, "-c", "DROP TABLESPACE IF EXISTS fixture_ts"],
        capture_output=True,
    )


def dump_flag_set(
    name: str, version: str, schema: str, flag_name: str, flags: list[str] | None
) -> Path:
    out_dir = FIXTURES_DIR / version / schema
    out_dir.mkdir(parents=True, exist_ok=True)
    out_path = out_dir / f"{flag_name}.sql"
    if flags is None:
        # dumpall: the whole cluster (postgres/template1 plus DB_NAME), not
        # a `pg_dump` invocation against one database -- `--no-role-passwords`
        # keeps the output deterministic (no password hashes to vary run to
        # run). Run before drop_fixture_db so DB_NAME is still loaded.
        cmd = DOCKER + ["exec", name, "pg_dumpall", "-U", DB_USER, "--no-role-passwords"]
    else:
        cmd = DOCKER + ["exec", name, "pg_dump", "-U", DB_USER, *flags, DB_NAME]
    result = run(cmd, stdout=subprocess.PIPE, text=True)
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
            for flag_name, flag_spec in SCHEMAS[schema].items():
                if isinstance(flag_spec, tuple):
                    min_version, flags = flag_spec
                    if int(version) < int(min_version):
                        continue
                else:
                    flags = flag_spec
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
