# P43 — The block model: identity, address and record

**A block of rows has an identity, an address and a record, and each is its own
type.** Today one struct, `CopyBlock` (`crates/pgdt-model/src/span.rs`), is all
three, and `InsertRun` beside it holds a table and a count and none of the record;
recording a block's rows is hosted once per pass that maps it, each opening the
block its own way. P8 needs a run carrying a record, P41 an address that is not a
file offset, P42 one naming a file, and P21 a record written for a block it reads
through the replay (`roadmap.md`, "P43 — The block model: identity, address and
record"); this phase separates the three and puts recording in one place, so each
of those starts from its part instead of carving it out.

**Behaviour-preserving apart from one cache-format bump** (`bump-cache-format`),
in 43.4. A shape that is messy because performance needs it is kept: every slice
touching the row path is priced by `--count-pair`, and a rise the pair resolves
that rework cannot remove keeps the hosts as they are, with that reason written
beside them (D108).

## Identity: `BlockId`, derived from the address

**`BlockId` is opaque, `Ord` in dump order, and derived from the address, never
persisted**: a private integer built only by `BlockAddress::id()` — plain format's
header offset, an archive entry's arm its position in the TOC. Every identity site
keys on it: the replay plan's maps, `EarlyStop`, `InCopyResume`, the back-fill's
`loaded` set, `summary.rs`'s edges and the `through`
sorts, each of which relies on dump order today. The replay recognises a block by
the id of the `CopyStart` it scans, with no lookup. Display sites (errors, notes,
`info`) print the address. Rejected: an ordinal, which needs an address-to-id map
to recognise a scanned block and a renumbering discipline on every append; the
address as its own key, which orders an enum across arms. `ResumeToken` is valid
only within its process, so nothing holding a `BlockId` persists it.

## Address and record: what `CopyBlock` becomes

**`CopyBlock { header, partition_root, address: BlockAddress, record:
Arc<BlockRecord> }`.** `header` and `partition_root` are what the block holds rows
*of* — in an archive both come from the TOC entry — so they sit beside the address,
not in it. `BlockAddress` is the four offsets, later an enum with an entry arm
(P41) and one naming a file (P42). `BlockRecord` is what a pass found:
`row_count`, `statistics` (its own `Arc` still, D34), `statistics_declined`,
`ignored_refusals`, `checked_in_full`, `census`, `recorded_rows`, `longest_line`.
**`LongestLine::offset` becomes block-relative**, as row groups and
`FieldRefusal::offset` already are, so the record carries no address and is
entry-relative under P41 as it stands.

A record is changed through `Arc::make_mut`, copied on write: the back-fill's
re-read, and `dump/settle.rs`'s census written over a metadata-level table's
blocks for one query. `Segment` holds `(header, address)` and no record, and
`TablePartitions` and `Table::read` under `Inheritance::Only` clone no whole
block; a reader of the census, `DynamicBlock`, takes the record's `Arc`.

**`InsertRun` holds a `BlockRecord` too**, its `row_count` moved into it and the
rest `None` until P8 fills it. A run's address — whether it gains a `data_offset`
— stays P8's, its span shapes resting on `KD1` (P8's inbox).

## One recorder, a held row split once

Every host hands a held row to `BlockCensus::observe_row` and then to the
observer's default `BlockObserver::observe_row`, each calling `split_fields`, and a
TimescaleDB catalog block's `RecordedRows::observe_row` splits it a third time; a
streamed row is split once, `RowFold` handing each run to the census's fold and the
observer's. **The recorder feeds a held row through the same field fold**, split
once, each field to every part recording it — D28's rule applied to the recording
path — the census's whole-row reject (`{`/`[` absent, no tested column) kept as
the gate it is today. No profile precedes it: the code answers whether a row is
split twice, and what sharing the walk costs is `--count-pair`'s to price (D108),
which refuses a sampled profile for placing a rise.

