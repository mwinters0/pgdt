# P5 — Pushdown: projection and predicate expressiveness

What this phase builds, decided before any code exists. The index row and the
schedule are [`roadmap.md`](roadmap.md); what has landed is
[`../status/STATUS.md`](../status/STATUS.md), never this file.

## What the phase is, and what it is not

Two unbuilt things, and they are not the same size:

- **Column projection** — a query materializes only the columns it asked for.
  `BatchOptions` has no projection today and `batch::RowBatcher::push_row`
  decodes every field of every row, so the saving is real and unclaimed.
- **Predicate expressiveness** — today the query surface is exactly one
  `Option<Predicate>` over `Eq`/`Ne`/`IsNull`/`IsNotNull`, compared as an
  unparsed string. Conjunction and ordering operators are unbuilt.

**The phase makes no claim about rejected rows getting cheaper.** The replay
loop already tests the predicate against the raw row and calls `push_row` only
if it passes, and `Predicate::matches` walks `split_fields` as far as one
column index and unescapes that single field, so a rejected row never reaches
a `ColumnBuilder`. Every performance claim this phase makes belongs to
projection.

**Whatever it claims, it claims about replay.** A query is two passes; the
mapping pass walks and censuses every row between `scanned_through` and the
target regardless of what is asked for, and replay then re-reads one block's
extent. Pushdown's share of a cold query's wall clock is that block, not the
file. Skipping *bytes* is P10's.

## Predicate expressiveness stops at a conjunction

**A predicate is a conjunction of single-column comparisons.** Several terms,
ANDed, each term a column, an operator and (for the comparing operators) a
value. `OR` and `NOT` are not in this phase.

**Ordering operators — `<`, `<=`, `>`, `>=` — are added, and they compare
typed**, decoding the field to the column's resolved Arrow type and comparing
there. They are available on any column that resolved `Mapped` with a
`NestedPlan::Scalar` plan, and are **refused with an error** on any other — a
nested column, or one whose resolution degraded.

**Where our comparison and PostgreSQL's differ, the difference is registered
rather than refused**, so the gap can be closed one type at a time later. The
register is the table below; it is seeded by `P5.6` and thereafter lives in
[`architecture.md`](architecture.md)'s "Predicates" section, which is where a
reader looking at the mechanism will find it.

| Arrow type | Reached by | Agrees with PostgreSQL | What would close the gap |
|---|---|---|---|
| `Boolean` | `boolean` | yes — `false < true` | — |
| `Int16`/`Int32`/`Int64` | `smallint`, `integer`, `bigint` | yes | — |
| `Float32`/`Float64` | `real`, `double precision` | yes, **given the NaN rule below** — these are the only types whose decoder yields a NaN | — |
| `Decimal128`/`Decimal256` | `numeric(p,s)`, `p ≤ 76` | yes — both sides carry the column's own scale, because the literal is decoded with the column's own decoder | — |
| `Date32` | `date` | yes | — |
| `Time64(µs)` | `time without time zone` | yes | — |
| `Timestamp(µs[, tz])` | `timestamp`, `timestamptz` | yes — compared as the stored instant | — |
| `FixedSizeBinary16` | `uuid` | yes — `uuid_internal_cmp` is `memcmp` over 16 bytes | — |
| `Binary` | `bytea` | yes — `byteacmp` is `memcmp`, then length | — |
| `Dictionary(Int32, Utf8)` | enum types | **no** — PostgreSQL orders an enum by *declaration* order; we compare the label text, so an enum declared `('low','medium','high')` orders alphabetically instead | the declaration order, which the dump carries verbatim in `CREATE TYPE … AS ENUM (…)`. Purely additive |
| `Utf8View` | `text`, `varchar`, `char`, `name` | **no** — PostgreSQL orders text by collation, and a plain dump records none (I32); we compare bytewise, which equals PostgreSQL only under `C`/`POSIX` | a collation the file does not carry |
| `Utf8View` | **bare `numeric`**, and `numeric` beyond 76 digits | **no, and this is the sharp one** — an unconstrained `numeric` column has no Arrow decimal representation, so it is `Mapped` to `Utf8View` and orders lexicographically: `"9" < "10"` is false | an arbitrary-precision decimal comparison |

