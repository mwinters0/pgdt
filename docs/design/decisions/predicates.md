# Predicates

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. The filter a query carries: its closed operator set, one
tree with short-circuit defined against the root, how a literal is read, how special values and equality are keyed,
divergence from the server's answer, the two CLI flags, and what a pushed or dynamic DataFusion filter may claim.
`pgdt-query`'s `predicate`, `pgdt-codec`'s `key` and `compare`, `datafusion-pgdump`'s `filter`.

### D53 The operator set is closed but for membership and the unrepresentable test
No `LIKE` (collation-dependent folding), `BETWEEN` (`And`), or column-to-column; `IS [NOT] DISTINCT FROM` is what
three-valued logic forces, `IS [NOT] UNREPRESENTABLE` what the null mode forces (D101). `IN` and `NOT IN` are
`PredicateOp`s holding their list, `NULL` included: one column against literals, as PostgreSQL parses `= ANY` and `<>
ALL` over consts (`transformAExprIn`), answering as the `Or` of `=` (or its negation) but decoding and looking up once a
row, where that `Or` costs per term (`dynamic-filter-join`). Rejected: `Expr::In`, a second kind of leaf for an embedder
to build; recognizing the `Or`, every front end emitting `In`. Evidence: `tests/membership.rs`.

### D54 One tree, no planner yet, short-circuit defined against the root
`filter` is one n-ary `Expr`, by default the empty conjunction. `And` may stop at the first `Unknown` except beneath
`Not`, only the root's `True` mattering. A decode failure surfaces only where evaluation reaches it — never in a group
statistics rule out, nor serially past a sorted block's stop — so which rows raise is outside the contract, `--jobs`
deciding. A resolution refusal is the plan's, before any row: the first refusing block's in file order, every leaf
walked, a misnamed column before anything a term asks. Rejected: DNF; exact Kleene everywhere; not skipping a group
holding an unkeyed value (`KD2`); the stop asked first (every valid dump paying for invalid text); a mode refused before
a typo. Reopens: a planner or rewrite, where a reading shows it pays. Evidence: `tests/pruning.rs`.

### D55 A literal is read in the type's `*_out` form at least and its `*_in` grammar at most
Floor and ceiling are `roadmap.md`'s, "A literal is guaranteed in `*_out`'s form and never read past `*_in`'s". A reader
keeps its narrow grammar, a nested literal's leaves too, and a port of `*_in`'s classifies only what it fails
(`decode::Unread`): a field fails a parse only at a `pg-refuses` check. A reader past the ceiling is replaced by its
port (`network_in`). `JSONB_MAX_DEPTH` is fixed (a stack overflow aborts). Zone names are open (tzdata grows unbidden),
abbreviations those PostgreSQL ships (I83). Rejected: failing every field read as no value; widening a canonicalized
kind's reader (`=` misses another spelling, `KD82`; `macaddr`'s one bounds set, D79, splits); for a narrowed `*_in`, the
dump's own major's; a server's own abbreviation file (forgoes I61's era edge); the tzdata at hand.

### D56 Special values are a rank in the key; equality has three canonicalizations, by injectivity of `*_out`
`infinity`/`NaN` are a rank in PostgreSQL's order, compared before the value; `OrderKey` derives no `Ord`. Rejected:
excluding the row like a NULL (I33, I34). `=` canonicalizes by whether `*_out` is injective over one file (`Equality`):
`v=1.5` hits `1.50` in a `numeric(10,2)`; `timetz`, `inet` are not rendered (I33, I38, I41); a `bytea` literal renders
in both `bytea_output` forms (I56), not decoded per row. A `ValueKey` carries the order it was read in, two relating
only as the semantics' engine relates them: the server's by type name (I92), DataFusion's by its coercion, in
`datafusion-pgdump`, its oracle (`key_order.rs`). Rejected: a variant per order; a declared-type tag on a scalar order;
cross-plan answers unspecified; arrow-cast in the codec (D74); the caller passing each key's Arrow type, unchecked.

### D59 Divergence is per position on its own channel: per term, and per column at registration
`ComparisonNote` carries a path and its declared type, per term from `comparison_notes` (operator-conditional, so
query's) and per column from `Table::divergences` in a query's semantics — DataFusion's variants there alone, as
DataFusion's `ORDER BY` reaches a column no term names. See I45, `KD7`, `KD10`. A provider registration folds its
`UnknownCollation` columns into one `UncollatedColumns` finding, every column of a collatable type with no `COLLATE`
earning one. Rejected: the front end folding them, re-reading the provider's sentences to recover a table and a reason.

### D60 `--where` is a second flag and the tokenizer defines the refusal set
`refuse_where_structure` refuses a `--filter` term tokenizing to anything but one leaf, an empty one `parse_filter`'s,
so a string both flags accept means one thing. A keyword needs whitespace, a paren or the string's end on both sides and
`NOT` after `is` stays in the term. A term splits at the earliest operator outside quotes, longest first; quotes are
stripped in a term, under either flag, and in no name flag. Rejected: `&&`/`||`; backslash escaping.

### D88 A pushed filter is `Exact` where the plan resolves it; a literal is the renderer's text
`supports_filters_pushdown` asks `Table::schema` to resolve the translated term in DataFusion semantics, the scan's own
refusals. A typed literal must be the column's Arrow type and is written by `render_field`, the decoders' inverse; a
string one stands only where the library compares text. Rejected: a formatter per type in the provider, a second grammar
to drift; pushing `IN` on a float, answered from a set with no `-0` made `0`; `Exact` where the library agrees with
PostgreSQL, which answers an enum `<` in declaration order pushed and in label order kept; `Inexact`, promising a
superset another semantics can break. Evidence: `crates/datafusion-pgdump/tests/pushdown.rs`.

### D101 The unrepresentable test reads the text against the declared type; DataFusion's is a scan's alone
`IS [NOT] UNREPRESENTABLE` tests a field's text against the declared type in the tiers its semantics reads (D98), under
every mode; `SchemaMode::Strings` refuses it. `pgdump_unrepresentable(<column>)` is pushed `Exact` and refuses wherever
DataFusion would evaluate it; `ready_session`'s appended optimizer rule refuses that at planning (RT22). Rejected: a
table function or companion column (no row identity; DataFusion has no hidden column, RT25); evaluating it (a NULL keeps
no origin); registration installing the guard (a rebuild, dropping prepared statements); a wrapped planner (one slot,
trusting an `Exact` conjunct gone); the function opt-in (withholds the always-sound pushed use). Code:
`unrepresentable_tests`, `ready_session`.

### D94 A dynamic filter is translated loosened into the library's tree, never read through `PruningPredicate`
Every producer re-checks its rows, so the scan answers `No` and only ever skips; a state is translated as a static
filter is (D88), loosened, so none refuses. A float compared with a zero has no term: a TopK and `MIN`/`MAX` order the
zeros apart where evaluation equates them. Held filters drop at `reset_state`. Rejected: `PruningPredicate` (bounds
decoded to scalars, pruned in DataFusion's semantics, nothing on a `CASE`, a long `IN` or a sorted block); a narrower
zero rule, sound only as far as each producer's shape is followed; keeping filters across a reset as Parquet does (a
recursive query's next iteration needs rows the last ruled out). Reopens: a release whose producers hold with the zeros
equal, then an `RT<n>`. Code: `filter::dynamic::loosened`.
