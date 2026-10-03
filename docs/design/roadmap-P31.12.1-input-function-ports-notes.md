# P31.12.1 — Refusals told from shortfalls by each input function's grammar: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md);
the mechanism is 31.12's
([`roadmap-P31.12-field-refusal-notes.md`](roadmap-P31.12-field-refusal-notes.md)).
The row was split: the enum is 31.12.3's
([`../status/history/2026-10-03.md`](../status/history/2026-10-03.md),
"31.12.1 lands without the enum").

## What exists

- **The readers stay as narrow as they were; a classifier runs where one
  fails**, answering `Refused` or `Unparsed` by the server's grammar:
  `decode::bool_unread` (I65), `oid_unread` over `oid_in` (I66),
  `macaddr_unread` and `macaddr8_in` (I68), `bytea_unread` (I69). Widening a
  canonicalized kind's reader instead would grow `KD82`'s class: `=` compares
  a field's bytes with the literal's `*_out` spelling, so a field read in
  another spelling is missed by it. `macaddr`'s one bounds set serving
  DataFusion's order (D79) rests on its narrow reader too.
- **`inet` and `cidr` are read by `decode::network_in`, a port of
  `inet_net_pton.c`**, not classified: the Rust parse read `::1/08`, which the
  server refuses, so the old reader was past the ceiling. Both types compare
  `Decoded`, so reading more spellings costs `=` nothing. I40 no longer
  carries the `cidr` refusal; I67 does.
- **A `bytea` is checked at parse on its bytewise path**: `Canonical::of`
  failing calls `bytea_unread`, so a value of any length is checked, the
  `escape` decoder reading the whole text already.
- **`oid` is refused only where every major refuses it**: v16 moved `oidin`
  to `strtoul` base 0, so `08` is read before v16 and refused from it, and a
  C23 glibc reads `0b`. A text one reads is `Unparsed`. That `010` is 10 to
  v15 and 8 to v16 is `KD84`.
- **`CACHE_FORMAT_VERSION` is 48**: a network field in a spelling the Rust
  parse missed now keys, and one it read that the server refuses, `::1/08`,
  fails the parse. Neither pin moved.
- **Evidence**: `decode.rs`'s five `*_is_refused_only_where_*`/`*_is_read_as_*`
  tests, each over spellings cast on the koji replica (PG16), the `oid` one on
  a `postgres:15.19-trixie` container too;
  `predicate.rs`'s
  `a_field_postgresql_refuses_is_told_from_one_this_build_does_not_read` and
  `oracle::a_literal_of_a_kind_with_an_input_function_port_is_read_as_the_server_reads_it`
  (every major); `tests/statistics.rs`'s
  `a_parse_fails_where_an_input_function_refuses_and_not_where_it_reads`.

## Findings

- **`macaddr_in` is `sscanf`, so the classifier models `%x`** for the digits
  and blanks it takes and leaves a run opening with a sign or `0x` to glibc,
  never refusing it (`KD85`); an octet past eight digits is modelled as
  consumed and left unvalued, since glibc truncates it (`…:100000000ff` is
  `ff`).
- **`inet_net_pton.c` accumulates a netmask in a wrapping `int`**
  (`-fwrapv`), so `1.2.3.4/4294967304` is an `inet` `/8`; the port wraps as
  the server does.
- **`oidin` breaks I35's additivity** outside the oracle's cases: `08` and
  `010` change meaning at v16. I35's scope limit covers it; `KD84` holds the
  consequence.

## What the slices after this inherit

- **31.12.3** needs the preamble to know when an enum's labels are exact;
  a miss is `Refused` only then. The classifiers here need nothing from it.
- **31.14's `strict`** checks every field; the unrefused `macaddr` spellings
  above stay unrefused under it.
