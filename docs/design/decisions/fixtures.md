# Fixtures and tests (`scripts/`, `fixtures/`)

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. How the suite knows it agrees with PostgreSQL: real
`pg_dump` fixtures on a pinned image, oracles and registers generated out of their producers, round trips, the
unrepresentable extremes, and how a hot-path change is priced. `scripts/`, `fixtures/`, each crate's `tests/`.

### D69 Fixtures are real `pg_dump` output on a pinned glibc image family
`postgres:<major>.<minor>-trixie`, exact minor, never `-alpine`: musl's `strcoll` is `strcmp`, so every text answer
would be the `C` answer. Four things move on regeneration (`\restrict`, `now()`, `--verbose` timestamps, OID drift) and
nothing may assert on them.

### D71 Agreement with PostgreSQL and `pg_dump`'s forms are generated checks, read out of their producers and joined by existence
"Agrees with PostgreSQL" is the fixture servers' own answers, committed per major (`comparison_oracle.py`,
`value_oracle.py`), never written by hand; one semantics, the newest major's, serves all (I35). A register is read out
of its producer's source, a `match` being no data and the upstream checkouts machine-local (`emitter_register.py
--extract` commits what the join reads), and joined by existence, an arm in an oracle case, a literal in a fixture of
its major, never by branch coverage. An exemption names what the check resolves, never "not read", the map tiling every
byte; a fixture exposing a `KD<k>` is held failing until its fix (`known_failures.rs`). Rejected: a C parser; a cross
product of session settings; a `DO` block testing the version in place of a sidecar file.

### D73 Round trips, asserted shapes, and a hand-verified signal path
`Typed` against `Strings` over a fixture and `encode(decode(raw))` against on-disk bytes hold that render inverts
decode; that decode is right is the value oracle's (`value_oracle.rs`). Boundary values are unit tests;
`v_box_domain_array` (I22), where the round trip is blind, is pinned directly. A shape under test is asserted
(`is_seekable()`, a stated `--jobs`), never assumed. No test races a signal or a scheduler: it trips or sequences the
event at a chosen point (`CancelsPast`, `a_lifted_frontier_takes_the_chunks_its_tasks_decoded`), and koji covers the
real scale.

### D102 The unrepresentable category is read off hand-written extremes through DataFusion's own path, never listed
Which values an Arrow type cannot hold is read, never listed: `types`' `t_extremes` and `t_extremes_nested` hold
PostgreSQL's least, greatest and special values of each arm mapping to a type other than `Utf8View`, by hand, PostgreSQL
keeping no catalog of them; each is held to what DataFusion's own `arrow-cast` formats and strictly casts, the rest
recorded by tier and held to D96's count, so an upgrade moving the calendar fails. `floor_mapping.py` forces a newly
typed arm's extremes (D38). Rejected: the list kept by review; a bound per type (the list again); a decoder's refusal as
the test (it took `24:00:00`). Code: `arrow_holds`, `extremes_problems`. Evidence:
`every_extreme_is_held_by_arrow_or_recorded`.

### D108 A hot-path change is priced by `--count-pair`, and a rise it resolves is placed by callgrind, then reworked or accepted
A hot-path change is priced in `instructions:u` on the shipped build at the tree's parent and at the tree, against its
own pair's spread, never as a re-taken figure. A resolved rise is placed by callgrind's `Ir` on the two legs, layout
only where callgrind does not reproduce it or no changed code sits above, then reworked into a shape as clean or
accepted; none is too small to place. A phase adopts the refusal by its spec. Rejected: one count a leg; determinized
shapes (a workload never shipped); a sampled profile placing it (line tables move the code); a materiality floor; rework
resting on codegen (`ChunkedScanner::end_pass`). Code: `cmd_count_pair`, `callgrind_recipe`. Evidence:
`test_measure.py`'s `CountPair` and `CallgrindRecipe`.
