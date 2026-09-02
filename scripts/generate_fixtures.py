#!/usr/bin/env python3
"""Generate pg_dump fixture files across PostgreSQL versions and flag combos.

Backs docs/design/architecture.md ("Fixtures"), the
tested/untested matrix in docs/design/pg-dump-compatibility.md, and
same doc's fixture-tree rules. Spins up a
throwaway, memory-limited Postgres container per version (never a host-run
process, per this repo's CPU-heavy-machine / glibc-arena caution), loads one
of two fixture schemas, runs pg_dump across each schema's own flag matrix, and
writes the output under fixtures/<major-version>/<schema>/<flag-set>.sql.

It also generates the **comparison oracle** -- what the server itself answers
for a table of typed comparisons, written under
fixtures/<major-version>/oracle/ (docs/design/architecture.md, "The comparison
oracle"). That pass lives here rather than in its own script because it runs
against the `types` schema's own database, so it must see the same DDL, in the
same container, as fixtures/<major-version>/types/*.sql. The case table and the
SQL are scripts/comparison_oracle.py.

An oracle pass ends by re-checking two things, and fails on either. The
cross-major differences (scripts/oracle_differences.py) against
fixtures/oracle-differences.tsv: a regenerated answer that changed something
has to be filed rather than noticed later. And the register-to-oracle
reconciliation (scripts/oracle_register.py), which is where a case added for a
type fixture_schema_types.sql does not declare surfaces -- the moment it is
generated, rather than as a column of `E42704` nobody reads.

A third pass takes the **ADBC floor oracle** -- what the Arrow ADBC PostgreSQL
driver returns for every declarable `pg_catalog` type, written under
fixtures/<major-version>/adbc/ (docs/design/architecture.md, "The ADBC floor
oracle"). Its sweep and file format are scripts/adbc_floor.py, and the pass
ends by running the floor-to-mapping reconciliation (scripts/floor_mapping.py)
for the same reason an oracle pass runs the other two: a driver release that
answers a type differently has to be met with a stance or a mapping at the
moment it is taken, not found later. Unlike the
comparison oracle it needs no fixture DDL -- it is a pg_catalog question -- but
it does need the container reachable *from the host*, because the driver is a
pip wheel in this script's own `uv` environment rather than something installed
into a `postgres:` image. That is what HOST_PORT is for.

Requires `docker` (aliased to `nerdctl` in this environment) runnable via
passwordless `sudo`.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

import adbc_floor
import floor_mapping
import comparison_oracle
import oracle_differences
import oracle_register

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
FIXTURES_DIR = REPO_ROOT / "fixtures"

DOCKER = ["sudo", "-n", "docker"]

# The fixture container's port, published on the loopback interface so the ADBC
# floor sweep can reach it from the host (adbc_floor.py: the driver is a wheel,
# not something to install into a `postgres:` image). Versions run one at a
# time, so one port serves all of them. 5432 and 5433 are taken by unrelated
# long-lived containers on this machine (CLAUDE.local.md), which is why this is
# neither.
HOST_PORT = 55432

# Routine version set: the latest minor release of every PostgreSQL major
# from 13 onward (13 being the oldest still-supported major) -- see
# docs/design/architecture.md ("Fixtures") for the policy.
#
# Pinned to exact minors (not floating "16-trixie"-style tags) so a
# regeneration is reproducible instead of silently drifting to whatever
# minor the tag resolves to that day. This list changes as new minors ship;
# update it here and say so in the commit message (see the policy doc for
# what that message needs to assert).
#
# **`-trixie` at every minor, never the unsuffixed tag and never `-alpine`.**
# The suffix is what pins the libc, and the libc is what the comparison
# oracle's text answers are taken under: on Alpine (musl) `strcoll` is
# `strcmp`, so every text comparison would silently be the `C`-collation
# answer. The unsuffixed `postgres:16` has already moved Debian suites once,
# which is the drift the pin exists against. `meta.tsv` records the platform
# triple and the default collation's version, and `oracle_differences.py`
# guards both across majors, so a version left on a different base is a
# reported fault rather than five hundred silent differences.
ROUTINE_VERSIONS = {
    "13": "postgres:13.23-trixie",
    "14": "postgres:14.24-trixie",
    "15": "postgres:15.19-trixie",
    "16": "postgres:16.15-trixie",
    "17": "postgres:17.11-trixie",
    "18": "postgres:18.6-trixie",
}

DB_NAME = "pgdq_fixture"
DB_USER = "postgres"

# A second database, loaded for the `edge_cases` schema only, so that its
# `dumpall` flag set produces a file in which *two* `\connect` segments carry
# `COPY` blocks -- the only shape that exercises I1's recurring
# metadata boundary (architecture.md, "Fixtures"). By I30 this name sorts
# between DB_NAME and `postgres`, so the segment order is fixed by naming
# rather than observed. See scripts/fixture_schema_edge_cases_tenant.sql for
# why its tables look the way they do.
TENANT_DB_NAME = "pgdq_tenant"
TENANT_SCHEMA = "edge_cases"

# fixture_schema_objects.sql's non-default tablespace (architecture.md,
# "Fixtures") needs a directory that exists and is
# owned by the container's postgres OS user *before* its `CREATE TABLESPACE`
# statement runs -- see prepare_tablespace_dir.
TABLESPACE_DIR = "/var/lib/postgresql/fixture_tablespace"

# Each schema exercises a different concern and so wants a different flag
# list -- see docs/design/architecture.md ("Fixtures") for
# why the split exists and why each gets exactly this set.
#
# `None` is a sentinel meaning "run pg_dumpall instead of pg_dump" -- see
# dump_flag_set. It isn't a `pg_dump` flag set at all, so it can't be
# expressed as a flag list.
#
# A flag-set value is normally a plain flags list (or None, above). It can
# also be a `(min_version, flags)` pair restricting the run to versions >=
# min_version -- introduced for objects/stats: `--statistics` (TOC_PREFIX_STATS,
# architecture.md, "Fixtures") is PG18+ only, and the
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
    # architecture.md, "Fixtures": `--verbose` is the one
    # documented way to widen the TOC comment block past three lines (the
    # `-- TOC entry ... (class OID)` / `-- Dependencies: ...` lines), so it's
    # this schema's whole reason for a flag set beyond the default.
    "objects": {
        "default": [],
        "verbose": ["--verbose"],
        "stats": ("18", ["--statistics"]),
    },
    # architecture.md's "Query: mapping and streaming are
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


def tenant_schema_file() -> Path:
    # Named for the fixture set it belongs to, not as a fifth fixture set:
    # nothing dumps `pgdq_tenant` on its own, it only ever shows up inside
    # `edge_cases/dumpall.sql`.
    return SCRIPT_DIR / f"fixture_schema_{TENANT_SCHEMA}_tenant.sql"


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
            "-p",
            f"127.0.0.1:{HOST_PORT}:5432",
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


def load_sql(name: str, database: str, sql: str) -> None:
    run(
        DOCKER
        + ["exec", "-i", name, "psql", "-U", DB_USER, "-d", database, "-v", "ON_ERROR_STOP=1"],
        input=sql,
        text=True,
        stdout=subprocess.DEVNULL,
    )


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
    load_sql(name, DB_NAME, schema_file(schema).read_text())
    if schema == TENANT_SCHEMA:
        # A second database in the same cluster, so this schema's `dumpall`
        # flag set has two `COPY`-carrying segments. Gated here rather than in
        # dump_flag_set, which stays a pure dump-and-write function.
        run(DOCKER + ["exec", name, "createdb", "-U", DB_USER, TENANT_DB_NAME], capture_output=True)
        load_sql(name, TENANT_DB_NAME, tenant_schema_file().read_text())


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
    # `--if-exists`, so this is a no-op for every schema but TENANT_SCHEMA:
    # a leaked database would appear in a later schema's `pg_dumpall` output.
    run(
        DOCKER + ["exec", name, "dropdb", "-U", DB_USER, "--if-exists", TENANT_DB_NAME],
        capture_output=True,
    )
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


# The oracle's cases name user-defined types -- the enum, the composites, the
# two user-defined ranges, the domains -- that exist only in this schema, so
# it is the database the oracle is asked in.
ORACLE_SCHEMA = "types"


def write_oracle(name: str, version: str) -> None:
    """Run the comparison oracle's scripts against a loaded ORACLE_SCHEMA
    database and write each one's `COPY ... TO STDOUT` output verbatim.

    One `psql` per file: each script is self-contained (its helpers live in
    `pg_temp`, which dies with the session), and psql is quiet so that the
    only thing on stdout is the COPY stream.
    """
    out_dir = FIXTURES_DIR / version / comparison_oracle.ORACLE_DIRNAME
    out_dir.mkdir(parents=True, exist_ok=True)
    for filename, build in comparison_oracle.SCRIPTS.items():
        result = run(
            DOCKER
            + [
                "exec",
                "-i",
                name,
                "psql",
                "-U",
                DB_USER,
                "-d",
                DB_NAME,
                "-q",
                "-X",
                "-v",
                "ON_ERROR_STOP=1",
            ],
            input=build(),
            text=True,
            stdout=subprocess.PIPE,
        )
        out_path = out_dir / filename
        out_path.write_text(result.stdout)
        rows = result.stdout.count("\n")
        print(f"  oracle/{filename}: {out_path.relative_to(REPO_ROOT)} ({rows} rows)")


def take_floor(version: str) -> None:
    """Take the ADBC floor oracle for one server and write `adbc/floor.tsv`.

    Against the `postgres` database rather than a fixture one: the sweep is a
    `pg_catalog` question and sees nothing any fixture schema loads, so it owes
    no `create_fixture_db`. From the host over the published port, because the
    driver is a wheel in this script's own `uv` environment (adbc_floor.py).
    """
    conn = adbc_floor.connect(f"postgresql://{DB_USER}@127.0.0.1:{HOST_PORT}/postgres")
    try:
        rows = adbc_floor.sweep(conn, version)
    finally:
        conn.close()
    out_path = adbc_floor.write_floor(FIXTURES_DIR, version, rows)
    print(
        f"  adbc/{adbc_floor.FLOOR_FILENAME}: {out_path.relative_to(REPO_ROOT)} "
        f"({len(rows)} rows, driver {adbc_floor.driver_version()})"
    )


def generate_for_version(
    version: str, image: str, schemas: list[str], dumps: bool, oracle: bool, floor: bool
) -> None:
    print(f"== {version} ({image}) ==")
    name = start_container(version, image)
    try:
        wait_ready(name)
        if floor:
            take_floor(version)
        if dumps:
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
                    print(
                        f"  {schema}/{flag_name}: "
                        f"{out_path.relative_to(REPO_ROOT)} ({size} bytes)"
                    )
                drop_fixture_db(name)
        if oracle:
            create_fixture_db(name, ORACLE_SCHEMA)
            try:
                write_oracle(name, version)
            finally:
                drop_fixture_db(name)
    finally:
        stop_container(name)


# How long a full regeneration takes is recorded in
# docs/design/architecture.md ("Fixtures"), and this is the threshold past
# which that figure has stopped being true. Thirty minutes is where a job
# stops fitting inside one session and has to be handed off (CLAUDE.md,
# "Long-running processes"), so a run that crosses it changes what a later
# session has to plan for. The trigger is this print rather than a comment
# asking someone to notice, because a comment is read when the file is edited
# and this has to fire when the run is slow.
STALE_AFTER_SECONDS = 30 * 60


def report_elapsed(seconds: float) -> None:
    """The run's own duration, printed unconditionally, with the staleness
    warning when it crosses [`STALE_AFTER_SECONDS`]."""
    print(f"\nelapsed: {seconds / 60:.1f} min ({seconds:.0f} s)")
    if seconds > STALE_AFTER_SECONDS:
        print(
            f"this run took longer than {STALE_AFTER_SECONDS // 60} minutes: the "
            'figure in docs/design/architecture.md ("Fixtures") is stale and must '
            "be updated, and a regeneration now has to be handed off to a later "
            "session rather than run inline.",
            file=sys.stderr,
        )


def main() -> int:
    started = time.monotonic()
    try:
        return _run()
    finally:
        report_elapsed(time.monotonic() - started)


def _run() -> int:
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
    parser.add_argument(
        "--skip-dumps",
        action="store_true",
        help="don't run pg_dump at all; useful with the oracle, which is a "
        "separate pass over the same containers",
    )
    parser.add_argument(
        "--skip-oracle",
        action="store_true",
        help="don't regenerate fixtures/<version>/oracle/",
    )
    parser.add_argument(
        "--skip-floor",
        action="store_true",
        help="don't regenerate fixtures/<version>/adbc/floor.tsv, the ADBC "
        "floor oracle; it is a third pass over the same containers",
    )
    args = parser.parse_args()
    versions = args.versions or sorted(ROUTINE_VERSIONS)
    schemas = args.schemas or sorted(SCHEMAS)
    if args.skip_dumps and args.skip_oracle and args.skip_floor:
        parser.error("--skip-dumps, --skip-oracle and --skip-floor together leave nothing to do")

    for version in versions:
        generate_for_version(
            version,
            ROUTINE_VERSIONS[version],
            schemas,
            dumps=not args.skip_dumps,
            oracle=not args.skip_oracle,
            floor=not args.skip_floor,
        )

    if not args.skip_oracle:
        print("\ncross-major differences:")
        if oracle_differences.check() != 0:
            print(
                "the committed differences no longer match the oracles — re-file "
                "them with `uv run oracle_differences.py --write` and read what "
                "moved before committing.",
                file=sys.stderr,
            )
            return 1

        print("\nregister-to-oracle reconciliation:")
        if oracle_register.check() != 0:
            print(
                "the comparison register and the case table no longer cover each "
                "other — read what `uv run oracle_register.py` names before "
                "committing.",
                file=sys.stderr,
            )
            return 1

    if not args.skip_floor:
        print("\nfloor-to-mapping reconciliation:")
        if floor_mapping.check() != 0:
            print(
                "the ADBC floor and our own mapping no longer cover each other — "
                "read what `uv run floor_mapping.py` names before committing; a "
                "row the driver newly answers needs a mapping or a stance.",
                file=sys.stderr,
            )
            return 1

    print("done.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