**NaN follows PostgreSQL, not Rust — and only the float rows reach it.**
`float8_gt(a, b)` is `!isnan(b) && (isnan(a) || a > b)`, so NaN is greater than
every value including infinity, and `NaN = NaN` is true, where Rust's
`partial_cmp` returns `None`. We implement PostgreSQL's. This applies to
`real`/`double precision`, which decode `NaN` to `f32`/`f64::NAN`; a `NaN` in a
`numeric(p,s)` column is a hard `Error::FieldDecode` today — `Decimal128` has
no representation for it — so it errors long before any comparison, and the
`Decimal` row above is unconditional. NaN is reachable through any numeric
column (I4).

**Under `SchemaMode::Strings` every ordering operator is refused**, because
nothing is `Mapped`. That falls out of the rule rather than needing its own
case.

**The register is enforced in code, not by discipline.** The classification —
`Agrees` / `Diverges` / `Refused` — is chosen by an **exhaustive `match` over
`DataType`**, so a newly producible type fails to compile until someone
classifies it. That is the check the register needs, because the moment it
would otherwise go stale is a routine type-mapping change in a different part
of the tree, and it fires there as a compile error rather than as a test
someone might not run. The table above is the human-readable rendering; the
code is authoritative.

*Rejected: a test asserting the Markdown table and the code agree row for row.*
Its own failure mode is bit-rot in the doc parser, and the table is small
enough to be re-read whenever the register changes.

**A divergent comparison is announced by the CLI, once, on stderr**, after the
schema resolves, naming the column and the divergence. It is not a
`Diagnostic` and not a `ColumnNote`: `DumpIndex.diagnostics` is the L1
file-level channel and `ResolvedSchema.notes` is the L2 per-column one, while
this signal is per-column *and* conditional on a predicate — L4 — so writing it
into either is the layering violation `layering.md` forbids. P6's inbox already
records that the caller-supplied **sink** is the designated unification point
for diagnostic channels, so inventing a third channel here would create one
more thing for that sink to reconcile. An entry is filed into P6's inbox
instead; an embedder gets nothing until then, which is where they already
stand for the ambiguity gap filed there.

**A nested column keeps text `Eq`/`Ne`, and that is a decision rather than an
omission.** Every value in a dump is already in canonical *output* form —
discrete ranges canonicalize on input, array input whitespace is dropped — so
the literal in the file is the one `array_out`/`record_out`/`range_out` would
write, and a string comparison against it agrees with PostgreSQL for every
value a dump can contain. The one divergence is a user supplying a
*non-canonical* literal, where PostgreSQL matches and the comparison does not.

**An ordering operator's refusal fires at schema resolution for a matching
block** — the same
place and moment `Error::UnknownPredicateColumn` is raised today by
`stream::resolve_predicate_index`, before any row of that block flows, so a
predicate is validated against a block in one place rather than two. A table
whose blocks carry different schemas can therefore refuse at the third block
after rows from the first two were emitted; that is already true of
`UnknownPredicateColumn` and adds no new shape of failure.

*Rejected: `OR` and `NOT` in this phase.* Not for code volume — for NULL. A
NULL field today matches neither `Eq` nor `Ne`: unknown is collapsed to false
at each term. That collapse is sound under `AND` and unsound under `NOT`, since
SQL's `NOT UNKNOWN` is `UNKNOWN` rather than `TRUE`. Admitting `NOT` therefore
does not add an operator, it obliges a real three-valued evaluator and re-opens
the semantics of every operator that already exists.

*Rejected: type-aware comparison on nested columns.* It needs the *input*-side
grammar, which I20's scope limit flags as considerably more permissive than the
`*_out` inverse the decoders commit to, plus canonicalization for the three
discrete built-in ranges (`int4range`, `int8range`, `daterange`; `numrange`,
`tsrange`, `tstzrange` do not canonicalize).

Both rejections are the same body of work and are scheduled as **P11**, after
P6 — see [`roadmap.md`](roadmap.md).

## Projection is addressed by name and cuts the resolved schema