**`Recorder` is pgdt-model's, under `internal`, on the derived side**: what a pass
records of a block's rows is model's concept, read by every pass above it
(`concepts.md`, "What a pass records of a block's rows"; D74), and it names only
model and core. One type holding an optional census and its fold, an optional
observer, optional recorded rows and the row count. A held row is
`row(offset, raw)`; a streamed row is `begin_row`/`fold`/`end_row`, pgdt-dump's
`FieldFold` implemented for it in place of `RowFold`; an interior piece is
`piece()` and `absorb()` in file order; and `finish` is the shared half of today's
close — ignored refusals taken, `checks_every_field`, the observer finished, a
refusal answered — into a fresh `BlockRecord`. The longest line stays the
scanner's, handed over in `CopyEnd`.

**A piece's role is in the types, at both levels.** The observer contract splits
in two: `RowObserver`, the row methods and `into_any`, and `BlockObserver:
RowObserver`, adding `close`, `piece` returning a `Box<dyn RowObserver>` and
`absorb` taking one. `Recorder<O: ?Sized>` is generic over its observer, with
two instantiations, `Recorder<dyn BlockObserver>` and `Recorder<dyn RowObserver>`:
the row methods and `FieldFold` are on every `O: RowObserver`, `piece`, `absorb`
and `finish` on the block's alone, so neither a piece observer nor a piece's
recorder carries a close it cannot answer. A piece splits no further, the
checker's pieces included. Rejected: one trait, its piece answering the block
methods with a panic that a one-type `Recorder` would carry up a level; two
recorder types composing a shared row part, unless a piece's recorder turns out
to hold a field differently from a block's.

**The pass holds the recorder; `map::Builder` takes the record.** `Builder` loses
`on_row`, its `pending_*` fields, `count_block`, `observe_block`, `record_rows`
and `row_fold`, and `on_copy_end(end, record)` takes the finished `BlockRecord`:
it tiles spans and builds the block's address, its concept in `concepts.md`, "The
file map", and leaves the row path. A host with no line machine, P41's, records
with no builder.

## One open, one close, the passes differing by policy

**One open in pgdt-mapping** turns a block's header, its metadata and database,
and the pass's policy into a `Recorder` — the counter built, the observer
attached, `records_rows` asked — and derives whether the block streams and whether
the interior is offered. `map_forward`'s `CopyStart` arm becomes it. Each pass
states its policy as data: the forward pass censuses by level and attaches
`observer_for`; the eager pass censuses, attaches no observer and stays serial;
the back-fill censuses always, attaches `observer_tracking` or `checker`, records
no rows (D133 records them on the passes that map) and offers the interior first.
Those differences are each pass's business; only the mechanism was written three
times.

**One close.** The forward and eager passes hand `finish`'s record to
`on_copy_end`; the back-fill checks extent, row count and longest line against the
record it holds (`CachedBlockChanged`), then merges through one `BlockRecord`
method holding the merge rule, the account's crediting staying the back-fill's,
the account being mapping's.

**The observer contract closes in one call.** The recorder's close calls
`take_ignored`, `checks_every_field` and `finish` on `BlockObserver` in sequence;
the contract's close becomes one call returning what the record takes. Behind it,
**the `Gatherer` splits into the three it is today by `piece` and `gathers`** — a
block's gatherer, a piece's and the strict checker — so the checker carries no
placeholder `Sizing`, no method gates on `!self.gathers`, and what each holds is
charged on assignment rather than by a `charge_held()` after each; `absorb`
recovers its piece's own type. Absorbed from `M303`, withdrawn unlanded, so the
contract and its implementations are reshaped once.

**The mapping pass takes one context.** Every function `pgdt-mapping` holds under
`clippy::too_many_arguments` that takes a pass field takes one pass context —
source, scan options, ledger, cancel, cache, watch, extent, statistics request,
allowance, account — which the open takes too; one taking none keeps its
parameters, and a later slice's reshape drops its allow only where it removes the
need. And `forward.rs`'s cancel-save-return and `whole_file.rs`'s
interrupted exit are each written once. Absorbed from `M302`, withdrawn unlanded,
its `CopyStart` arm being the open. D126 and D15 keep `FirstPass`'s sync and async
serve loops apart, and this phase does not reopen them.

## The eager pass is the fixtures' oracle

