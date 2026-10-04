# P31.24 — A base type's `DELIMITER` read from the preamble: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"What the register finds, and when the phase ends"; the entry it closes was
`KD94`, and the check it corrects is 31.18's
([`roadmap-P31.18-geometric-in-notes.md`](roadmap-P31.18-geometric-in-notes.md)).

## What exists

- **`TypeKind::Base { delimiter: Option<u8> }`**, read by
  `preamble::base_type_delimiter` as `DefineType` reads the clause, the first
  byte of the `DELIMITER` literal (I22): `,` where the type states none, and
  `None` where the value is no plain string literal, which `pg_dump` never
  writes. `TypeKind::base()` builds the `,` shape.
- **`NestedCompare::Uncomparable` carries `delimiter`**, set by
  `pgtype::typdelim` through domains and a typmod, and
  `predicate::array_delimiter` splits a container's array there. A delimiter
  `None`, or a byte `array_in` reads as syntax at some supported major (a
  brace, a double quote, a backslash, whitespace, a control or non-ASCII
  byte), splits nothing: that array's check refuses nothing, and its element
  type is already listed as unchecked.
- **A column of such an array is still text with its plan refused** (D41):
  only a composite's field of one is split, as before.
- **Fixture**: `fixture_schema_emitters.sql`'s `emitters.delimited`, a
  composite holding a `bt_varchar[]` (`DELIMITER = ';'`) whose quoted
  elements are followed by `;`, at every routine major. Its `COPY` text is a
  value form in `emitter_register.py`'s `VALUE_FORMS`.
- **`CACHE_FORMAT_VERSION` is 64**: a persisted `TypeKind::Base` changed shape,
  and the persisted-index digest moved with the fixture.
- **Evidence**: `tests/preamble.rs`'s
  `a_base_type_s_delimiter_splits_its_array_in_a_strict_parse`, a strict parse
  over the `emitters` fixture's `default` and `binary-upgrade` flag sets at
  every major, which refused `emitters.delimited`'s first row with the split
  at `,`; `preamble.rs`'s `a_base_type_s_delimiter_is_read_off_its_literal`;
  `predicate.rs`'s
  `a_base_type_s_array_beneath_a_container_is_split_at_its_delimiter`.

## Findings

- **A v15+ server hands `pg_dump` a non-ASCII `typdelim` as `charout`'s octal
  escape**, so the dump says `DELIMITER = '\377'`, which a restore takes as
  `\`, the literal's first byte. The preamble reads that byte too, and the
  split declines it, `array_in` reading a backslash differently across majors;
  the restore's own change of delimiter is PostgreSQL's, and nothing is filed.
