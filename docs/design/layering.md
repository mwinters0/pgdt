# Module layering

A standing architectural constraint, not a phase. Every module in
`pgdump_query` belongs to exactly one of four layers, and dependencies point
downward only. Read this before adding a module, moving code between modules,
or wiring a new concern across existing ones.

The layers are modules today, in one crate. They are drawn where crate
boundaries would eventually go, so that splitting them out later is a
mechanical extraction rather than a redesign. Nothing here depends on that
split happening.

## The layers

| Layer | Concern | Modules |
|---|---|---|
| **L1 — Bytes and structure** | Byte-range I/O, line and `COPY` block structure, COPY TEXT field splitting/escaping/unescaping, DDL text grammar, the on-disk cache format, the full file map, the file-level diagnostic vocabulary | `io.rs`, `copy.rs`, `scan.rs`, `index.rs`, `cache.rs`, `preamble.rs`, `map.rs`, `diagnostic.rs` |
| **L2 — PostgreSQL semantics** | Declared type string → Arrow `DataType`; domain/enum resolution; joining a `COPY` header against `DumpMetadata`; per-type field decode and render-back, scalar and nested; how two values of a column compare, and whether that is PostgreSQL's order | `pgtype.rs`, `resolve.rs`, `decode.rs`, `nested.rs` |
| **L3 — Arrow assembly** | Building Arrow arrays and `RecordBatch`es, including the zero-copy `Utf8View` path into the reader's buffers and the retention of the read chunks those views point into | `batch.rs` |
| **L4 — Query and planning** | Which blocks to read, cache segment planning, resume, predicate application, the streaming API | `stream.rs`, `predicate.rs` |

`error.rs` and `lib.rs` are cross-cutting and belong to no layer.

Which concern lives in which module, and why: [`architecture.md`](architecture.md),
"Module map".

### What each layer must not know

- **L1** does not know Arrow, PostgreSQL type *semantics*, or that queries
  exist. It parses a declared type as an opaque string and stores it as one;
  it never interprets it.
- **L2** does not know files, byte offsets, buffers, the cache, or async. It is
  pure functions over data L1 already produced.
- **L3** does not know queries, predicates, or the cache.
- **L4** is the top; the CLI and future embedder crates sit above it.

## Rules

1. **`use` goes downward or sideways, never upward.** An L1 module may not
   name an L2/L3/L4 type. Within a layer, modules may depend on each other.

2. **L1 is Arrow-free.** No `use arrow` anywhere in an L1 module. This is the
   layering's load-bearing property: it is what lets a caller who wants only
   dump metadata avoid compiling Arrow at all, and it is machine-checkable.

3. **L2 uses `arrow::datatypes` only** — `DataType`, `Field`, `Schema`. Never
   `arrow::array`, never `arrow::buffer`. L2 names types; it does not build
   arrays.

4. **L2 is synchronous and does no I/O.** No `async fn`, no `ByteRangeSource`,
   no `std::fs`. If a function in L2 needs to read something, the layering is
   wrong, not the function.

5. **Anything persisted to the cache is expressible in L1's vocabulary.**
   The cache format is L1's, so it cannot hold an L2 conclusion. Stated positively: "store what
   the dump said, never what we concluded" — declared type strings, not resolved
   Arrow types. That is rule 5 applied to `DumpMetadata`; it applies identically
   to everything the cache grows later.

6. **A trait crossing a layer boundary is defined in the lower layer and
   implemented in the higher one.** `ByteRangeSource` is the existing instance:
   L1 defines it, callers supply the implementation. This is how a higher layer
   injects behaviour into a lower one without inverting the dependency.

## Checks

Rules 2 and 3 are greps. Both must produce no output:

```sh
# L1 is Arrow-free
rg -n '^use arrow' pgdump_query/src/{io,copy,scan,index,cache,preamble,map,diagnostic}.rs

# L2 names Arrow types but does not build arrays
rg -n '^use arrow::(array|buffer)' pgdump_query/src/{pgtype,resolve,decode,nested}.rs
```

Rule 1 is read off the import lists:

```sh
rg -n '^use crate::' pgdump_query/src/*.rs
```

**That check is only as complete as the import lists are**, so a module names
every crate-internal dependency it has in one — never `crate::other::thing`
inline at the use site. An inline path is invisible to the grep, so a module
that uses them depends on more modules than the check reports. The
counter-check reads production code only (it stops at the first
`#[cfg(test)]`, since a test module's imports are not the module's
dependencies) and must also produce no output:

```sh
for f in pgdump_query/src/*.rs; do
  awk '/^#\[cfg\(test\)\]/{exit}
       /crate::/ && !/^(use |pub use )/ && !/^[[:space:]]*\/\// {print FILENAME": "FNR": "$0}' "$f"
done
```

This applies to an upward reference too. `batch.rs` naming `crate::stream` is
the recorded deviation below, and it belongs in the import list precisely so
the check reports it rather than hiding it at the use site.

## Where the layering comes under pressure

These are the four cases where the boundary is not obvious. Each has a
decision rule; follow it rather than re-deriving one.

**Zero-copy `Utf8View` is why L3 is its own layer.** A field with no escapes is
appended as a view into the Arrow `Buffer` wrapping the chunk L1 read, not
copied. That marries array construction to the reader's buffers, and it is why
"parse values into Arrow" is not a single self-contained concern that could sit
beside `pgtype.rs`. The rule that keeps L2 clean: **a decode function takes a
`&[u8]` field and returns a value; it never takes an Arrow builder.** If a type
genuinely needs to write into a builder, that code is L3.

**Evaluating a predicate inside the scan would invert control, not
dependency.** Nothing does this today — the replay loop tests the predicate
after the scanner yields a row — and it is worth stating anyway, because it is
the shape any future move of that evaluation must take: L1 defines the callback
or trait and L4 supplies the implementation, per rule 6. `predicate.rs` stays
in L4 wherever it comes to run.

**Per-row-group statistics (P10) span all four layers**, which makes them
the sharpest test of these rules: gathered during L1's scan, requiring L2 to
parse a value, persisted in L1's cache. Rule 5 settles the persistence
question — `RowGroupStats` records the **declared PostgreSQL type** a statistic
was computed as, not an Arrow `DataType`, for the same reason `DumpMetadata`
stores declared type strings. Rule 6 settles the compute question — the parse
function is injected downward, not imported upward.

**Archive containers (P8), `object_store` (P14) and the decompressing sources
are L1-only.** All are additions *inside* L1: `object_store` and a decompressing
source as further `ByteRangeSource` implementations, the container layer between
`io.rs` and `scan.rs`. None belongs above L1, and none may introduce an Arrow
dependency into it. The `.xz` source is the built instance and holds to it —
`XzSource` lives in `io.rs` beside `LocalFileSource` and nothing above L1 knows
a source decodes ([`architecture.md`](architecture.md), "The compressed
source"); the gzip source (P15) and the zstd one (P18) join it there.

The two byte sources compose in one direction only — a decompressing source
wraps whatever supplies its compressed bytes, local or remote — so neither
knows about the other, and a seek table is L1 data like the rest of the cache
(rule 5).

**A source advises its own partitioning, which is what keeps decode scheduling
out of L4.** `ByteRangeSource::partitions` answers where this source would like
a range split and what one partition costs it resident; a scheduler above asks,
runs the partitions and never names a source type
([`architecture.md`](architecture.md), "Execution model and API surface"). The
alternative — L4 calling `xz-seek`'s block pieces directly — would put block
scheduling in the query layer and make it name the compressed source, which is
rule 1 read backwards.

## Known deviations

One piece of code sits outside its layer. It is recorded so that nobody treats
it as precedent, and nobody "fixes" it opportunistically — move it only as part
of work that reworks the module anyway.

- `batch.rs` (L3) exposes `read_table`, a public push-mode entry point. That is
  L4 work.

**The chunk-retention deque and the `Bytes` → `arrow::Buffer` conversion are
not on this list**: they are `batch::RetainedChunks`, in L3
([`architecture.md`](architecture.md), "Arrow assembly and the zero-copy
path"). They are named here because a deviation that has been recorded for
several phases is one a reader learns to expect, and because they are what the
"work that reworks the module anyway" clause above looks like when it fires —
the retained unit is being re-derived against decoded blocks rather than read
chunks, and the move rode in on that rather than standing as a tidying pass.

## Adding a module

Assign its layer before writing it, and record the assignment in the table
above in the same change. A module that seems to belong to two layers is
two modules.

Reasoning behind these boundaries:
[`../status/history/2026-08-22.md`](../status/history/2026-08-22.md).
