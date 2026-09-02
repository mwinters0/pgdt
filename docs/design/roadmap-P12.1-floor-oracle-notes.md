# P12.1 — The floor oracle

What 12.2's reconciliation inherits, and the two things the sweep found that
the spec did not have.

The mechanism itself is filed by subject:
[`architecture.md`](architecture.md), "The ADBC floor oracle". This doc holds
only what the next slices need and what is not recoverable from the code.

## Module map

| File | What it is |
|---|---|
| `scripts/adbc_floor.py` | the sweep, the file format, `read_floor` |
| `scripts/generate_fixtures.py` | the third pass (`take_floor`), `HOST_PORT`, `--skip-floor` |
| `scripts/test_adbc_floor.py` | the committed files' own properties, no container |
| `scripts/pyproject.toml` | `adbc-driver-postgresql==1.12.0`, `pyarrow>=21` |
| `fixtures/<13–18>/adbc/floor.tsv` | 74 rows at 13, 82 at 14–18 |

`read_floor(FIXTURES_DIR, version)` hands back `FLOOR_COLUMNS`-keyed dicts and
is what 12.2 joins against `builtin_scalar`. `driver_version()` is the
installed version; **12.2 owns the assertion that it equals the pyproject
pin** (D8) — 12.1 records it and asserts nothing about it, so the pin check
lands with the reconciliation that D8 puts it in.

## The sweep's array exclusion is a back-reference

An array type is excluded by *back-reference* — `NOT EXISTS (SELECT 1 FROM
pg_type e WHERE e.typarray = t.oid)`, which is D7's spelling. That is exactly
"the array types", and it is what D7's reason names: our resolution reaches
them by recursing from the element type, so a row for `integer[]` would be
evidence about a mechanism this file does not measure.

**`int2vector` and `oidvector` are therefore in the file**, and neither is an
array type: no type names either as its `typarray`, and no array recursion
reaches them, since that recursion is a spelling test over the declared name
(I28) and neither name carries an array spelling. `int2vector` is the row D5
commits this phase to closing.

Consequence for 12.2: **`oidvector` costs nothing.** It comes back
`arrow.opaque` over `binary` on every major, and D2's rule holds only where
ADBC yields a *non-opaque* type — so the `extension` column places it, along
with the whole opaque tail, with no per-type line. What the columns cannot say
is what stays a hand-written exemption: `money` (D2's third bullet), the `reg*`
family (D2's second), and D10's two waiting rows naming 12.3 and 12.6. The two
`E42883` refusals are a second column-placed class.

## What the driver actually answers

Every claim D1's table makes about release 24 is now in the file rather than
argued. `json` **and** `jsonb` are `arrow.json`; `uuid` is *not*
`FixedSizeBinary(16)` but `arrow.opaque` over `binary`; no field carries
`POSTGRESQL:type` metadata, only `ADBC:postgresql:typname` on the opaque ones,
which the file does not record because `typname` is already a column.

Four rows 12.2 will have to look at first:

- **`regproc` → `int32`.** It is the *only* `reg*` type the driver gives a real
  Arrow type to; `regclass`, `regtype`, `regrole` and the rest are all
  `arrow.opaque`. D2's second bullet is written over "the `reg*` family", and
  the family turns out to have exactly one member the rule has to answer for.
- **`money` → `int64`**, as D2's third bullet expects. This is the `KD13` row.
- **`interval` → `month_day_nano_interval`** and **`int2vector` →
  `list<item: int16>`**: D10's two waiting exemptions, both confirmed as real
  floor rows rather than opaque ones.
- **`numeric` → `arrow.opaque` over `string`.** Not a floor row at all, as D2
  says — worth knowing because it is the one opaque whose storage is `string`
  rather than `binary`, so a check keying on "storage is binary" would misread
  it.

## Two types the driver cannot read at all

`aclitem` and `gtsvector` answer `E42883` — *no binary output function
available* — on all six majors. Binary is the only encoding the driver reads,
so its answer for them is nothing at all. They are recorded rather than
omitted: a type missing from the file cannot be told from a type the sweep
never asked about, and 12.2's "every floor row resolves to an arm" direction
needs to know these are refusals rather than absences.

## The container gained a published port

`start_container` now publishes `127.0.0.1:55432:5432`. The driver is a pip
wheel in `scripts/`'s own `uv` environment (D7: taken from the host), and the
container is the only thing to point it at. Versions run one at a time, so one
fixed port serves all six; 5432 and 5433 are taken by unrelated long-lived
containers on this machine.

`wait_ready` polls `pg_isready` *inside* the container, which can succeed a
moment before the host's forward is up — so `adbc_floor.connect` retries for
30 s. That race is new with this slice and is the only reason the helper
exists.

## The pass is cheap, and it is independent of every fixture schema

The whole six-major floor sweep is **30 s**, against the ~20 min a full
fixture regeneration takes. It is a `pg_catalog` question, so it runs against
the `postgres` database immediately after `wait_ready` and owes no
`create_fixture_db` / `drop_fixture_db` pair — which is why
`--skip-dumps --skip-oracle` is now a meaningful invocation rather than an
error.

No measurement figure declares `scripts/` or `fixtures/`, so
`uv run measure.py --stale` is unchanged by this slice: all thirteen figures
were already red and none moved.
