#!/usr/bin/env python3
"""The ADBC floor oracle: what somebody else's driver returns for a type.

`docs/design/roadmap-P12-adbc-type-floor.md` is the binding spec and
`docs/design/architecture.md`, "The ADBC floor oracle", is the description;
this module is the sweep and the file format. It has no container plumbing of
its own -- `generate_fixtures.py` owns that and calls in here, the same
division the comparison oracle already runs on.

**What it records.** One row per `pg_type` row a user could declare a column
of, saying what Arrow type `adbc_driver_postgresql` hands back for it. That is
the *floor*: wherever the driver yields a real Arrow type, ours is never a
widening of it. The join against our own mapping, and the stance each row
outside the rule carries, belong to the reconciliation the spec puts beside
this -- this module only takes the evidence.

**Taken from the host, not from inside the container.** The driver is a
pip-installable wheel pinned in `scripts/pyproject.toml`; installing it into a
`postgres:` image would be a second build system this project does not have.
So `generate_fixtures.py` publishes the fixture container's port on the
loopback interface and the sweep connects to it like any other client.

**A probe is a cast, not a declared column.** The comparison oracle asks its
pairs through two typed columns because a column carries a *collation* and a
cast does not. Nothing here reads a collation: the driver maps a result field
by its type OID alone, and `SELECT NULL::<type>` produces exactly that OID. So
the cheap probe is the honest one, and it costs one round trip per type
instead of a temp table per type.

**`declared` is `format_type(oid, NULL)`, which is what `pg_dump` writes.**
`pg_dump` spells a column's type with `format_type(atttypid, atttypmod)`, so
the sweep's key is already the string our resolution reads out of a
`CREATE TABLE` -- `integer` rather than `int4`, `character varying` rather than
`varchar`. `typname` is kept beside it because that is the name the driver
reports inside an `arrow.opaque` type, and the two differ for a third of the
catalog.

**Every major, because the type set moves.** Multiranges arrive at 14, so a
row absent at 13 is not a row we are below.
"""

from __future__ import annotations

import time
from importlib import metadata
from pathlib import Path

import comparison_oracle

#: Directory name under `fixtures/<version>/`, beside `oracle/`. It is not a
#: `pg_dump` output and has no flag set, and the Rust test vocabulary's
#: `all_fixtures()` walks for `*.sql`, so it steps over this the same way.
FLOOR_DIRNAME = "adbc"
FLOOR_FILENAME = "floor.tsv"

#: The distribution whose version every row records. The pin itself lives in
#: `scripts/pyproject.toml` -- a value in two places is a value that drifts --
#: and asserting the two agree is the reconciliation's job, not the sweep's.
DRIVER_DIST = "adbc_driver_postgresql"

#: `floor.tsv`'s columns, in order.
#:
#: * `driver`    -- the `adbc_driver_postgresql` version the row was taken with.
#: * `server`    -- the PostgreSQL major.
#: * `declared`  -- `format_type(oid, NULL)`: how `pg_dump` spells the type.
#: * `typname`   -- the catalog name, which is what an `arrow.opaque` reports.
#: * `oid`       -- the catalog OID, which is what an unnamed opaque reports.
#: * `typtype`   -- `b`/`e`/`r`/`m`/`d`, so a reader can see which bucket a row
#:                  is in without re-querying.
#: * `status`    -- `ok`, or `E<sqlstate>` where the driver could not read the
#:                  type at all. The comparison oracle's own convention.
#: * `extension` -- the Arrow extension name (`arrow.opaque`, `arrow.json`),
#:                  or `\\N` where the field is a plain Arrow type.
#: * `arrow`     -- the Arrow type: the *storage* type where `extension` is
#:                  set, the field's own type otherwise, `\\N` on an `E` row.
#:
#: **`driver` and `server` are on every row rather than in a sibling
#: `meta.tsv`.** The driver version is the axis the floor is a claim about, so
#: a row that has been copied, diffed or grepped out of the file still says
#: which release it speaks for; and the reconciliation reads the pin off the
#: rows it is already joining rather than off a second file.
FLOOR_COLUMNS = [
    "driver",
    "server",
    "declared",
    "typname",
    "oid",
    "typtype",
    "status",
    "extension",
    "arrow",
]

