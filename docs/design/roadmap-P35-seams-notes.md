# P35 notes — crate seams and file limits

What the phase leaves the next one, and what it tried and refused. The spec is
[`roadmap-P35-seams.md`](roadmap-P35-seams.md); the graph is
[`decisions.md`](decisions.md), "D68" and "D74", held by
`scripts/layering.py`; the shipped profile is "D105" over
[`measurements.md`](measurements.md), "What link-time optimization buys the
shipped build"; each crate's tree is [`concepts.md`](concepts.md). The
per-slice notes went at the wrap, and git holds them at `ce5014a8`. No `D<k>`
was added: every slice decision reached D68, D74, D103 or D105 in the slice
that made it, or sits beside its mechanism in code, and what the spec's "What
this phase does not do" rules is the keystone's to harvest. No `KD<k>` was
owned by P35, and no out-of-band row names it.

## What the next phase inherits

### The graph and the checks holding it

- **Seven libraries, `pgdt-fixtures` below them for tests alone, three
  embedders above** (`pgdt`, `datafusion-pgdump`, `datafusion-cli-pgdump`),
  and the apparatus (`count-instructions`, `namespace-init`, `peak-rss`).
  `layering.py` holds every member's edges (`GRAPH`) and each library's API
  namespaces (`NAMESPACES`); its docstring lists what else it refuses.