**A projection names columns.** It is matched against the queried table's
column names exactly as `Predicate::column` already is — resolved per block
against that block's own schema, the way `stream::resolve_predicate_index`
does — and a name the block's schema does not carry is an error, mirroring
`Error::UnknownPredicateColumn`. Order is as requested, so a projection may
reorder; a repeated name is refused.

**`TableStream::resolved_schema()` returns the projected schema.** All four of
`ResolvedSchema`'s vectors — `schema`, `columns`, `notes`, `plans` — are cut
to the projection, in the requested order, together. They are positional and
parallel by construction, and `RecordBatch::try_new` checks the built arrays
against `schema` exactly; a stream that advertised the full table while
emitting narrow batches would put those two out of agreement.

*Rejected: addressing by index.* DataFusion's `TableProvider::scan` hands a
provider `Option<Vec<usize>>`, which is the shape P6 will receive — but those
indices are into the schema pgdq itself advertised, so P6 translates them to
names against that schema. Taking indices at *this* layer instead would make
the meaning of `2` depend on which block matched, which is precisely what
every other lookup here avoids by going through the `COPY` header.

**A filter may name a column the projection does not.** `SELECT a WHERE b = 1`
is ordinary SQL and stays ordinary here: the projection decides what is built,
never what may be tested. So the set of fields `push_row` must reach is the
projection *union* the filter's columns — which costs nothing, since the row is
walked whole either way.

*Rejected: reporting the full table schema and projecting only the batches.*
It reads as the friendlier API and it desynchronizes the one invariant
`RecordBatch::try_new` is checking.

*Rejected: requiring a filtered column to be projected.* It would make a
`COUNT(*)`-shaped query — `--no-columns` with a filter — inexpressible, which
is the case the figure's floor row is built on.

## The phase publishes one figure, and it supersedes the cross-file apparatus

Per [`roadmap.md`](roadmap.md), "A slice row that commits to a measurement
names its instrument": projection's saving is a performance claim, so it gets
an instrument in [`measurements.md`](measurements.md) taken by
`scripts/measure.py`, not a number quoted from a one-off run.

**The measurement apparatus is currently shaped around projection's absence**,
and says so: `generate_perf_data.py` and `measurements.md` both record that
"`pgdq query` has no column projection, so there is no within-file way to ask".
Three figures are built on that gap — `nested-end-to-end` attributes cost by
generating three separate 3.00 GiB files holding different column sets and
subtracting across them; `cross-file-floor` exists only to measure that
subtraction's own noise floor; and `composite-isolated` works around it with
two files declaring one column two ways. All three are workarounds for the
feature this phase adds.

**So the figure is one file at five projection widths**, warm and typed, on the
existing `--arrays --composite` input — 16 scalar columns plus two array
columns and one composite. The generator needs no change.

| Row | What its difference against the row above buys |
|---|---|
| 0 columns (`--no-columns`) | the replay floor: no decode, no build, no render |
| 1 small scalar (`v_smallint`) | what one cheap column costs above the floor |
| 16 scalars | the scalar set |
| 16 scalars + the composite | **the composite column alone** |
| all 19 | **the two array columns alone** |

Every attribution the cross-file apparatus makes is a subtraction between two
adjacent rows of this one table, over identical rows of an identical file. The
cross-file subtraction floor `cross-file-floor` measures does not enter, and
neither does the
census's file-dependent untyped baseline. That is what makes it a supersession
rather than one more figure.

The zero-column row is why the table is worth publishing rather than
arithmetic: a zero-column projection is `COUNT(*)`, and it measures the
irreducible cost of replay once no field is decoded at all — the floor every
other row is read against.

**`composite-isolated` is not published by the doc's sweep.** It stays in
`measure.UNTAKEN` until this phase supersedes it. Publishing it first would
mean writing its section and marker and deleting both a few slices later, when
a strictly better instrument — the same rows in the same file, with no second
file and no cross-file subtraction — exists. That sweep keeps the part that genuinely
needs a quiet machine: the re-stamp and `session-drift`.

*Rejected: folding projection widths into `nested-end-to-end`.* A figure is
exactly one whole table, and adding rows to a published one would silently
restate a number under a heading that does not claim it.

