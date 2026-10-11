# P35.17 notes — pgdt-codec by type family

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt-codec" tree, its `moves` block applied whole and then struck from that
doc, as 35.23's were. Every body moved whole; nothing persisted changed, so no
format bump, and `golden_order_is_pinned_to_the_format_version` still holds
its digest. `module_graph.py` finds no codec cycle, and the two codec `OWED`
entries are struck.

## Where the cut fell

- **The families are `value/`**, one module each, with `value.rs` declaring
  them and holding `first_difference`. `decode.rs` keeps `Unread` and `Read`
  and declares every `decode_*`/`render_*` it declared before by `pub use`, so
  `pgdt_codec::decode::…` paths are unchanged. `decode/refusal.rs` and
  `decode/stored.rs` are gone into their families.
- **`nested` is a namespace**: `nested.rs` declares `value/nested.rs`'s 18
  items, and the crate root no longer does. `layering.py`'s `NAMESPACES`
  lists it. Above the codec only paths moved (`pgdt_codec::nested::…` in
  pgdt-query's `batch.rs`, `batch/prior_shape.rs`, `tests/nested.rs` and the
  codec's bench).
- **`pgtype` declares `Position`, `NumericTypmod` and `IntervalQualifier`**
  from `declared.rs`. The design named only `Position`; the typmod types were
  `pgtype`'s API too, and keeping their path costs one `pub use` line.
- **`compare` declares `comparison_for` from `compare/walk.rs`**, which it
  otherwise never reads. `oracle_register.py`'s `REGISTER` reads the walk's
  arms there.
- **`internal`'s paths changed and its contents did not**: each item is
  re-exported from its new home.

## Tests

- **`decode/tests.rs` is split by family**, each test going to the family
  whose items it calls, under `value/<family>/tests.rs`. Some families now
  have a one-test file.
- **`decode/differential.rs` and `decode/prior_shape.rs` stay `decode`'s
  children.** They check the API the namespace declares against its prior
  shape, across four families.
- **`key/order/tests.rs` is `key/tests.rs`, unsplit**: its four tests drive
  `field_key` across every family, so they test the dispatch.
- **`grammar/tests.rs` is split three ways**: `text_grammar`'s tests stay;
  the `unchecked_positions`, `field_refused` and delimiter tests go to
  `grammar/check/tests.rs`, and the two clause tests to
  `grammar/form/tests.rs`. The three-line `ty` helper is copied into both
  files that use it.
- **`resolve.rs`'s inline tests are `resolve/tests.rs`.**
- **The two key-only oracle tests are in `key/tests.rs`'s `oracle` module.**
  They read each major's `types` fixture through `preamble_only`, so the
  codec dev-depends on pgdt-dump, pgdt-io (for `LocalFileSource`) and tokio.
  `kind_name` is copied with them: the golden digest hashes its names, and
  pgdt-query's Arrow-agreement table still names kinds through its own copy.
  Both copies are exhaustive, so a new kind fails to compile in both. The
  crate's suite is 174 tests: 172, plus the two that moved.

## What later cuts inherit

- **`measure.py`'s declarations follow the families.** `DECODE` declares
  `decode.rs`, `value.rs` and every family file but `nested`.
  `PREDICATE` declares `key.rs` and each family file holding a key type.
  `PGTYPE` adds `declared.rs`, `compare/walk.rs` and `grammar/`'s two
  children, and `NESTED` declares `value/nested.rs`. A family file holding a
  key type sits in two mechanisms, so `allocator` and
  `parallel-scan-throughput` now merge their paths with `_declare`, as the
  rest do.
- **`file_limits.py`'s exemption names `value/datetime/grammar.rs`**, the
  same file moved whole. `value/nested.rs` is past the soft function limit,
  as `nested.rs` was.
- **The 35.16 entry under STATUS's "Decisions worth another look" is
  rewritten** to what is now true: a `grammar` namespace would now need only
  `text_grammar` and its two types moved to a child of their own.
- **Each family's private parsers that duplicate one another stay side by
  side**, as the design said.
