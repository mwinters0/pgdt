# P15.1 notes — The gzip fixtures

The fixture half of the evidence slice; the figures are 15.1.1
([`../status/history/2026-10-21.md`](../status/history/2026-10-21.md), "15.1
split: the figures are 15.1.1").

- **`pg_dump`'s own `-Z` output is `fixtures/<major>/edge_cases/compress.sql.gz`**
  at every routine major, the `edge_cases` schema's `compress` set
  (`--compress=6`). `scripts/generate_fixtures.py` writes a set that
  compresses (`compresses`) as `.sql.gz` (`output_suffix`), refusing one
  whose output does not open with gzip's magic, so no `*.sql` sweep —
  `pgdt_fixtures::all_fixtures`, the emitter register's `dump_files` — reads
  it as a dump. `pgdt_fixtures::gzip_fixture` addresses one.
- **`--compress` is no longer exempt** from the emitter register: the set
  passes it, so the `Unsupported` exemption was stale and is struck
  (`scripts/emitter_register.py`). `pg-dump-compatibility.md`'s compression
  row still reads `Planned`; it names the fixture as what P15 will be read
  against.
- **`test_support` writes the synthetic payloads** with encoders not ours:
  `gzip_compress` (`flate2`, one member) and `bgzf_compress`
  (`noodles-bgzf`'s writer), over `gzip_test_payload`, many lines of
  `COPY`-like text. `pgdt-io` gains `flate2`, `noodles-bgzf` and
  `pgdt-fixtures` as dev-dependencies, the last without `mapping`, as
  `pgdt-codec`'s and `pgdt-mapping`'s are.
- **What the files are is pinned**, in `crates/pgdt-io/src/test_support/tests.rs`:
  the `-Z` fixture is one member at every major, its header's flags and
  `MTIME` 0, holding a plain dump whole; the synthetic single member has the
  same header; a BGZF file walks by its headers alone — the `BC` subfield at
  bytes 12–17, each member `BSIZE + 1` long, `ISIZE` at most 64 KiB and
  equal to the member's plaintext, an empty 28-byte member last.

## Negative results

- **A regenerated `compress.sql.gz` cannot be compared with the committed
  `default.sql`**: `\restrict`'s key and the `now()` column move on every
  run (D69), so the two are equal only from one run, and regenerating the
  whole `edge_cases` schema to get one moves 84 files, including byte
  lengths at 13 and 14, where a timestamp's trailing zero is dropped. Only
  the new `.sql.gz` files were kept from the run; the test checks the dump's
  framing and its major instead.
- **Every routine major writes the same header**, `1f 8b 08 00 00000000 00
  03`: no `FNAME`, `MTIME` 0, `XFL` 0, OS Unix.

## What the next round inherits

- **15.1.1 is the figures**: the serial inflate legs (zlib-ng, zlib-rs,
  `miniz_oxide`) against the plain parse on each measured device, and the
  candidate spacings, as an instrument landed in one commit and read from it
  (`measurements.md`, "A figure may be published outside the sweep"). A large
  gzip input for them is generated, never committed.
- **Nothing reads the fixtures through a source yet.** 15.5's parity and
  determinism tests are where they are first read; `pg_dump`'s single-member
  shape becomes an `I<n>` there, as the spec's "Facts the grilling found"
  says.
