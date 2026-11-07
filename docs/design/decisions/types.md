# Type resolution and decoders

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. Turning a declared type into an Arrow type and a
comparison: the bar every mapping clears, the ADBC floor it is held to, the nested plan carried beside each `DataType`,
and the decoders and renderers that read a dump's text and write it back. `pgdt-codec`.

### D37 The bar: the dump alone determines the value
A declared type maps to a real Arrow type only if its text round-trips consulting nothing outside the file; otherwise
`Utf8View` with a note naming the kind of unknown. Misreading is unrecoverable, not recognizing is not; `money` fails it
(`KD13`). Every column is nullable regardless of DDL, a `NOT NULL` refusing a NULL instead, which `ignore` reads (I76);
a range's three flags are not. Rejected: a bare built-in name `Unknown` where a declared type could shadow it —
`pg_dump` writes built-ins bare under an emptied path (I8), so it would demote its own columns (`KD48`).

### D38 The ADBC driver's shipped release is a floor, swept from the catalog
Where the ADBC driver yields a real Arrow type, ours is never wider. The floor is one pinned release, never `main`,
swept per declarable `pg_catalog` type at every major, and `floor_mapping.py` fails drift in either direction; the
driver's refusals and `arrow.opaque` answers fall outside it by the file's own columns, and a row it reaches that we do
not meet carries a `Disposition`. It binds the typed mode alone: the untyped mode, like `--schema-mode strings`, is the
user asking for a wider type, and widens only a column the map says its front end cannot hold (D100). Rejected: the
`typelem` shape test, which deletes `int2vector` (I47).

### D40 The comparison is decided per declared type, in the same arm
`builtin_scalar` answers Arrow type and `CompareKind` together, per declared type and `COLLATE`: types sharing
`Utf8View` compare differently, so nothing orders by the Arrow type. DataFusion semantics is one per query, lest rows
depend on what a plan pushes; it maps each kind (`datafusion_order`), every text-emitted kind to `Text` — `macaddr` too,
though its file text orders as its octets, since a user's literal is compared bytewise — and a divergence is the
column's, never a term's (D59). Rejected: a nested column ordered as DataFusion does (`make_comparator` under default
options, marked `TODO` upstream in `compare_op_for_nested`); for `macaddr`, a per-literal switch bought for range
pruning (refusing a literal not in lowercase, or keeping the octet key for one that is).

### D43 `NestedPlan` travels beside the `DataType`; the census speaks after the DDL and moves the pair
An Arrow type does not name its literal (`int4range[]`, `int4multirange`: both `List<Struct>`), so a `NestedPlan`
travels beside every `DataType`, every builder and renderer takes it, and the pair changes only whole. The census speaks
after the DDL, never instead of it, and is a parameter of resolution so no caller skips it. `MetadataNotScanned` is held
apart from `NotDeclared`: same output, opposite advice. Rejected: widening `builtin_scalar`'s tuple; `Field` metadata.

### D45 Strict decoders apart from permissive parsers; decode and render allocate only what they return
`decode_*` reads what a dump holds and cannot loosen, each the inverse of its `render_*`; `parse_*` reads what `*_in`
reads — a filter's literal, a strictly checked field — and cannot tighten: two scanners, not one with a flag, each
transcribing the newest major and under-accepting (I35, I44). Wrong depth, `[lb:ub]=` or a field-count mismatch is
refused, never reshaped; a value no text form spells is never rounded (I40), and `FieldRender` where its renderer tells
(`KD166`). A renderer writes into its caller's buffer, an integer, boolean, text, date, time, timestamp or array with no
`String` between (`tests/render_allocations.rs` pins the count), and a decoder allocates only what it returns, an
array's element borrowing unless it carried an escape. Rejected: a `String` per rendered field. Evidence:
`nested-decode-micro`.

### D143 An input grammar is a walk, read a part at a time
`array_in`'s, `record_in`'s, `range_in`'s, `multirange_in`'s and `int2vectorin`'s grammars, and `json_in`'s with
`jsonb_in`'s refusals beside it, are walks handing their caller each part as read (`Parts`); `parse_*` collect the
nested ones and a strict check reads all, holding no copy and no tree of a field, only a scalar part's text and key and a
range's bounds and theirs. `parse_jsonb` is `JsonCursor`'s, held to the walk by a differential test. Rejected: a value's
key tree built and dropped, tens of bytes an element; a check walk beside each nested scanner. Code: `nested/input/`,
`json/walk.rs` (`JsonWalk`), `grammar/walk.rs` (`FieldWalk`). Evidence: `input/differential.rs`,
`tests/decoded_copies.rs`.

### D144 A reader is a push machine fed a field's runs, looking at no byte past the one it reads
`Unescape`, the input walks (D143), `TextFold` and `ByteaFold` are hand-written state machines fed a field's bytes as
its runs come, carrying past a run's end only their own state: an escape begun, `NULL`'s letters, a quote awaiting its
double, a whitespace run as its bytes' run lengths (`Blanks`, `KD167`), a `bytea` read once for whether it decodes and
whether `byteain` refuses it. Why: `resumable-reader-micro`. Rejected: the `async` walk, past its noise of the pull walk
where the push machine is below it; a pull walk forking ahead, holding runs back to the fork; a machine generic over its
parts, instantiating without bound. Code: `value/source.rs`, `nested/input/`, `json/walk.rs`, `bytea.rs`,
`grammar/walk.rs`, pgdt-core's `Unescape`. Evidence: `check/tests.rs` (`walked_in_runs`), `tests/decoded_copies.rs`.
