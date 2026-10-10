# P35.10.2 notes — each API namespace its file's own module

What 35.5 inherits from laying the five namespaces out as the spec names it
([`roadmap-P35-seams.md`](roadmap-P35-seams.md), "What this phase does not
do"), over 35.10.1's sort. Each crate's `lib.rs` and each namespace's file is
where it lands. No body changed and no test moved but with what it tests;
`CACHE_FORMAT_VERSION` stands.

## How it fell

- **Each namespace is `pub mod <name>;`, its file's own module**, and what it
  holds that `internal` re-exports sits in child modules beside it, cut at the
  file's seams:
  - `pgdt_core::instrument`: `hooks` — the macros and every function and guard
    the library calls to record its own work.
  - `pgdt_dump::cache`: `file` — the header, the encoded file, and reading and
    writing it — and `identity`, what a cache records of its source and how
    that compares.
  - `pgdt_codec::decode`: `refusal` — the refusal checks and the storage
    format's bounds — `float`, `stored` — a `time`'s and a timestamp's value
    as PostgreSQL stores it — and `digits`, the decimal digit writers whole,
    `push_integer` being their one face a crate above calls.
  - `pgdt_codec::pgtype`: `strict`, `divergence` and `discrete`.
  - `pgdt_codec::key`: `order`, everything but `ValueKey`.
- **What moved is what `internal` re-exports**, with the private helpers only
  it calls. A `pub(crate)` item a sibling module calls stays in the
  namespace's file: nothing outside the crate reaches it, and the slice row
  moves what is re-exported by `internal`.
- **A child is `pub(crate) mod`, never `mod`.** `internal` sits in `lib.rs`,
  outside the namespace, so it cannot name an item of a module private to
  the namespace, and a `pub(crate) use` there cannot be re-exported `pub`
  (E0364). `pub(crate)` is private to every embedder, and the only path to a
  child's item from outside the crate is `internal`'s.
- **`decode` re-exports its children's items a sibling calls, `pub(crate)`**,
  so `key`'s `decode::<item>` calls read as they did; `key::order` names
  `pgtype::discrete::DiscreteExt` by its path. A test module reaching a child
  item names it by path, as `pgtype/tests.rs` and `decode/differential.rs` do.
- **`key`'s tests moved with what they test**, to `key/order/tests.rs`: they
  reach `order`'s private items, which a sibling of `order` cannot.
- **What a parent calls of its child went `pub(super)`**: `SourceIdentity`'s
  seven private methods, `key::order::field_key` and the two `heap_bytes`
  `ValueKey` calls, and `digits`' `push_two`, `push_year` and `push_fraction`.
- **No library file loads by `#[path]`**, test modules included: each file's
  children resolve beside it by convention. `tests/layering.rs` reads
  `pub(crate) mod` as a production module.

## What 35.5 inherits

- **35.5's checks hold of this tree**: no `#[path]` under `crates/*/src`, no
  namespaced item at its crate's root, and every library module but the
  namespaces and `internal` either `mod` or `pub(crate) mod` — which "every
  library module private" has to read as not `pub`.
- **`measure.py` names each new file in its mechanism**: `DECODE`, `CACHE`
  and `PREDICATE` list theirs, and `PGTYPE` is the register's four files,
  which `STATISTICS` and the koji section declare.
- **`decode.rs`, `key/order.rs` and `pgtype.rs` are still past a limit**
  (`file_limits.py`), for 35.6.
