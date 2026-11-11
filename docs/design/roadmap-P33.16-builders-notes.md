# P33.16 notes — a nested value decodes into its builders

**A query's batch reads an array, record, range, multirange or `int2vector`
field as the output grammars' walk reads it, each part appended to its builder
as it is handed over** ([`decisions.md`](decisions.md), "D153"). No resident
reading was taken; what proves it is `crates/pgdt-query/tests/decoded_copies.rs`'s
`a_nested_value_decodes_into_its_builders` — a `bigint[]` of single digits
written in place and a `text[]` whose every element `COPY` writes with an
escape, each of seven or eight MiB — which holds each under its builders'
Arrow form and fails on the parent, whose staged elements and unescaped field
hold several times it.

- **`append_walked`** (`pgdt-query`'s `batch/builder.rs`): `walk_array`,
  `walk_record`, `walk_range`, `walk_multirange` with `walk_bracketed`, and
  `walk_int2vector` feeding their builders; an array's elements land in its
  innermost list's child (`array_leaf`), and each dimension's lists are closed
  once the walk returns its shape (`close_dimensions`), the depth and the
  lower bounds refused then. `append_typed` walks a nested text in place, so a
  nested part inside a container is walked as its own text.
- **`push_field`** reads a nested field by `RawRow::read`: one holding no
  escape is walked in place, an escaped one off its bytes once
  `escaped_utf8_error` has raised the row's own error over a text that is not
  UTF-8, its refusal quoting the field's head (I94).
- **`escaped_utf8_error`** (`pgdt-codec`'s `value/text.rs`): the UTF-8 check
  `predicate/eval.rs` made of an escaped field off its runs, moved beside
  `TextFold` and read by both.
- **The walks and their shapes** (`nested/output/walk.rs`) are `internal`.

## Its count pair, placed and accepted

**`02cdaa3d` against the 33.16 tree resolved a rise on the three query rows**,
`query --schema-mode strings` and `typed` and `pgdt sql -c`, and nothing on
either parse, which builds no batch (`runs/count-pair-33.16-20261110/log`).
callgrind reproduces each row on both rep pairs, the two agreeing function for
function, and its frames sum to each row's median change
(`runs/count-pair-33.16-20261110/callgrind/reading.txt`, the account
instruction by instruction; the legs kept in
`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.16/legs/`).

- **The change's own cost is `push_field`'s `is_walked` test**, a few
  instructions on every field's path, a strings query's included, all inlined
  into `RowBatcher::push_row`.
- **The rest of `push_row`'s rise on a typed row is register allocation of code
  33.16 left alone**, the largest part `push_hex`'s loop inlined from
  `decode_bytea_into`, two register moves longer a byte pair: identical code
  run as often is layout, and a rework resting on codegen is refused
  ([`decisions.md`](decisions.md), "D108"). So are `memrchr`'s fall under
  stdout's line writer and `ResolvedTerm::eval`'s rise on `pgdt sql`'s
  `IS NOT NULL`, which reaches no changed code. `append_typed` falls, its
  nested arms gone into `append_walked`.
- **Accepted**: the test is the dispatch D153 needs ahead of `RawRow::decode`.

## What 33.16.1 inherits

- **A nested column's resident form is its builders'**: the leaf's Arrow
  values, a list's `i32` offset per list and a validity entry per row, all
  grown by doubling as Arrow's builders grow, so at the last doubling a
  builder holds its old and new buffers both. Whether a decode factor carries
  that growth is the factor's to state.
- **A nested column under a tier test is not walked** (D153): it is unescaped
  and decoded through its elements, `UnrepresentableRead::cannot_hold` reading
  its whole text, so its factor is the staged form's, an escaped field held
  twice beside its line.

## Negative results

- **No builder truncates a refused field's parts**, which the spec's "What a
  held line costs" states: every `Err` from `append_typed` and
  `append_walked` ends its query or is read off a builder made for that one
  value, so no reader meets them (D153).
- **The count pair prices only `push_field`'s dispatch**: its input, the
  brace-free control, holds no nested column, so the walk's speed against the
  staged elements on a nested column is read by no instrument here; 33.16.3
  prices it.