`--figure <id>` re-takes one table on its own, so this figure does not wait on
a sweep pair. It cannot be taken before the code lands, so the re-stamp runs
first regardless, and the fold-in re-reads whatever `--check` names as the
figure's consumers.

## A projection narrows what is decoded, never what is walked

**`push_row` keeps walking the whole row.** A projection skips `decode_field`
and the typed builder append for a column nobody asked for; it does not stop at
the last needed field.

The walk is `memchr` over the row and is the cheap half. The expensive half is
`decode_field` plus a builder append — what the per-row gap `nested-end-to-end`
reports is made of — and that is entirely what a projection removes. Stopping early
would buy the cheap half at the cost of the system's **only** field-count
check: `batch::RowBatcher::push_row` is the sole site that raises
`Error::ColumnCountMismatch`, and the mapping pass, which does walk every field
of every row for the array-shape census, never errors on a count. Under the
standing rule that the input contract is valid PostgreSQL rather than
`pg_dump`'s output, a hand-edited dump with a stray tab is exactly the input
that check exists for, and an early stop turns it into a wrong answer with no
error.

*Rejected: stopping at the last needed column and counting the remaining
delimiters with `memchr::count` to keep the check.* It is sound, and it is an
optimization of the half that was never shown to cost anything. If the walk
turns up in this phase's figure, it is earned then.

## Projection is a per-column escape from a hard decode failure

**A column that is not projected is never decoded, so a value that would raise
`Error::FieldDecode` no longer does.** Whether a query succeeds therefore
depends on its projection. That is intended and is to be documented in
`docs/manual/` when it lands, not left to be discovered.

It is the answer to a limitation that is real today: an array nested inside a
composite is a hard `Error::FieldDecode` naming the column, and the only escape
is `--schema-mode strings` for the *whole table*, which untypes every other
column as collateral. Projecting the column away is what a user would try
first, and after this phase it works.

*Rejected: decoding unprojected columns anyway to preserve error parity.* It
discards the entire saving to raise an error about a column nobody selected.

*Rejected: a strict mode that restores the checking.* The strict answer is
already reachable by projecting the column back in.

## PostgreSQL's special values are ordered, not undecodable

**An ordering operator answers `infinity`, `-infinity` and `NaN` exactly,
rather than raising `Error::FieldDecode` on them.** These are legal values of
their declared types — `pg_dump` emits them from any healthy database, and the
`types` fixture carries them — with a *total* order PostgreSQL defines and this
project verified against a live server (I33): `-infinity` below every finite
value, `infinity` above, `NaN` above `infinity` and equal to itself. The
comparison has a defined answer that needs no decoder.

What cannot hold them is **Arrow**, not the file: `Date32` has no infinity and
`Decimal128` has no NaN. That is a representation gap in the output type, and
reading it as the file contradicting its own DDL inverts where the limitation
lives.

So two populations are separated that a single `Error::FieldDecode` had
merged:

- **A legal special value.** A closed, enumerable set — PostgreSQL's own, not
  an open-ended class. `order_key` returns a sentinel that sorts correctly
  against every finite value, which is what the server itself stores.
  `OrderKey::Int` is already an `i64` while `Date32` is an `i32`, so the
  sentinels are free at the bottom and top of the range.
- **Text that is genuinely malformed for the declared type** — `abc` in an
  `integer`. Nothing can be concluded, and `Error::FieldDecode` stays.

This also repairs the ordering register rather than qualifying it. `Date32` is
recorded as `Agrees`, and that claim was false wherever the server answers and
this project errored.

*Rejected: excluding the row instead, as a NULL is excluded.* A NULL is the
absence of a value and has no order; `infinity` has one. Excluding returns a
silently wrong answer, which is worse than the error it replaces.

*Rejected: keeping the error and documenting the escape.* The escape the error
names does not exist for this case: `--schema-mode strings` resolves no column,
so it refuses ordering outright. A user filtering on a `date` column holding
`infinity` has no way through at all.

*Rejected: waiting for the build path to represent these values.* Deciding an
order needs strictly less than materializing a value, and holding the cheap
correct answer hostage to the expensive one buys a symmetry nobody asked for.

### The filter is exact where the batch still cannot hold the value

