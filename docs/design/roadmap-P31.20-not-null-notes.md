# P31.20 — A NULL in a `NOT NULL` column refused: notes

What the slices after this one inherit. Why `NOT NULL` binds `default` too is
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "A
clean `strict` parse promises what the field and its declaration decide"; the
shape it follows is 31.19's
([`roadmap-P31.19-char-length-notes.md`](roadmap-P31.19-char-length-notes.md)).
The forms and their evidence are I76.

## What exists

- **The preamble reads every `NOT NULL` a dump declares before its data**:
  `ColumnDef::not_null` (the column's own, named or not, v18's `NO INHERIT`,
  and an inline `PRIMARY KEY`, identity or `serial`), `TableDef::not_null`
  (v18's table-level element, a `PRIMARY KEY (…)` list, a typed table's
  column option, and the `ALTER TABLE ONLY … ALTER COLUMN … SET NOT NULL`
  before v18, a new `TableReference::NotNull`), and `TypeKind::Domain`'s
  `not_null`. All three are read off `preamble::top_level_words`, so a
  `CHECK`, a generated expression and a literal spelling the words declare
  nothing.
- **`DatabaseMetadata::column_not_null`** answers for a table, walking its
  parents as `declared_column` does and passing on every `NOT NULL` but a `NO
  INHERIT` one; **`pgtype::domain_not_null`** answers for a declared type,
  walking domains over domains. `resolve_columns` joins them into
  **`ResolvedSchema::not_null`**, whatever the type resolved to (an unknown
  type's column included) and all `false` under the strings schema mode.
- **Where a NULL is refused**, but under `ignore`: the typed read
  (`RowBatcher::push_field`), every filter term naming the column on the row
  path, any operator (`ResolvedTerm`'s and `ResolvedMembership`'s
  `not_null`), a data-level parse keying the column
  (`ColumnGatherer::observe`) and a strict parse (`check_row`, a column
  checked for its `NOT NULL` alone where it reads no value). Each raises
  **`Error::NullRefused`**, with `COPY`'s line where a pass numbers it and the
  offset alone from a query; `FieldRefusal::value` is `None` for it, so an
  ignoring parse records it and a later default parse fails with it.
- **The Arrow field stays nullable** (D37): `ignore` reads the NULL.
- `pgdt info --detail` lists a domain's `NOT NULL`; `--map` labels the `SET
  NOT NULL` span. **`CACHE_FORMAT_VERSION` is 57**: the persisted metadata
  and spans changed shape, and `PERSISTED_INDEX` moved with it.
- **Evidence**: `preamble.rs`'s
  `every_form_of_not_null_is_read_and_nothing_else` and
  `a_not_null_is_found_on_the_table_and_through_its_ancestors`; `pgtype.rs`'s
  `a_domain_not_null_binds_the_domains_over_it`; `tests/preamble.rs`'s
  `inherited_and_typed_columns_are_declared_through_their_references`, which
  holds `ResolvedSchema::not_null` to the server's at every major, default and
  `--binary-upgrade`; `tests/decode.rs`'s
  `a_null_in_a_not_null_column_is_refused_wherever_it_is_read` (ten
  declarations, every reading surface, `ignore`, the strings mode and the
  recorded refusal; three that bind nothing).

## Negative results and limits

- **A `NOT NULL` domain beneath an array or a composite is not held**
  (`KD97`, 31.26): the flag is the column's, and `NestedPlan` carries none.
- **A filter term refuses a NULL on the row path only**: a group pruned by
  its statistics, or answered by them, reads no row and refuses nothing, as a
  pruned group's field past a `varchar(n)` is not refused.
- **A `NOT NULL` added after the data binds nothing here**, as it binds no
  `COPY`: v18's `NOT VALID` constraint and a post-data primary key.
- **Neither a `CHECK` nor a statement the grammar skips is followed**: an
  `ALTER TABLE` with several subcommands, `ALTER DOMAIN … SET NOT NULL`; no
  `pg_dump` writes either before the data.

## What the slices after this inherit

- **31.21** names what a `strict` parse left unchecked: a `NOT NULL` column is
  checked, a domain's beneath a container is not until 31.26.
- **31.26** carries a domain's `NOT NULL` into `NestedPlan`, where 31.19 put a
  `varchar(n)`'s length; `domain_not_null`'s `KD97` marker says where it is
  read.
