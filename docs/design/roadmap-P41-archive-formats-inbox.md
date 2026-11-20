# P41 inbox — facts filed for its grilling

Evidence found in earlier phases that P41 (archive formats: the shared model,
and `--format=custom`) will need. **This is a queue, not a document**: when
P41 is grilled, walk every entry, fold it into the spec or discard it as stale,
and delete this file. See `docs/process.md`, "Inboxes: facts filed by
destination".

Each entry says what the fact is, why *this* phase cares, and where it came
from — go re-check the origin rather than trusting an entry that has aged.

---

## The partition-root marker lives in the TOC entry, not in the SQL

**Fact.** I2: when load-via-partition-root applies, a `TABLE DATA` entry's
`COPY` header names the partition's **root** table, so one header name owns
several blocks in a single dump — and `pg_dump` forces the mode by itself for
a table hash-partitioned on an enum column, so a flagless dump produces it.
Plain-format output is disambiguated by a `-- load via partition root <root>`
line, which `dumpTableData()` writes into the entry's **`defn`** field;
`_printTocEntry()` is what renders `defn` as a comment in plain format.

**Why P41 cares.** It reads the archive container (custom, and the model
directory and tar share) rather than plain text. The marker is not part of the
SQL — it is archive metadata that plain format happens to render as a comment —
so a container reader has to get it from the TOC entry directly, and
`crate::map`'s lexical `partition_root_marker()` will find nothing there.
Whether the entry's `defn` is even preserved in the archive TOC the same way is
**unverified**; that is a source-reading job for this phase, not an assumption
to inherit.

**Origin.** 2026-08-24. See I2 in
[`postgres-invariants.md`](postgres-invariants.md) — which carries the
reproduction and the exact source functions — and the
`--load-via-partition-root` row in
[`pg-dump-compatibility.md`](pg-dump-compatibility.md).

**Contingent on.** Nothing in `pgdt`; this is upstream behaviour with a
re-verification step recorded on I2 itself.

---

## Entry order is the archive's order, and it is not grouped by table

**Fact.** `DOTypeNameCompare()` in `pg_dump_sort.c` orders entries by (type
priority, namespace name, **object name**), and a `TABLE DATA` entry's object
name is the *partition's* own name, not the root's. So several blocks sharing
one `COPY` header name are **not** adjacent — an unrelated table whose name
sorts between two partition names is emitted between their blocks. Observed,
flagless, 16.15.

**Why P41 cares.** This is a property of the *archive's entry ordering*,
not of plain-text rendering, so it carries over to every container format
the shared model reads. Any design that enumerates a table's data by walking
entries until the name changes is wrong for the same reason the plain-format
version was — and a container's TOC makes "just index every entry up front"
cheap, which is the better answer there and worth reaching for deliberately
rather than rediscovering the trap.

**Origin.** 2026-08-24. See I2 in
[`postgres-invariants.md`](postgres-invariants.md).

---

## A custom-format entry is zlib-framed, not gzip

**Fact.** The custom format writes an entry through `pg_dump`'s compressor API,
`AllocateCompressor` (`pg_backup_custom.c`), whose gzip arm calls
`deflateInit` (`compress_gzip.c:77`, 16.15). That is a zlib stream: a 2-byte
header and an Adler-32 trailer. The directory format's file API calls `gzopen`
and writes RFC 1952 members. `pgdt-gzip-seek` reads RFC 1952 members and
checks CRC-32 throughout. An entry's data ends with its own `\.` line
(`pg_dump.c`), so `CopyScanner` reads an entry's bytes unchanged.

**Why P41 cares.** "Gzip's decoder exists" holds for the inflater, not for
the framing or the checks, so the per-entry decompression this phase designs
needs a zlib arm. P42's directory entries are gzip files.

**Origin.** An architecture review at `ec20c8fd`, 2026-11-20, checked against
`/mnt/wd12t/upstream/postgres` at `release-v16.15`.

---

## One linear plaintext address space per source, assumed past the cache

**Fact.** `read_range(offset)`, a source's one `extent()`,
`PartitionBoundaries::At(Vec<u64>)` and the ledger's ordering of lines by file
position (`ledger.rs`) all assume each source has one linear plaintext address
space. `dump_metadata_from_spans` learns a dump's database and versions only
from `Connect` and `VersionHeader` spans, where an archive states both in its
header. `ByteRangeSource` has many defaulted methods, and the two wrappers
forward them by hand, so a forward that is forgotten compiles and takes the
default. No forward is missing today.

**Why P41 cares.** The sketch names entry-relative addressing for the cache
and `ResumeToken`, but not partition advice or the ledger's file order. Both
carry the same assumption. A TOC-built index either synthesizes the two span
kinds or gives the metadata a second way in. The archive layer adds wrappers,
for which a forwarding macro or a test enumerating the trait's methods is
cheap insurance.

**Origin.** An architecture review at `ec20c8fd`, 2026-11-20. *Contingent on*
`crates/pgdt-io/src/source.rs` and `crates/pgdt-dump/src/ddl/metadata.rs`.

---

## Core's errors name a block by a plain-format header offset

**Fact.** Eight `pgdt_core::Error` variants carry a block as `header_offset:
u64` (`UnterminatedCopyBlock`, `ColumnCountMismatch`, `CachedBlockChanged`, the
three predicate-column refusals, `UnknownProjectionColumn`, and
`TableColumnsDisagree`, which carries a second offset as well), and
`PartitionThroughRoot` and `PartitionsThroughRoot` each carry a
`header_offsets: Vec<u64>`. Each is a display site, which prints the block's
address (D167). Core can name no model type (D68, D74), so it cannot hold the
`BlockAddress` that P43 places in `pgdt-model`.

**Why P41 cares.** An archive block's address is an entry in the TOC, not a
header offset, so each of these errors needs an address core can print. The
options to grill are a display type core owns, an address rendered to a string
by the raiser, or the variants leaving core.

**Origin.** The review of 43.3's `CachedBlockChanged` call, 2026-11-20.
*Contingent on* `crates/pgdt-core/src/error.rs`.
