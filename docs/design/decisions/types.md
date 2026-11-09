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
nested ones and a strict check reads all, holding no copy and no tree of a field, only a range's bounds and their keys
and the text of a scalar part a slice reader keys (D147). `parse_jsonb` is `JsonCursor`'s, held to the walk by a
differential test. Rejected: a value's key tree built and dropped, tens of bytes an element; a check walk beside each
nested scanner. Code: `nested/input/`, `json/walk.rs` (`JsonWalk`), `grammar/walk.rs` (`FieldWalk`). Evidence:
`input/differential.rs`, `tests/decoded_copies.rs`.

### D144 A reader is a push machine fed a field's runs, looking at no byte past the one it reads
`Unescape`, the input walks (D143), `TextFold` and `ByteaFold` are hand-written state machines fed a field's bytes as
its runs come, carrying past a run's end only their own state: an escape begun, `NULL`'s letters, a quote awaiting its
double, a `bytea` read once for whether it decodes and whether `byteain` refuses it. Why: `resumable-reader-micro`.
Rejected: the `async` walk, past its noise of the pull walk where the push machine is below it; a pull walk forking
ahead, holding runs back to the fork; a machine generic over its parts, instantiating without bound. Code:
`value/source.rs`, `nested/input/`, `json/walk.rs`, `bytea.rs`, `grammar/walk.rs`, pgdt-core's `Unescape`. Evidence:
`check/tests.rs` (`walked_in_runs`), `tests/decoded_copies.rs`.

### D145 A whitespace run whose end decides it is fed on into the part and rewound, never held
An unquoted array element's whitespace run, and one after `NULL`, is fed on into its part from a mark as it comes, and
rewound to the mark where the byte after it says the run is not the part's; a refusal met in the run stands only where
it is interior, `{1\v}` being a `json[]`. The mark and rewind are `Parts`'s, each implementor restoring its own fixed
state or truncating what it collected. Why: a run may be a line's worth, which a restore reads, and a parse holds
nothing growing with a field (D144). Rejected: the run held until its end decides it, as its bytes or their run lengths,
growing on one alternating its bytes; a rewind in each leaf reader, one per type where `Parts` has a few implementors.
Code: `value/source.rs`, `nested/input/array.rs`, `grammar/walk.rs` (`Mark`). Evidence: `check/tests.rs`,
`tests/decoded_copies.rs`.

### D146 A grammar asking of a byte it has not read reads on as each answer; a number never reads ahead
`path_decode` reads a lone `(` as its bracket and `path_in` wraps its points in one only where it is the text's last
`(`: `GeometricWalk` reads on as both grammars, two at most, keeping the one the last `(` agrees with. `Strtod` refuses
where `strtod` would read on and leave a byte unread (`1e,`, `0x)`, `infin`), a byte no geometric grammar reads after a
number, and keeps a decimal's first 800 significant digits and whether one past them is nonzero, all its rounding reads.
Why: a parse holds nothing growing with a field (D144). Rejected: the text held from the `(` until its end decides, a
line's worth; a `strtod` backtracking over its read-ahead, which an open `nan(` tag makes a line's worth; every digit
kept. Code: `geometric/walk.rs`, `libc.rs` (`Strtod`). Evidence: `geometric/differential.rs`, `tests/decoded_copies.rs`.

### D147 An order key is read by its kind's push machine, a `&str` fed whole, with no slice reader beside it
An integer's, `oid`'s, `boolean`'s, enum's and `uuid`'s key is read by a push machine (D144) of fixed state — a
saturated magnitude and whether the text is `*_out`'s spelling, an enum's label up to `NAMEDATALEN` — which `field_key`
feeds a `&str` whole and a strict check a part's runs; another kind's slice reader is fed its collected text. Why: one
grammar per type (P33's spec). Rejected: Rust's `parse` beside the machine for `*_out`'s spelling, two grammars one test
holds together; a head clipped before a slice reader, a second grammar for what to clip. Code: `key/order.rs`
(`KeyRead`), `int.rs`, `bool.rs`, `uuid.rs`. Evidence: `key/order/differential.rs`, `tests/decoded_copies.rs`.

### D148 A `numeric` is read by a push machine holding only the significant digits its column's answer reads
`numeric_in` is fed a byte at a time (D144, D147) as this build's grammar and v14's and v16's beside it: a leading
integer zero and a trailing fraction zero are counted, never held; a typmod'd column holds the digits its precision and
scale round, a bare one I63's, past which each refuses, and a major's spelling no more than decides whether the column
stores it, a radix one to I63's width in bits. A `&str` is fed the majors only where the grammar fails.
`typmod_unscaled_digits` and `NumericKey::parse` are it fed whole. Why: one grammar per type; a bare key is its digits,
so fixed state cannot hold it. Rejected: the text collected for the slice readers, a field's worth for a padded value.
Code: `numeric/input.rs` (`NumericIn`), `key/order.rs`. Evidence: `key/order/differential.rs`,
`tests/decoded_copies.rs`.

### D149 A datetime's push machine is its lexer; its decoders and its key read the bounded fields
`ParseDateTime`'s lexer is the push machine a date, time, `timetz`, timestamp and `interval` text is fed through (D144),
skipping a blank or punctuation run as PostgreSQL does; its fields are within the server's workbuf and
`MAXDATEFIELDS` (I83), beside whether a NUL or non-ASCII byte was met. `datetime/grammar.rs`'s decoders and the value
readers read its fields, never the text, for a key, the tier and a query's batch alike; `interval`'s ISO 8601
fallback, reading the raw text, is its own machine. Why: only the lexer's input grows with a field, and PostgreSQL
splits `ParseDateTime` from `DecodeDateTime` alike. Rejected: the decoders as push machines, rewriting bounded state;
a head clipped at `longest_read`, refusing a blank-padded value the server reads; the batch on a text reader beside
it, two grammars (D147).
