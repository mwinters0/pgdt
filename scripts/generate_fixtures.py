#!/usr/bin/env python3
"""Generate pg_dump fixture files across PostgreSQL versions and flag combos.

Backs docs/design/decisions.md ("D69"), the
tested/untested matrix in docs/design/pg-dump-compatibility.md, and
same doc's fixture-tree rules. Spins up a
throwaway, memory-limited Postgres container per version (never a host-run
process, per this repo's CPU-heavy-machine / glibc-arena caution), loads one
fixture schema at a time, runs pg_dump across each schema's own flag matrix, and
writes the output under fixtures/<major-version>/<schema>/<flag-set>.sql.

It also generates the **comparison oracle** -- what the server itself answers
for a table of typed comparisons, written under
fixtures/<major-version>/oracle/ (docs/design/decisions.md, "D70"). That pass lives here rather than in its own script because it runs
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

A second pass in the same database takes the **value oracle** -- the server's
own reading of every typed value of the `types` schema, not in its output
spelling, written to fixtures/<major-version>/oracle/values.tsv
(scripts/value_oracle.py, which also holds the reconciliation the pass ends
with: every typed arm read at every major).

A third pass takes the **ADBC floor oracle** -- what the Arrow ADBC PostgreSQL
driver returns for every declarable `pg_catalog` type, written under
fixtures/<major-version>/adbc/ (docs/design/decisions.md, "D38"). Its sweep and file format are scripts/adbc_floor.py, and the pass
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
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

import adbc_floor
import floor_mapping
import comparison_oracle
import oracle_differences
import oracle_register
import value_oracle

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
# docs/design/decisions.md ("D69") for the policy.
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

DB_NAME = "pgdt_fixture"
DB_USER = "postgres"

# A second database, loaded for the `edge_cases` schema only, so that its
# `dumpall` flag set produces a file in which *two* `\connect` segments carry
# `COPY` blocks -- the only shape that exercises I1's recurring
# metadata boundary (decisions.md, "D69"). By I30 this name sorts
# between DB_NAME and `postgres`, so the segment order is fixed by naming
# rather than observed. See scripts/fixture_schema_edge_cases_tenant.sql for
# why its tables look the way they do.
TENANT_DB_NAME = "pgdt_tenant"
TENANT_SCHEMA = "edge_cases"

# fixture_schema_objects.sql's non-default tablespace (decisions.md,
# "D69") needs a directory that exists and is
# owned by the container's postgres OS user *before* its `CREATE TABLESPACE`
# statement runs -- see prepare_tablespace_dir.
TABLESPACE_DIR = "/var/lib/postgresql/fixture_tablespace"

# Each schema exercises a different concern and so wants a different flag
# list -- see docs/design/decisions.md ("D69") for
# why the split exists and why each gets exactly this set.
#
# A [`Dumpall`] runs `pg_dumpall` over the whole cluster instead of `pg_dump`
# over the schema's database, with its own flags -- see dump_flag_set.
#
# A flag-set value is normally a plain flags list (or a `Dumpall`, above). It
# can also be a `(min_version, flags)` pair restricting the run to versions >=
# min_version -- introduced for objects/stats: `--statistics` (TOC_PREFIX_STATS,
# decisions.md, "D69") is PG18+ only, and the
# routine matrix runs versions 13-18.
#
# A third shape is a **session-setting variant**, [`Setting`]: a plain `pg_dump`
# run while the schema's database carries one setting `pg_dump` does not pin
# (I4), set by `ALTER DATABASE ... SET` as a real server's would be. One
# variant per setting, never a cross product
# (docs/design/roadmap-P31-correctness-evidence.md, "The session-setting axis").


@dataclass(frozen=True)
class Setting:
    """A flag set that is a setting rather than a flag: `name = value` on the
    database for the length of one plain dump, reset after it."""

    name: str
    value: str


@dataclass(frozen=True)
class Dumpall:
    """A flag set that is a `pg_dumpall` run over the cluster, with these
    flags besides [`PG_DUMPALL_ARGS`]."""

    flags: tuple[str, ...] = ()


FlagSet = list[str] | Dumpall | Setting
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
        "dumpall": Dumpall(),
        # The `INSERT` shapes the two run above do not write, and the one
        # `--data-only` shape that wraps each block in trigger toggles.
        "rows-per-insert": ["--rows-per-insert=2"],
        "on-conflict-do-nothing": ["--inserts", "--on-conflict-do-nothing"],
        "disable-triggers": ["--data-only", "--disable-triggers"],
    },
    "types": {
        "default": [],
        "data-only": ["--data-only"],
        "binary-upgrade": ["--binary-upgrade"],
        "quote-all-identifiers": ["--quote-all-identifiers"],
        "extra-float-digits-0": ["--extra-float-digits=0"],
        "bytea-output-escape": Setting("bytea_output", "escape"),
        "timezone-monrovia": Setting("TimeZone", "Africa/Monrovia"),
    },
    # decisions.md, "D69": `--verbose` is the one
    # documented way to widen the TOC comment block past three lines (the
    # `-- TOC entry ... (class OID)` / `-- Dependencies: ...` lines), so it's
    # this schema's whole reason for a flag set beyond the default.
    "objects": {
        "default": [],
        "verbose": ["--verbose"],
        "stats": ("18", ["--statistics"]),
        # The foreign table's rows, which no other flag set dumps.
        "include-foreign-data": ["--include-foreign-data=objects_files"],
        # Every other `pg_dump` option that moves output bytes, each passed by
        # some set here, since this schema holds an object of every kind an
        # option selects or omits (`emitter_register.py`'s option half). Sets
        # combine options that do not conflict; one a major lacks waits for
        # its minimum.
        "selection-schema": [
            "--schema=objects",
            "--exclude-table=objects.secrets",
            "--exclude-table-data=objects.orders",
            "--blobs",
            "--strict-names",
        ],
        "selection-table": ["--table=objects.widgets", "--table=objects.orders"],
        "omit": [
            "--exclude-schema=public",
            "--no-tablespaces",
            "--no-publications",
            "--no-subscriptions",
            "--no-unlogged-table-data",
            "--no-blobs",
            "--no-acl",
            "--enable-row-security",
            "--disable-dollar-quoting",
            "--use-set-session-authorization",
            "--attribute-inserts",
        ],
        "sections": ["--section=pre-data", "--section=post-data", "--restrict-key=pgdtfixture"],
        "extension": ("14", ["--extension=file_fdw", "--no-toast-compression"]),
        "no-table-access-method": ("15", ["--schema=objects", "--no-table-access-method"]),
        "large-objects": (
            "16",
            [
                "--schema=objects",
                "--large-objects",
                "--exclude-table-and-children=objects.events",
                "--exclude-table-data-and-children=objects.orders",
            ],
        ),
        "table-and-children": ("16", ["--table-and-children=objects.events", "--no-large-objects"]),
        # The filter file is written by the schema's own script.
        "filter": ("17", ["--filter=/tmp/objects_filter.txt", "--exclude-extension=postgres_fdw"]),
        "no-data": ("18", ["--no-data", "--no-policies", "--no-statistics", "--sequence-data"]),
        "no-schema": ("18", ["--no-schema"]),
        "statistics-only": ("18", ["--statistics-only"]),
    },
    # A literal of `pg_dump`'s emitters no other schema reaches
    # (fixture_schema_emitters.sql), and the `pg_dumpall` runs: its globals
    # come from `CLUSTER_SCRIPTS`, and each of its options is passed by some
    # set here.
    "emitters": {
        "default": [],
        "binary-upgrade": ["--binary-upgrade"],
        "clean": ["--clean", "--if-exists"],
        "data-only": ["--data-only", "--disable-triggers", "--superuser=postgres"],
        "dumpall": Dumpall(),
        "dumpall-clean": Dumpall(("--clean", "--if-exists", "--exclude-database=postgres")),
        "dumpall-binary-upgrade": Dumpall(("--binary-upgrade",)),
        "dumpall-data-only": Dumpall(
            (
                "--data-only",
                "--inserts",
                "--column-inserts",
                "--attribute-inserts",
                "--rows-per-insert=2",
                "--on-conflict-do-nothing",
                "--disable-triggers",
                "--superuser=postgres",
                "--extra-float-digits=3",
                "--load-via-partition-root",
            )
        ),
        "dumpall-schema-only": Dumpall(
            (
                "--schema-only",
                "--no-comments",
                "--no-owner",
                "--no-privileges",
                "--no-acl",
                "--no-security-labels",
                "--no-publications",
                "--no-subscriptions",
                "--no-tablespaces",
                "--no-unlogged-table-data",
                "--use-set-session-authorization",
                "--disable-dollar-quoting",
                "--quote-all-identifiers",
                "--restrict-key=pgdtfixture",
            )
        ),
        "dumpall-globals-only": Dumpall(("--globals-only", "--verbose")),
        "dumpall-roles-only": Dumpall(("--roles-only",)),
        "dumpall-tablespaces-only": Dumpall(("--tablespaces-only",)),
        "dumpall-no-toast-compression": ("14", Dumpall(("--schema-only", "--no-toast-compression"))),
        "dumpall-no-table-access-method": (
            "15",
            Dumpall(("--schema-only", "--no-table-access-method")),
        ),
        # The filter file is written by the schema's own script.
        "dumpall-filter": ("17", Dumpall(("--filter=/tmp/emitters_dumpall_filter.txt",))),
        "dumpall-no-data": (
            "18",
            Dumpall(("--no-data", "--no-policies", "--no-statistics", "--sequence-data")),
        ),
        "dumpall-statistics": ("18", Dumpall(("--no-schema", "--statistics"))),
        "dumpall-statistics-only": ("18", Dumpall(("--statistics-only",))),
    },
    # decisions.md's "D48": the one shape where a single `COPY <name>` header
    # owns several blocks (I2). `default` already produces it -- pg_dump
    # forces load-via-partition-root for hash-on-enum partitioning with no
    # flag -- and the explicit flag extends it to the LIST-partitioned
    # table too, so both sets are needed to cover both routes.
    "partitions": {
        "default": [],
        "load-via-partition-root": ["--load-via-partition-root"],
    },
    # The column shapes per-row-group statistics are tested against
    # (fixture_schema_statistics.sql). Every other flag set above varies what
    # `pg_dump` writes around a `COPY` block, and this schema's subject is what
    # is inside one; `--load-via-partition-root` is the exception, being what
    # makes one table's rows several blocks, which a table's statistics are
    # combined across (I2).
    "statistics": {
        "default": [],
        "load-via-partition-root": ["--load-via-partition-root"],
    },
}


# What every run passes besides its flag set, named so the emitter register
# (scripts/emitter_register.py) counts these options as run too.
PG_DUMP_ARGS = ["-U", DB_USER]
PG_DUMPALL_ARGS = ["-U", DB_USER, "--no-role-passwords"]


def schema_file(schema: str) -> Path:
    return SCRIPT_DIR / f"fixture_schema_{schema}.sql"


_SIDECAR = re.compile(r"^fixture_schema_(?P<schema>[a-z_]+)\.(?P<major>[0-9]+)\.sql$")


def schema_files(schema: str, version: str, directory: Path = SCRIPT_DIR) -> list[Path]:
    """The files loaded for `schema` at `version`, in load order: the base
    file, then each **version sidecar** `fixture_schema_<schema>.<major>.sql`
    whose major is at or below `version`, lowest first.

    A sidecar holds DDL an older major refuses, so the base file loads on
    every major and the sidecar only where it parses
    (docs/design/roadmap-P31-correctness-evidence.md, "Version-conditioned
    schemas"). A file named like a sidecar of this schema whose middle is not
    a major is an error, not a file skipped.
    """
    sidecars: list[tuple[int, Path]] = []
    for path in directory.glob(f"fixture_schema_{schema}.*.sql"):
        match = _SIDECAR.match(path.name)
        if match is None or match["schema"] != schema:
            raise ValueError(f"{path.name}: a sidecar is fixture_schema_{schema}.<major>.sql")
        sidecars.append((int(match["major"]), path))
    return [directory / f"fixture_schema_{schema}.sql"] + [
        path for major, path in sorted(sidecars) if major <= int(version)
    ]


@dataclass(frozen=True)
class ClusterScript:
    """Cluster-global objects a schema's `pg_dumpall` runs read: a script run
    in the `postgres` database before the schema loads, the tablespace
    directory it needs, and what [`drop_fixture_db`] removes after."""

    file: str
    tablespace_dir: str
    tablespaces: tuple[str, ...]
    databases: tuple[str, ...]


#: The `emitters` schema's globals: a tablespace with options and a comment,
#: and a database whose name `appendPsqlMetaConnect` cannot write bare
#: (fixture_schema_emitters_cluster.sql).
CLUSTER_SCRIPTS: dict[str, ClusterScript] = {
    "emitters": ClusterScript(
        file="fixture_schema_emitters_cluster.sql",
        tablespace_dir="/var/lib/postgresql/emitters_tablespace",
        tablespaces=("emitters_ts",),
        databases=("pgdt-emitters",),
    ),
}


def tenant_schema_file() -> Path:
    # Named for the fixture set it belongs to, not as a fifth fixture set:
    # nothing dumps `pgdt_tenant` on its own, it only ever shows up inside
    # `edge_cases/dumpall.sql`.
    return SCRIPT_DIR / f"fixture_schema_{TENANT_SCHEMA}_tenant.sql"


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=True, **kwargs)


def container_name(version: str) -> str:
    return f"pgdt-fixture-{version}"


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


def prepare_tablespace_dir(name: str, directory: str = TABLESPACE_DIR) -> None:
    # `docker exec` defaults to root in the official postgres image (the
    # entrypoint drops to the postgres OS user itself via gosu, but that
    # doesn't apply to a fresh exec) -- root can mkdir here, but CREATE
    # TABLESPACE needs the directory owned by the user postgres itself
    # connects as, so chown it explicitly rather than relying on the
    # container's default exec user.
    run(DOCKER + ["exec", name, "mkdir", "-p", directory])
    run(DOCKER + ["exec", name, "chown", "postgres:postgres", directory])


def load_sql(name: str, database: str, sql: str) -> None:
    run(
        DOCKER
        + ["exec", "-i", name, "psql", "-U", DB_USER, "-d", database, "-v", "ON_ERROR_STOP=1"],
        input=sql,
        text=True,
        stdout=subprocess.DEVNULL,
    )


def create_fixture_db(
    name: str, version: str, schema: str, attempts: int = 10, delay: float = 1.0
) -> None:
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
    if (cluster := CLUSTER_SCRIPTS.get(schema)) is not None:
        prepare_tablespace_dir(name, cluster.tablespace_dir)
        load_sql(name, "postgres", (SCRIPT_DIR / cluster.file).read_text())
    for path in schema_files(schema, version):
        load_sql(name, DB_NAME, path.read_text())
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
    for cluster in CLUSTER_SCRIPTS.values():
        for database in cluster.databases:
            run(DOCKER + ["exec", name, "dropdb", "-U", DB_USER, "--if-exists", database], capture_output=True)
        for tablespace in cluster.tablespaces:
            psql_command(name, "postgres", f"DROP TABLESPACE IF EXISTS {tablespace}")


def psql_command(name: str, database: str, sql: str) -> None:
    run(DOCKER + ["exec", name, "psql", "-U", DB_USER, "-d", database, "-c", sql], capture_output=True)


def dump_flag_set(
    name: str, version: str, schema: str, flag_name: str, flags: FlagSet
) -> Path:
    out_dir = FIXTURES_DIR / version / schema
    out_dir.mkdir(parents=True, exist_ok=True)
    out_path = out_dir / f"{flag_name}.sql"
    if isinstance(flags, Setting):
        # On the database, so the dump's own new session takes it as a
        # server's would hand it over; reset before any other flag set runs.
        psql_command(name, DB_NAME, f"ALTER DATABASE {DB_NAME} SET {flags.name} = '{flags.value}'")
        try:
            cmd = DOCKER + ["exec", name, "pg_dump", *PG_DUMP_ARGS, DB_NAME]
            result = run(cmd, stdout=subprocess.PIPE, text=True)
        finally:
            psql_command(name, DB_NAME, f"ALTER DATABASE {DB_NAME} RESET {flags.name}")
        out_path.write_text(result.stdout)
        return out_path
    if isinstance(flags, Dumpall):
        # dumpall: the whole cluster (postgres/template1 plus DB_NAME), not
        # a `pg_dump` invocation against one database -- `--no-role-passwords`
        # keeps the output deterministic (no password hashes to vary run to
        # run). Run before drop_fixture_db so DB_NAME is still loaded.
        cmd = DOCKER + ["exec", name, "pg_dumpall", *PG_DUMPALL_ARGS, *flags.flags]
    else:
        cmd = DOCKER + ["exec", name, "pg_dump", *PG_DUMP_ARGS, *flags, DB_NAME]
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


def psql_output(name: str, sql: str, *flags: str) -> str:
    """What a quiet `psql` session over the fixture database prints for
    `sql` on stdin."""
    return run(
        DOCKER
        + ["exec", "-i", name, "psql", "-U", DB_USER, "-d", DB_NAME, "-q", "-X"]
        + ["-v", "ON_ERROR_STOP=1", *flags],
        input=sql,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout


def write_values(name: str, version: str) -> None:
    """The value oracle over a loaded ORACLE_SCHEMA database: the catalog
    first, which the reading script is built from, then the script."""
    catalog = value_oracle.parse_catalog(psql_output(name, value_oracle.CATALOG_SQL, "-A", "-t"))
    out = psql_output(name, value_oracle.values_script(catalog))
    out_path = value_oracle.values_path(version)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(out)
    print(f"  oracle/{out_path.name}: {out_path.relative_to(REPO_ROOT)} ({out.count(chr(10))} rows)")


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
    version: str,
    image: str,
    schemas: list[str],
    dumps: bool,
    oracle: bool,
    floor: bool,
    values: bool,
) -> None:
    print(f"== {version} ({image}) ==")
    name = start_container(version, image)
    try:
        wait_ready(name)
        if floor:
            take_floor(version)
        if dumps:
            for schema in schemas:
                create_fixture_db(name, version, schema)
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
        if oracle or values:
            create_fixture_db(name, version, ORACLE_SCHEMA)
            try:
                if oracle:
                    write_oracle(name, version)
                if values:
                    write_values(name, version)
            finally:
                drop_fixture_db(name)
    finally:
        stop_container(name)


# How long a full regeneration takes is recorded in
# docs/design/decisions.md ("D69"), and this is the threshold past
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
            'figure in docs/design/decisions.md ("D69") is stale and must '
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
    parser.add_argument(
        "--skip-values",
        action="store_true",
        help="don't regenerate fixtures/<version>/oracle/values.tsv, the value "
        "oracle; a pass of its own over the comparison oracle's database",
    )
    args = parser.parse_args()
    versions = args.versions or sorted(ROUTINE_VERSIONS)
    schemas = args.schemas or sorted(SCHEMAS)
    if args.skip_dumps and args.skip_oracle and args.skip_floor and args.skip_values:
        parser.error(
            "--skip-dumps, --skip-oracle, --skip-floor and --skip-values together "
            "leave nothing to do"
        )

    for version in versions:
        generate_for_version(
            version,
            ROUTINE_VERSIONS[version],
            schemas,
            dumps=not args.skip_dumps,
            oracle=not args.skip_oracle,
            floor=not args.skip_floor,
            values=not args.skip_values,
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

    if not args.skip_values:
        print("\nvalue-oracle reconciliation:")
        if value_oracle.check() != 0:
            print(
                "a typed arm has no value-oracle reading — read what "
                "`uv run value_oracle.py` names before committing.",
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
