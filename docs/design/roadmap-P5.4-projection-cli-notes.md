# P5.4 — The CLI for projection: notes

What the next slices inherit from giving projection a command line. The spec is
[`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md); the mechanism lives in
[`architecture.md`](architecture.md), "Projection" (its last three paragraphs)
and "Testing philosophy"; what has landed is
[`../status/STATUS.md`](../status/STATUS.md).

## What is there now

`pgdq query` takes `--column <name>`, repeatable, and `--no-columns`. clap
refuses the pair with `conflicts_with`. One free function,
`main::projection(Vec<String>, bool) -> Option<Vec<String>>`, turns the two
flags into the three states `QueryOptions::projection` already had; nothing
else in the query handler changed except the header rule below.

`pgdump_query-cli/tests/query_projection.rs` is new — nine tests driving the
real binary.

## Calls made here, and why

**A zero-column query prints no header, and the "no rows" branch moved off
that flag.** The header is the batch's own field names, so at width zero it
would be an empty line and `--no-columns | wc -l` would report `rows + 1` —
the one thing `P5.3`'s notes left open for this slice. The header is therefore
suppressed when the batch has no columns, and each row still prints as an
empty line, which is what makes the count come out.

That splits one flag into two. `header_printed` used to mean both "the header
is out" and "something arrived", and the tail message read the second meaning
off it; a zero-column query makes those different facts, so there is now a
separate `any_batch`. Reusing `rows > 0` instead would have changed what a
table that yields an empty batch reports, which is not this slice's to decide.

**The CLI rejects nothing about the names themselves.** A repeated `--column`
and a name the block does not carry are the library's
`DuplicateProjectionColumn` and `UnknownProjectionColumn`, raised before a byte
is read and before the first block resolves respectively. Pre-checking
duplicates in `main.rs` would have given the CLI and an embedder two different
sentences for one fault, and the flag conflict is the only thing clap can
decide that the library cannot see.

**`--no-columns` wins in `projection()` if both flags somehow arrive.** That
branch is unreachable — clap refuses the pair — and the ordering is written so
the function is total rather than panicking on a state the parser guarantees
away.

## The test that discharges P5.1's obligation

`the_registered_projection_widths_are_executable` reads
`measure.PROJECTION_WIDTHS` and `measure.projection_flags` out of the harness
via `uv`, generates a 2 MiB `--arrays --composite` perf file, and runs all five
widths through the real binary, checking each prints the header width its row
label claims.

`P5.1` registered those command shapes against flags that did not exist and
named this slice as what makes them run. The failure it was guarding has no
other guard: an unaccepted flag is discovered when a sweep executes the query,
minutes into a figure, with the figure lost. The flags are therefore read from
the harness rather than transcribed, so a width added there is run here without
anyone remembering to.

It costs one generator run per `cargo test --workspace` (~0.5 s at 2 MiB),
which is the same trade `perf_generator_fidelity.rs` already makes, and it
fails rather than skips when `uv` is absent for the reason `common::require_uv`
states.

## What the next slices must not break

**`P5.5` makes `--filter` repeatable.** `parse_filter` is unchanged here and
already returns one `Predicate` per argument, so that slice changes the arity
at the call site, not the parser. `--column` and `--filter` are independent at
every layer: the filter's column index is resolved against the *unprojected*
schema, which `query_projection.rs` pins end to end with `--no-columns
--filter is_active=t`.

**`P5.7` takes `projection-widths` and updates the manual.** Nothing more is
needed for the figure — the widths run. The manual is still untouched, so the
per-column escape from a decode failure (`--column` naming everything but the
failing column, instead of `--schema-mode strings` untyping the whole table) is
documented for users only there; `Error::FieldDecode`'s message still names
only the `strings` escape.

## Staleness

This slice edits `pgdump_query-cli/src/main.rs`, which `nested-end-to-end`,
`census-attribution`, `cross-file-floor` and `map-only` declare — all four
already stale from `P5.3`, so `--stale` reads the same eight figures and no
more. Not acknowledgeable, and for the same reason: the header rule is on the
render path every query figure times. They stay stale until `P5.7`'s sweep.