**A filter may correctly select a row the output column cannot then
represent**, so `--filter 'v_date < 2020-01-01'` selects `-infinity`'s row and
building the `Date32` column for it still fails. That is accepted and is a
property to state in `docs/manual/`, not a defect: the two paths have
different powers. The same asymmetry is already deliberate one section above,
where projecting a column away escapes its decode failure.

Materializing these values is a separate, harder question — a null, a sentinel
indistinguishable from a real date, or the error — and it belongs to whichever
phase owns typed materialization. It is carried in `STATUS.md`'s known
deficiencies until then.

## A third flush trigger bounds what a batch pins

**A batch flushes when the source byte span it covers exceeds a cap**, beside
the existing `max_rows` and `max_bytes` triggers. The cap is
`max_source_span: Option<usize>` on `QueryOptions`, defaulting to
`Some(64 << 20)`.

**64 MiB is chosen not to trip on ordinary work.** It is 64 default chunks — a
flat bound well inside the 512 MB cgroup the measurements run in — and the perf
inputs are ~3.86 KB/row (control) and ~4.49 KB/row (arrays + composite), so a
full-selectivity 8192-row batch spans ~32 and ~37 MiB and still hits `max_rows`
first, exactly as today. The default therefore bounds the pathological case
without moving any published figure. It is configurable because
`ScanOptions::chunk_size` is and the two interact; `None` restores today's
unbounded behaviour.

The `Utf8View` path hands `StringViewBuilder::append_block` a clone of the read
chunk's Arrow `Buffer`, so the **in-flight batch** pins every chunk it took a
view into until it flushes — the `chunks` deque's own eviction at the scanner
position does not release them. Neither existing trigger bounds that: `max_rows`
counts *selected* rows and `max_bytes` counts *selected* field bytes, so with
1 MiB chunks a 1%-selective filter pins on the order of 80 MiB and a
0.01%-selective one on the order of 8 GiB. Against this project's flat-memory
goal that is a defect rather than waste, and it is true of the shipped system
today — see [`../status/STATUS.md`](../status/STATUS.md)'s known
deficiencies.

Whatever adds this trigger must honour `batch::invalidate_block_cache`:
`StringViewBuilder::finish()` resets its block list, so every cached per-chunk
block index is invalidated on **every** flush.

*Rejected: compaction below a selectivity threshold*, which is how
[`roadmap-P7-scan-performance.md`](roadmap-P7-scan-performance.md) sketched it.
It is the same fix applied after the fact: it still admits an unbounded peak
before the threshold trips, and it copies exactly the data the zero-copy path
exists to avoid copying. A span cap bounds memory against a number a caller can
set, and — unlike a threshold — it is testable at fixture scale with a small
chunk size.

## Both halves are query options, and the CLI repeats rather than splits

**`BatchOptions` becomes `QueryOptions`, and carries both the projection and
the filter list.** The `predicate: Option<Predicate>` positional parameter on
`stream::table_stream` and `batch::read_table` goes away.

The two halves of this phase are the same kind of thing — what the query asks
for — and splitting them across a struct and a positional argument would make
every caller learn which is which. The rename follows from the same reading:
the struct already carries `database` and `scan_extent`, neither of which is
about batching, so a name that says "batch" is already wrong and two more query
concerns make it misleading.

**On the CLI, both repeat.** `--filter` becomes repeatable and its terms are
ANDed; projection is a repeatable `--column <name>`, with `--no-columns` for
the zero-column case. Ordering operators parse longest-first — `>=` before `>`,
`<=` before `<` — as `!=` before `=` already does.

*Rejected: `--columns a,b,c`.* A PostgreSQL column name may legally contain a
comma (`"a,b"` is a valid quoted identifier), so a comma-separated list either
invents a CLI quoting grammar or has a case it cannot express — against the
standing rule that the input contract is valid PostgreSQL rather than
`pg_dump`'s usual output. Repeating needs no grammar and matches `--filter`.