- **The tree closes clean**: `file_limits.py` green but the one exemption,
  `value/datetime/grammar.rs`, which P33 reconsiders; `module_graph.py`'s
  `OWED` empty and `ACCEPTED` the `origin` ↔ `remote` dispatcher alone;
  `concepts.md` claiming every production file once, with two **Not in one
  place** lines (a qualified name read four ways under "SQL lexing", the
  cache's refusal worded twice under "The cache"); the seam register empty,
  every finding of the closing read cut or kept.
- **The checks are meters and tests, not rustdoc.** Nothing runs `cargo doc`'s
  link check: a doc link from a lower crate into a higher one, or into another
  module's private child, does not resolve, so such a reference is a code
  span. `module_graph.py --docs` lists the links that run against the edges;
  the closing read found none marking a concept not standing on its own.

### The API and `internal`, for P36

- **Each library's API is its root and its namespaces; `internal` is what a
  crate above reaches that is not API**, a method so reached sitting on a
  `<Type>Ext` extension trait. The `internal` lists are P36's inventory of
  where the embedders' layer reaches past a designed surface; KD115, KD116 and
  KD117 are the edges already known, KD117 the one place a namespace's own
  file defines `pub` what `internal` re-exports.
- **The closing round changed public surface, as ruled**: one conversion
  across DataFusion's boundary (D74), one `datafusion_pgdump::install`, a
  registration's uncollated columns one `UncollatedColumns` finding, each mode
  enum's words stated once by `Display` and read in any case by `FromStr`,
  `GatherOptions` out of `ScanOptions` (D103), the density readings out of
  pgdt-model. P36 starts from that surface, not the pre-carve one.
- **The factory's entry listing shows a hand-set `Strict` as `default`**, the
  mode a scan reads it as; the query front ends refuse `Strict` through
  `PostgresInvalidValues::queryable`.

### Tests

- **A test in a lower crate cannot call a higher one**: a dev-dependency on a
  crate depending on the crate under test links a second copy of it, whose
  types are not the test's. Feature unification does this anyway to a unit
  test in pgdt-codec or pgdt-mapping, `pgdt-fixtures`' `mapping` edge linking
  their crate twice; nothing passes a type across today, and a test that
  wanted `sweep_request` there would meet the two copies' types unequal.
- **`check.py` follows `mod` lines only**, a library only dev-dependencies
  reach reading for every target that depends on it, so a fixture change runs
  every target of the five crates above `pgdt-fixtures`.
- **Kept duplicated, deliberately**: the differential tests' LCG in pgdt-codec's
  and pgdt-query's `differential.rs`; `kind_name` in pgdt-codec's `key/tests.rs`
  and pgdt-query's `predicate/tests.rs`, both exhaustive so a new kind fails to
  compile in both; and what the codec's families duplicate of one another —
  `value/libc.rs`'s two `strtod` models, a numeric lexer beside
  `NumericKey::parse`, `json_in` beside `parse_jsonb` — since merging one is a
  behaviour change. `persisted_index_is_pinned_to_the_format_version` hashes
  the label `"tests/data/edge_cases.sql"`, not the path it reads.

### Figures and the profile

- **Every build inheriting `release` is fat LTO in one codegen unit** (D105):
  the shipped `pgdt`, the `allocator` legs, the instrument, `profiling` and
  `cargo bench`, each rebuilding after a workspace edit in many times what the
  default leg takes.
- **Every register figure but `session-drift` and `release-profile` predates
  D105**, so the next sweep re-takes them under it. `--stale` cannot see a
  profile move: the stamp names the allocator and glibc, not a profile, and
  only `release-profile` declares the root `Cargo.toml`;
  `test_the_shipped_profile_is_the_fat_leg_restated` is what holds `release`
  to the fat leg.
- **The downstream-build rule passes on every workload at `ef442ad6`**, and no
  `#[inline]` is owed at a crate boundary. `strings` passes by one rep; a
  differential of it is possible only until the next sitting rebuilds
  `runs/pgdt-release-{default,fat}`, and after that needs a sitting of its own.
- **`vendor/xz-seek/VENDORED_FROM` still names `pgdump_query`**; the copy is
  never patched, and the next `vendor_xz_seek.py` sync rewrites it.

## Negative results

- **No split or merge was due.** Statistics apart from pgdt-query is not a
  crate, the edges running both ways; pgdt-codec's values group sheds
  dependencies but changes with the rest of the codec as often as alone;
  pgdt-dump's DDL grammar changed alone too rarely to split. Codec+query and
  dump+query fire "pub nothing outside the pair reads" and co-change high, and
  D74's reasons for both boundaries stand; dump's surface is what pgdt-mapping
  took over. `peak-rss` and `count-instructions` share a dozen lines of wrapper
  protocol and no commits, so the merge test is not met.
- **A design unread against the edges cycles.** 35.11's trees, drawn from
  names, cycled in every crate whose cut was still owed; a parent defining
  what its children read and calling them is a cycle. Projecting the moves
  (`module_graph.py --design`) before cutting is what removed them.
- **"Public before the carve" was the wrong proxy for API**, and "named by no
  embedder" sorted almost nothing out: the sort is by what an embedder of the
  crate would call given its `lib.rs`, and it moved ten items and was
  corrected twice (`ValueKey` API, the per-block re-read API).
- **What Rust forbids**: nothing at a crate's root can take a private module's
  name (E0255, E0428); a `pub(crate) use` cannot be re-exported `pub` (E0364),
  so a namespace's child is `pub(crate) mod`; a trait method taking
  `Option<Self>` needs `Self: Sized`. `cargo fix` leaves an unused trait
  import in place. Dedenting a moved test body by hand corrupts a multi-line
  literal; `cargo fmt` re-indents code alone.
- **Moving a persisted type between modules moves no cache byte**: bincode
  encodes no path and serde names fields, so no slice bumped
  `CACHE_FORMAT_VERSION`.
- **Cargo names no custom profile to a build script**: `PROFILE` reads
  `release` for every profile inheriting it, so `pgdt`'s `build.rs` reads the
  profile directory out of `OUT_DIR`.
- **`arrow::datatypes` is not `arrow-schema`**: it re-exports `arrow-buffer`'s
  `i256` and interval types, so the codec reached `arrow-buffer` before the
  carve.
- **rust-analyzer resolves nothing a package loads from outside its own
  directory**, which is why the fixture vocabulary is a crate rather than a
  `#[path]` include; `rust-analyzer scip` crashed on a closure whose return
  type rested on one.
- **Counting every `fn` was wrong for Rust**: with methods and test functions
  in, fourteen of thirty files were past forty; production functions alone
  named the files a reader would.
- **The profile readings**: a narrower wall-clock gap on `typed` between
  sittings was not the carve helping, the user-time gap holding; `parse`'s move
  since `2df93720` was not the seventh crate, rising in all three legs with the
  `dd` floor; thin LTO retiring more instructions than the default on two rows
  and fewer on two is unattributed, and no decision rests on it.
