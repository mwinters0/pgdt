# P31.3 — The value oracle: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"The value oracle". No product code changed.

## What exists

- **`scripts/value_oracle.py`**: the catalog query, the walk building one
  `SELECT` per node of every column of every `public` table, the reconciliation,
  and their rules in its docstring, asserted in `scripts/test_value_oracle.py`.
- **The pass** is `generate_fixtures.py`'s `write_values`, in the comparison
  oracle's database after its scripts, writing
  `fixtures/<major>/oracle/values.tsv` at all six majors. `--skip-values`
  leaves it out; `--skip-dumps --skip-oracle --skip-floor` runs it alone, and
  it ends with the reconciliation, as the other oracle passes do. It is not in
  `comparison_oracle.SCRIPTS`, so the cross-major differ does not read it. A
  second run writes the same bytes.
- **`pgdump_query/tests/value_oracle.rs`** reads every column of every table of
  every `types` flag set with DDL — all but `data-only`, the test failing on a
  flag set it does not name — typed and projected alone, and holds each node
  whose Arrow type is not text to its row. Every flag set must compare the
  columns `default` does.
- **The fifth reconciliation** is `value_oracle.problems`, over
  `floor_mapping.typed_arms`: every typed arm has a non-NULL reading at every
  major. Run by `test_value_oracle`'s `CommittedTree`, so by `mise run check`.

## Rules the spec did not state, and why

- **A node is a container too.** An array's row is its dimension lengths
  (`2x2`, `0` when empty), a multirange's and an `int2vector`'s its count, a
  composite's and a range's `()`, each `\N` when NULL; so a NULL container and
  an empty one differ, and a composite with every field NULL is not NULL
  (`num_nulls`, not `IS NULL`, which is true of one).
- **A multi-dimensional array is read flat**, as `unnest` reads it, `[k]` the
  flat ordinal; the test flattens a nested `Array` plan the same way, which
  `NestedPlan` makes unambiguous (a nested `Array` is only ever a dimension).
- **An array of composites is read by subscript**, one dimension deep: a
  function returning a composite expands into its columns in `FROM`, so
  `unnest … WITH ORDINALITY AS u(v, i)` renames the composite's fields rather
  than naming its value. No fixture holds a multi-dimensional one; one would
  show as typed nodes the oracle does not read.
- **Integers, `oid` and `boolean` are read by their send functions too**, in
  hex: their decimal is their output spelling. A special — an infinity,
  `NaN` — has no number and is its own text.
- **A leaf no reading names** (`text`, an enum, `json`, `inet`) gives no row,
  and the test skips any node whose Arrow type is text, and anything beneath
  one — `t_nested_array.v_arr_holder`'s `intarr[]` field, refused as text,
  has element rows nothing meets.
- **What Arrow's format spec cannot hold reads NULL, the whole value when
  nested** (D96): the test converts each reading to the node's Arrow type —
  Julian day to `Date32`, 1970's microseconds into `i64`, `Time64` below
  `24:00:00`, the interval's microseconds into nanoseconds, a decimal within
  its precision — and a value holding a leaf that does not convert is
  expected NULL with nothing beneath it. The engine tier is the library's to
  hold, and holds.
- **`extra-float-digits-0`'s floats agree within one unit of the text's last
  digit**, 15 for a double and 6 for a real, every other flag set's bit for
  bit. `DBL_MIN` written `2.2250738585072e-308` reads as the subnormal
  nearest it, within that.
- **A generated column is not read**: `COPY` omits it.

## Exclusions, each asserted still to fail

`value_oracle.rs`'s `EXCLUSIONS`, a strict table as `known_failures.rs` is,
listed in `test_deficiencies.py`'s `KnownFailureTables`: `KD2`
(`t_composite_matrix.v_tagged`, every flag set), `KD67` (both `bytea`
columns, `bytea-output-escape`), `KD71` (`v_box_domain_array`, text under
`quote-all-identifiers` and typed elsewhere) and `KD72`
(`t_extremes.v_double`, `extra-float-digits-0`). Slices 31.8, 31.9 and 31.10
delete their rows here as well as in `known_failures.rs`.

## Findings

**None new.** Every other node of every column, at every major and flag set,
is the value the server reads, Monrovia's offsets and the v18 sidecar's table
included.

## What 31.4 inherits

- A column a `types` schema addition declares is read by the next values pass
  with no change to the script; a type outside `READINGS` contributes
  containers only, and the reconciliation is what fails when a new typed arm
  has no case.
- The oracle reads the `types` schema alone; schemas 31.4 adds elsewhere are
  not read by it.