*Rejected: `--columns ''` for the zero-column case.* `--no-columns` says it.
And the case is not only the figure's: `pgdq query --table t --filter 'x=1'
--no-columns | wc -l` is a filtered row count.

**`--no-columns` is load-bearing for the figure, not a convenience.** Every
query figure in [`measurements.md`](measurements.md) is taken through the CLI —
`pgdq query --source … --dqcache none … >/dev/null` inside the container — so a
zero-column projection that only the library could express would be a figure
that could not be taken. It also means this phase's figure is an end-to-end
number, `render_field` included, like `nested-end-to-end`, rather than a
library-internal one.

**A zero-column batch needs `RecordBatch::try_new_with_options`.** `try_new`
fails with "must either specify a row count or at least one column", so
`RowBatcher::flush` passes an explicit `row_count`. Verified against
arrow 59.2.0, which documents the option for exactly this.

## A filter term is parsed for two audiences

Two kinds of user read `--filter` differently, and the syntax serves both
rather than choosing. Sysadmin-shaped users find the bare `column=value`
spelling natural. SQL-fluent users assume a string literal must be quoted, and
write `--filter 'foo = "the answer"'` — double quotes rather than SQL's single
ones, because the term is already inside shell single quotes. Both spellings
mean what they look like.

**Whitespace outside quotes is not data.** A term is trimmed on both sides of
the operator, so `--filter 'foo = the answer '` asks for `the answer`. The
column side already trimmed; the value side now does too. Untrimmed, the
failure was loud on a typed column — `Error::PredicateValueDecode`, naming the
value — and silent on a text column under `=`, where a leading space made an
empty result that read as an answer. Whitespace is Rust's `str::trim`, matching
the column side, so the parser holds one definition of it and a non-breaking
space pasted out of a web page is caught. An all-whitespace value collapses to
the empty string with no special handling.

**A quoted value is taken exactly as written**, which is what restores every
value trimming would otherwise make unaskable — `--filter 'foo = " x"'` is a
leading space, so a space-padded `char(n)` value is expressible from the
command line and not only through the API.

**Both `'` and `"` open a quoted value, matching pairs only, with an interior
quote doubled** as SQL does it. Accepting one character would punish whichever
half of the audience reached for the other, and which one a user picks is
decided by the shell rather than by taste. Doubling keeps the grammar closed:
no escape alphabet, and so no second decision about what a backslash-n or a
doubled backslash mean. Two quote characters also give a lazier escape for
free — a value holding one quote character can be written in the other.

*Rejected: backslash escaping.* A backslash inside a shell double-quoted
argument is itself shell-processed, so the correct spelling is one nobody
writes right twice.

**Quotes work on the column side too, and the operator split is quote-aware.**
PostgreSQL identifiers hold spaces, case and operator characters, and SQL
spells that `"my column"`; the quotes are stripped and nothing else happens,
since column names are matched verbatim and there is no case-folding to
reproduce. `split_filter_op` skips quoted regions rather than taking the
earliest operator byte anywhere, which is what lets a column named `a=b` parse.
The rule is otherwise unchanged — earliest position, longest spelling — so
`name=alpha>x` still parses as before.

**The `IS NULL` / `IS NOT NULL` forms are the fallback, not the first test.**
An operator outside quotes is looked for first, and the `IS` forms are tried
only on a term that has none. That is a fix rather than a reordering: stripping
the suffix from the whole term first makes `--filter 'note=this is null'` an
`IS NULL` on a column named `note=this`. Under this order it is an equality
against `this is null`, which is what it says. Quote-awareness is needed on top
rather than instead — that term goes wrong with no quotes anywhere — and it is
what lets `--filter '"is null" = x'` name a column `is null`.

**A malformed quote is refused, never reinterpreted.** If what remains after
trimming opens with a quote, it must close with the matching one at the very
end with every interior occurrence doubled; an unterminated quote and trailing
text after the closing one are the same error with the same message. The
alternative — falling back to the unquoted reading — is the silent-wrong-answer
shape this whole grammar exists to remove.

**A value that opens and closes with a matching quote is quoted, always.**
There is no telling a SQL user quoting a string from someone searching a `json`
column for a quoted word, and a rule that guessed from the column's type would
make a term's meaning depend on a schema resolved much later. The escape is the
doubling rule, and the manual shows that case beside the `char(n)` one, since
they are where a user needs to know the rule exists.

