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
`decode_*` reads what a dump holds and cannot loosen, each the inverse of its `render_*` on what `*_out` writes;
`parse_*` reads what `*_in` reads — a filter's literal, a strictly checked field — and cannot tighten: two scanners, not
one flagged, each transcribing the newest major and under-accepting (I35, I44). Wrong depth, `[lb:ub]=` or a field-count
mismatch is refused, never reshaped; a value no text form spells is never rounded (I40), and `FieldRender` where its
renderer tells (`KD166`). A renderer writes into its caller's buffer, an integer, boolean, text, date, time, timestamp
or array with no `String` between (`tests/render_allocations.rs` pins the count), and a decoder allocates only what it
returns and an array's dimension list, an element borrowed unless escaped. Rejected: a `String` per rendered field.
Evidence: `nested-decode-micro`.

### D143 An input grammar is a walk, read a part at a time
`array_in`'s, `record_in`'s, `range_in`'s, `multirange_in`'s and `int2vectorin`'s grammars, and `json_in`'s with
`jsonb_in`'s refusals beside it, are walks, the nested ones handing their caller each part as read (`Parts`) and
`JsonWalk` saying only whether its bytes are read; `parse_*` collect the nested ones and a strict check reads all,
holding no copy and no tree of a field, only a scalar part's text and key, a `json` value's open containers, a bit each,
and a range's bounds and theirs. `parse_jsonb` is `JsonCursor`'s, held to the walk by a differential test. Rejected: a
value's key tree built and dropped, tens of bytes an element; a check walk beside each nested scanner. Code:
`nested/input/`, `json/walk.rs` (`JsonWalk`), `grammar/walk.rs` (`FieldWalk`). Evidence: `input/differential.rs`,
`tests/decoded_copies.rs`.

### D144 A long field's reader is a push machine fed its runs, looking at no byte past its run
`Unescape`, the input walks (D143), `TextFold`, `ByteaFold` and the strict check's grammars (D146) are hand-written
state machines fed a field's bytes as its runs come, carrying past a run's end only their own state: an escape begun,
`NULL`'s letters, a quote awaiting its double. A scalar a key or a query's batch reads is collected for its slice
reader. Why: `resumable-reader-micro`; a dump's long fields are text, `bytea`, `json` and their containers. Rejected:
the `async` walk, past its noise of the pull walk; a pull walk forking ahead, holding runs back to the fork; a machine
generic over its parts, instantiating without bound; a machine per keyed scalar, dearer on every short value (D108).
Code: `value/source.rs`, `nested/input/`, `json/walk.rs`, `grammar/walk.rs`, `Unescape`. Evidence: `check/tests.rs`.

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

### D156 `Unescape` hands a field's text on a stretch at a time, an escape read whole where its run holds it
`Unescape::run` stages an escape's byte with the short stretches beside it and hands them on as one stretch, a longer
one as a slice of its run; an escape is read by one lookahead, `escape_at`, which `Unescaped` reads a held field by too.
Why: a reader's work per stretch, a UTF-8 check or a statistic's fan-out, is otherwise paid per escape, and a machine
stepped per byte, per byte (D108). Rejected: an escape's byte handed on alone, no copy made; a stage in each reader, the
one rule written per reader; `Unescaped` stepping a machine, a held field paying for runs it has none of;
`unescape_owned` reading a held field byte by byte, cheaper only on stretches as short as the control's stress column
holds, real escaped text's being longer. Code: `Unescape`, `escape_at`. Evidence: `copy.rs`'s tests, `--count-pair`,
`runs/dwal-33.27-stretches-20261113/`.
