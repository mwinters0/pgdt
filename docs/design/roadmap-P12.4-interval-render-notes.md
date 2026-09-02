# P12.4 — `interval`: render-back's refusal, and `KD8`'s third case

What 12.5 inherits, and the calls the code does not explain itself.

The mechanism is filed by subject:
[`architecture.md`](architecture.md), "Decoders and render-back" — which now
carries render-back's third outcome, the two alternatives rejected for it, and
`KD8`'s rewritten paragraph. This doc holds only what the next slice needs.

## Module map

| File | What it is |
|---|---|
| `pgdump_query/src/decode.rs` | `render_interval` returns `Option<String>`, `None` for a nanosecond count with a remainder |
| `pgdump_query/src/error.rs` | `Error::FieldRender { declared_type, reason }` |
| `pgdump_query/src/batch.rs` | `render_field` → `Result<Option<String>>`, and `render_array`/`collect_array` with it |
| `pgdump_query-cli/src/main.rs` | `print_batch` → `Result<()>` |

## The spec row was two thirds delivered before this slice opened

D11's row named three things and **the comparison register's `interval` arm was
already in the tree**: `builtin_scalar` answers the Arrow type and the
comparison in one tuple, so 12.3 could not change `interval`'s mapping without
touching the same arm, and the comparison half — `agrees(K::Interval)`, the
fused 128-bit span — was P11's and needed no change at all. Nothing was skipped
and nothing was added to compensate; the row is delivered because the arm reads
`(Interval(MonthDayNano), agrees(K::Interval))` and
[`architecture.md`](architecture.md)'s register table says so.

**Worth knowing for 12.6**, which is the same shape: `int2vector`'s resolution
arm and its register row are one tuple too, so the slice that adds the codec
and the arm will find it cannot leave the register for later even if it wanted
to. That is a property of `builtin_scalar` being a single exhaustive `match`,
which is also what makes the register total.

## `render_field` grew a third outcome, and the signature is the whole slice

`Ok(None)` is SQL NULL; `Err(Error::FieldRender)` is an Arrow value no
PostgreSQL text form spells. The two rejected alternatives — truncating, and
panicking to keep the `Option<String>` signature — are argued in
[`architecture.md`](architecture.md), "Render-back has a third outcome".

Three consequences the next slice should not be surprised by:

- **The walk recurses, so every nested arm propagates.** `Array`,
  `Multirange`, `Record` and `Range` each `?` their children, and
  `collect_array`/`render_array` became fallible with them. A refusal at a leaf
  therefore reaches the caller from inside a `text[]` or a composite, which the
  unit test exercises through a `List<Interval>` rather than trusting the
  recursion.
- **`print_batch` is fallible and cannot fire.** Every typed column the CLI
  prints was filled by `append_typed` from a `decode_*`, whose range is by
  construction what the matching `render_*` writes back, so the binary has no
  reachable path to the error. It propagates rather than unwrapping because an
  unreachable panic in the output path is a worse answer than an error
  message — not because anyone expects to see it.
- **Test call sites `.expect("renders back")`.** Six of them, across
  `tests/common/mod.rs`, `tests/nested.rs`, `tests/decode.rs`, `tests/batch.rs`
  and `batch.rs`'s own two helpers. That is deliberate rather than lazy: a fixture
  value that stopped rendering back would be a real failure, and the expect
  says so at the point it would happen.

## `KD8` was rewritten, not annotated

D4 sends `interval`'s two lost value classes into the existing entry rather
than opening a stance-(a) row beside it, and the rewrite says what is now true
instead of recording what was added: three classes, `Date32`'s missing
infinity, `Decimal128`'s missing `NaN`, and `Interval(MonthDayNano)`'s two.

**The entry stayed enumerated rather than being generalised** to "any value the
Arrow type cannot hold". That wording would have swallowed the
`Timestamp(Microsecond)` overflow at PostgreSQL's documented maximum timestamp,
which is filed one section up as an expected `FieldDecode` and is **not** a
deficiency — the value is 294276 CE and no healthy database emits one, where
`pg_dump` writes an infinity from any of them. Widening the sentence would have
quietly promoted a fact into the register.

**The render refusal earns no register entry of its own.** It is not a
limitation with a remedy the user lacks: PostgreSQL has no sub-microsecond
interval, so the refusal is the correct answer rather than a weaker one, and
the value cannot appear in a dump at all.

## Measurement

`decode.rs` and `error.rs` are declared by no figure. `batch.rs` is declared by
`nested-end-to-end`, `cross-file-floor` and `projection-widths`, and
`pgdump_query-cli/src/main.rs` by all three of those plus `census-attribution`
and `map-only` — every one of which was already red, for
the reasons [`../status/STATUS.md`](../status/STATUS.md) gives. No
acknowledgement is written: the change adds a `Result` discriminant to the
per-field output path, which is a query-shaped figure's measured path, and
"small" is not evidence. `uv run measure.py --stale` is therefore unchanged at
thirteen, and `--check` still reconciles thirteen markers.
