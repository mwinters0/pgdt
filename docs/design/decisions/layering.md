# Layering

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. The crate graph and what crosses it: seven library crates
and the module assignment `concepts.md` states and `layering.py` enforces, what each layer may name, what is API and
what is `internal`, and how a refusal is spelled for each front end.

### D68 Seven crates, the module assignment Cargo enforces
Seven library crates — core, model, io, dump, codec, mapping, query — each using only those before it, dump and codec
siblings; front ends and embedders above query. Each module's crate and its reason are `concepts.md`'s;
`scripts/layering.py` holds every member to the graph. Model is apart from core so an edit there is the one asking
whether the cache format bumps, and below the parsers so the codec does not recompile with the grammar. Rejected: four
layers in one crate with upward deviations; the cache a crate of its own (its loader re-derives the map's diagnostics,
so it is dump's); the mapping pass query's (it reads nothing of query); the graph held to the libraries alone (a new
member unplaced); codec and query, or dump and query, one crate (D74's boundaries).

### D74 Below the codec no Arrow, the codec pure, and the core's error naming no dependency
Core, model, io and dump name no Arrow; codec and mapping only its schema types, the codec also `i256`; model and codec
do no I/O, the codec synchronous. A decoder takes an unescaped field and returns a value, never a builder: arrays are
query's, whose views tie building to the read buffers (D46). Anything persisted is model vocabulary — declared type
strings, never a `DataType` or another codec conclusion; a contract over a block's rows is model's. The core error
carries a dependency's as an opaque `Cause`, and the provider's refusals reach DataFusion as
`External(datafusion_pgdump::Error)` over it. Rejected: a `#[from]` variant per dependency (each compiled everywhere);
the core error crossing bare; `OrderKey::Decimal` as digit strings or any integer but `Decimal256`'s.

### D106 A crate's API is a designed surface, and what a boundary forces public that is not API is `internal`'s
A library's API is its root and the namespaces `layering.py`'s `NAMESPACES` lists: what an embedder would call given
what the crate's `lib.rs` says it is for. What a crate above needs that is not API is `internal`'s (a method on an
`internal` `<Type>Ext` trait), named by no embedder; an item defined low that means something only above enters that
crate's API by re-export. No facade: nothing is published (D104) and a crate is reached directly. The seven are
`pgdt-<name>`; the DataFusion crates keep `datafusion-*`, the prefix that ecosystem finds a provider by. Rejected: what
is `pub` or named by an embedder today as the test; a facade (every name spelled twice); a published `pgtypes` crate out
of the codec; `introspect` or the DataFusion adapter exempt from the embedder rule. Evidence: `tests/embedder.rs`.

### D107 A library refusal names its remedy in the library's terms, and each embedder spells it after the sentence
A library error states the condition and ends with its remedies as the API names them — a mode, a mapping pass, a
selector naming a database — never a front end's flag or command, nor does a log line. What a front end must spell is in
the variant, never its `String`, a refusal a mode causes naming that mode where it is raised. `pgdt` appends its flags
in the library's order on every refusal leaving a command; `datafusion-pgdump`'s `Error` carries the words of the
`Spelling` a front end handed at open or registration, `pgdt sql` handing one per grammar. Rejected: a structured
`Remedy` (a second taxonomy); a sentence per variant per front end (wording that drifted); spelling in `pgdt sql`'s
`run()` alone; DataFusion's `Context`; a variant wrapping any refusal.

### D129 A core error displays its cause and never chains it
A `pgdt_core::Error` variant carrying a cause, an `io::Error` or a dependency's `Cause`, writes it into its `Display`
and has no `source`: a reporter walking the chain (anyhow's `Caused by:`) prints each message once, and one printing
`Display` alone (DataFusion's `External`) prints it whole. Each cause displays what lies beneath it, and none wraps one
of ours: the one `Arrow` raised is `RecordBatch` validation's. A typed cause is its field's (`Error::codec_cause`).
Rejected: chaining, not displaying (lost where `Display` alone prints); chaining what lies beneath the cause (xz-seek
and arrow display and chain theirs, so the duplicate moves down a level); a choice per variant. Reopens: a cause whose
`Display` omits its inner error. Code: `pgdt_core::Error`. Evidence: `error::tests`, `gzip_source.rs`'s
`a_codec_failure_is_said_once`.