#: Every `pg_type` row a user could declare a column of, and nothing else.
#:
#: `typtype` in `b`, `e`, `r`, `m`, `d` takes the base, enum, range,
#: multirange and domain types and leaves out composites (`c`, which are
#: every table's row type as well as the declarable ones) and pseudo-types
#: (`p`, which no column can be). `typisdefined` drops a shell type.
#:
#: **The array exclusion is a back-reference, not a shape test.** An array
#: type is exactly one that some other type names as its `typarray`; testing
#: the *shape* instead (`typelem <> 0 AND typlen = -1`) also catches
#: `int2vector` and `oidvector`, which are declarable types in their own right
#: that no array recursion covers. Arrays proper are left out because our own
#: resolution reaches them by recursion from the element type, so a row for
#: `integer[]` would be evidence about a mechanism this file is not measuring.
SWEEP_SQL = """
SELECT t.oid::bigint AS oid,
       t.typname AS typname,
       t.typtype AS typtype,
       format_type(t.oid, NULL) AS declared
  FROM pg_type t
  JOIN pg_namespace n ON n.oid = t.typnamespace
 WHERE n.nspname = 'pg_catalog'
   AND t.typisdefined
   AND t.typtype IN ('b', 'e', 'r', 'm', 'd')
   AND NOT EXISTS (SELECT 1 FROM pg_type e WHERE e.typarray = t.oid)
"""


def driver_version() -> str:
    """The installed `adbc_driver_postgresql` version, as recorded in every row."""
    return metadata.version(DRIVER_DIST)


def connect(uri: str, timeout: float = 30.0):
    """A DBAPI connection to `uri`, retried until the published port answers.

    `generate_fixtures.py` waits for `pg_isready` *inside* the container, which
    can succeed a moment before the host's published port is forwarding, so the
    first connection attempt is racy in a way nothing else in that script is.
    """
    import adbc_driver_postgresql.dbapi as dbapi

    deadline = time.monotonic() + timeout
    last: Exception | None = None
    while time.monotonic() < deadline:
        try:
            return dbapi.connect(uri)
        except Exception as exc:  # noqa: BLE001 -- retried, then re-raised below
            last = exc
            time.sleep(0.5)
    raise TimeoutError(f"could not connect to {uri} in {timeout}s") from last


def _field_cells(field) -> tuple[str | None, str | None]:
    """One Arrow field as `(extension, arrow)`.

    An extension field records its extension name and its *storage* type,
    because that is the pair D2's rule reads: `arrow.opaque` over `binary` is
    the driver's bottom, and `arrow.json` over `string` is a real answer.
    """
    ty = field.type
    name = getattr(ty, "extension_name", None)
    if name is None:
        return None, str(ty)
    return name, str(ty.storage_type)


def sweep(conn, version: str) -> list[list[str | None]]:
    """The whole floor for one server, as `FLOOR_COLUMNS` rows.

    One query per type rather than one projecting all of them: a type the
    server has no binary output function for aborts the transaction, and
    attributing that to the type is the whole point of the `status` column.
    Rows are sorted here rather than in SQL, so the file's order is code-point
    order on every server instead of the database collation's.
    """
    driver = driver_version()
    with conn.cursor() as cur:
        cur.execute(SWEEP_SQL)
        types = cur.fetch_arrow_table().to_pylist()

    rows: list[list[str | None]] = []
    for entry in sorted(types, key=lambda t: t["declared"]):
        declared = entry["declared"]
        try:
            with conn.cursor() as cur:
                cur.execute(f"SELECT NULL::{declared} AS v")
                field = cur.fetch_arrow_table().schema.field(0)
            status, (extension, arrow) = "ok", _field_cells(field)
        except Exception as exc:  # noqa: BLE001 -- recorded as the row's answer
            conn.rollback()
            sqlstate = getattr(exc, "sqlstate", None)
            status, extension, arrow = f"E{sqlstate or 'XXXXX'}", None, None
        rows.append(
            [
                driver,
                version,
                declared,
                entry["typname"],
                str(entry["oid"]),
                entry["typtype"],
                status,
                extension,
                arrow,
            ]
        )
    return rows


def floor_path(fixtures_dir: Path, version: str) -> Path:
    return fixtures_dir / version / FLOOR_DIRNAME / FLOOR_FILENAME


def write_floor(fixtures_dir: Path, version: str, rows: list[list[str | None]]) -> Path:
    path = floor_path(fixtures_dir, version)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(comparison_oracle.format_tsv(rows))
    return path


def read_floor(fixtures_dir: Path, version: str) -> list[dict[str, str | None]]:
    """A committed `floor.tsv` back as `FLOOR_COLUMNS`-keyed rows."""
    text = floor_path(fixtures_dir, version).read_text()
    return [
        dict(zip(FLOOR_COLUMNS, row, strict=True))
        for row in comparison_oracle.parse_tsv(text)
    ]