**`build_index` and `build_map` move to `pgdt-fixtures`, behind `mapping`**: the
eager, serial pass `map_file` is held to, called by tests alone, and no part of
`pgdt-mapping`'s designed surface (D106). **`backfill_block` goes with them**, on
the same test: one block re-read outside any pass, called by tests alone, and
unable to persist what it reads, being handed no cache. A fixture, it reads under
a `ReadContext` as the pass's readers do. `eager_pass` opens each block through
the one open under the eager policy and reaches `attach_text` and
`derived_diagnostics` through `pgdt_mapping::internal`. It stays an independent
oracle — serial, whole-file, no interior, no resume, no saves — sharing with the
passes it checks only the recorder and the open, the mechanism under test.

## `pgdt-model` separates what it persists from what it derives

A `persisted/` module tree inside `pgdt-model` holds the serde shapes and nothing
else, so an edit there is the edit that asks whether `CACHE_FORMAT_VERSION` bumps;
the contracts (`BlockObserver`, `CensusFold`, `FieldCount`), the walks over the
catalog, the diagnostics and TimescaleDB's recipe sit beside it, an impl on a
persisted type allowed outside the tree. A module split, not a crate: D68 needs
no new crate, and the pin tests already catch a bump a fixture shows. The crate
doc's claim that any edit in it asks the bump question becomes true of the tree.

## Where it lives

| Part | Crate, module | Concept (`concepts.md`) |
|---|---|---|
| `BlockId`, `BlockAddress`, `BlockRecord` | pgdt-model, `persisted/` | "The file map", beside `span.rs`'s shapes |
| `Recorder` | pgdt-model, derived side, `internal` | "What a pass records of a block's rows", beside `CensusFold` |
| `FieldFold` for `Recorder` | pgdt-dump, where `RowFold` was | "The scanner": the carry's interface |
| The open, its policies, the pass context | pgdt-mapping | "The mapping pass" |
| `eager_pass`, `build_index`, `build_map`, `backfill_block` | pgdt-fixtures, behind `mapping` | "The fixture vocabulary" |

## What the phase does not own

- **An `INSERT` run's address and rows**: P8's, and `KD9`'s cuts with them.
- **`KD173`**, at one of the replay's batch emits: `M300`'s, which reworks those
  emits; P43 changes how the replay names and segments a block, not its draw.
- **`KD107`**, the first refused field merged across reads: it moves with the
  merge into `BlockRecord` and stays `(c)`, the phase changing no behaviour.
- **`KD5`**, a map rebuild cloning the whole span list: a record behind an `Arc`
  lightens each clone and leaves the order as it is.

`M299`, `M300`, `M301` and `M305` block P43 and land ahead of its slices;
`M304` does not, the seams round its meter queues being over `measure.py`, the
instrument every slice here prices with.

## Slices

Ordered so each makes the next one's mistakes visible: the moves with no
behaviour first, then identity, then the shape every later slice records into,
then the row path one mechanism at a time, each priced on its own.

1. **43.1** `pgdt-model`'s `persisted/` split: a move, no serde change, no bump;
   the concept map's claims follow.
2. **43.2** The mapping pass context, and the two interrupted exits each written
   once. Off the row path.
3. **43.3** `BlockId` at every identity site, derived from the header offset;
   display sites print the address. No format change.
4. **43.4** `CopyBlock` as header, partition root, address and `Arc<BlockRecord>`;
   `LongestLine` block-relative; `InsertRun` holding a record; the back-fill and
   `settle.rs` copying on write; `Segment` holding header and address, nothing
   cloning a whole block. The phase's cache-format bump. `--count-pair` on the
   typed query.
5. **43.5** The observer contract's one-call close, and the `Gatherer` as three
   types. `--count-pair` on a data-level parse.
6. **43.6** The `Recorder`: a held row split once, `RowFold` replaced, the three
   hosts fed through it; the observer contract split into its row and block
   traits; `Builder` recording nothing, `on_copy_end` taking the
   record. `--count-pair` on a data-level parse.
7. **43.7** The one open under per-pass policies, and the one close, the
   back-fill merging through `BlockRecord`; `map_forward`'s `CopyStart` arm
   becoming the open. `--count-pair` on a data-level parse.
8. **43.8** `build_index`, `build_map` and `backfill_block` in `pgdt-fixtures`,
   the eager pass opening through the open under its policy.