**Quoting stops at `--filter`.** `--column` and `--table` take their names
verbatim, which is not an inconsistency with the above but the same reasoning:
a filter term is one string that must be split into three parts, so quotes
carry boundary information there, while the shell has already delimited a
`--column` argument. This is also what keeps the rejection of `--columns
a,b,c` above intact — that flag would have to *invent* a grammar, where
`--filter` already has one. Quotes on `--column` would be decoration that made
a column genuinely named with quote marks unaskable. The failure stays loud, so
what is added is the missing sentence: a name that is not found and that opens
and closes with a matching quote says it was matched literally, quote marks
included.

## Resume carries a query fingerprint, and the projection cuts the diagnostics

**`ResumeToken` gains an opaque `query_fingerprint: u64`** over the table, the
projection, the filter list and the schema mode; resuming a stream whose
options do not match it is an error.

The token exposes no fields and never will, so this costs nothing at the API
surface, and it defends a rule that is otherwise defended by nothing: "one
schema per stream, resolved up front". Today the token carries no fingerprint
of the query at all — not even the table name — so resuming against a
different table is already unchecked, and a projection makes the silent-
divergence case ordinary rather than exotic.

**A projection cuts `notes` and `columns` with the rest of `ResolvedSchema`**,
so an unprojected column's resolution note is not reported by that stream. This
follows from the schema being the projected one, and costs nothing that matters:
`pgdq query` does not print notes, and `pgdq info` — which does — never
projects.

## Slices

Ordered so each makes the next one's mistakes visible. Progress is tracked in
[`../status/STATUS.md`](../status/STATUS.md), never here.

| | Slice | What it is |
|---|---|---|
| **P5.1** | Register the figure | The generator already emits the 19-column input this needs, so there is no generator change and no staleness to acknowledge: the slice registers the figure under `measure.UNTAKEN` and adds the harness's command shapes for the five widths. They name flags that do not exist until `P5.4`, which is harmless because `UNTAKEN` is never run. **No library code.** |
| **P5.2** | The source-span flush trigger | Closes a gap in the *shipped* system and settles the batch layer's flush path before projection touches `push_row` |
| **P5.3** | Projection in the library | The `QueryOptions` field and the API move, the projected `ResolvedSchema`, `push_row` skipping, the zero-column `RecordBatch` |
| **P5.4** | The CLI for projection | `--column`, `--no-columns`. What the figure invokes |
| **P5.5** | The filter conjunction | Repeatable `--filter`, terms ANDed |
| **P5.6** | Typed ordering operators | `<`, `<=`, `>`, `>=`, and the refusal on a column that is not `Mapped` with a `Scalar` plan |
| **P5.8** | Special values are ordered | `infinity`/`-infinity`/`NaN` answered exactly by `order_key`; `Error::FieldDecode` kept for genuinely malformed text. Repairs `Date32`'s `Agrees` claim |
| **P5.7** | Take the figure and retire what it replaces | Fold it in; re-read the consumers `--check` names; delete `composite-isolated` from `measure.UNTAKEN` and re-scope `cross-file-floor` and `nested-end-to-end`, whose cross-file subtraction this figure supersedes; update the manual |
| **P5.9** | The filter term is parsed for two audiences | Trim outside quotes, a quoted value taken as written, both quote characters with a doubled interior quote, a quote-aware split, quoted column names, and the `IS` forms demoted to the fallback — which fixes the `note=this is null` misparse |

**`P5.9` is a clarification of work `P5.5` and `P5.6` already landed** — the
grammar that would have been in this spec had it been anticipated. It is a
slice rather than an out-of-band item because it changes a documented contract
of the mechanism those slices built, and the phase is open.

**`P5.8` runs before `P5.7` despite its number**, which is allocation order,
not position: it was discovered when `P5.6`'s register was reviewed. The sweep
has to measure the finished library, and `P5.7`'s manual pass is what documents
`P5.8`'s property.

Two seams are deliberate. **P5.2 is not part of P5.3**: it repairs
already-tested behaviour, and a review judging that at the same time as a new
capability has to accept both at one confidence. **P5.6 is not part of P5.5**
for the same reason inverted: ANDing terms that already work is not the review
that introducing typed comparison is.
