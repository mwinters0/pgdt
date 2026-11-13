# P33.28 notes — a row's scan reworked

**33.11's rise is reworked; 33.3.1's and 33.3.3's are placed and accepted;
the rework's pair resolves no rise.** The placements are 33.26's for
33.3.1's two parses and strings query (`runs/count-pair-33.26-20261112/callgrind/txt/`,
legs `sp` and `31t`) and this slice's for every other row each pair
resolves, on the pairs' kept legs
(`runs/count-pair-33.28-20261113/placement/`, `place.txt` the reading,
`gen.py` and `analyze.py` beside it; the legs in
`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.26/legs-33.3.{1,3}/`).
D108's falls are unplaced.

## 33.11, reworked

- **`CopyScanner` folded an `Option<LongestLine>` per row**
  (`LongestLine::fold`), testing whether the block had a longest yet and
  rebuilding the option on every row: 33.26's `CopyScanner::next_event`
  rise in `d096761c..0e51cc2b`, the same on every control row.
- **Now each row is measured by its length alone** (`CopyScanner::measure`):
  a row is never empty, so a `len` of zero is no row yet, and the longest is
  written only where a row is strictly longer, the earliest of the longest
  standing as D140 states. `CopyEnd::longest_line` and
  `CopyScannerExt::longest_line` still answer an option; the debug build
  asserts a measured row is not empty. `pgdt-mapping`'s
  `tests/longest_line.rs` holds every pass, ties and an empty block included.

## 33.3.1, placed and accepted

Each placement below reproduces on both rep pairs function for function;
the typed query's gate misses on one rep, mimalloc's paths moving between
runs, and its functions are the strings query's.

- **On the parses, `ChunkEvents::next_event` leaves the mapping pass's
  loop**: called out of line where the parent inlined it into
  `map_forward`'s closure, which falls by half what it gains. Changed code:
  `end_pass` hands out a streamed row as a third `PassEnd`, and the
  mapping pass no longer inlines the wrapper (the query's replay still
  does). The long rows show the same pair of functions, per chunk.
- **On every row, `CopyScanner::next_event`, and on the query rows the
  replay's loop that inlines the wrapper**, are the per-row path with its
  operations unchanged — `Endings::hold` lifted out whole, `streams_rows`
  written only at a header and a terminator — so register allocation of code
  33.3.1 left alone.
- **`memrchr` under stdout's line writer** (strings, typed and `arrays`)
  reaches no changed code: layout.
- **Accepted** (D108): what would undo the first is an inline attribute on
  the wrapper, or the streamed row handed out by `CopyScanner::next_event`
  testing for one on every row, which pays per row what the call costs; the
  second is the rework resting on codegen D108 refuses.

## 33.3.3, placed and accepted

33.26 named it by its pair alone; the row read it as
`ChunkEvents::next_event`'s, which neither leg's count bears out (that
function's count is the same in both). Placed on its own legs:

- **The parses**: `map_forward`'s loop, whose per-row code is unchanged —
  the change there is the chunk's read handing the census its fold,
  `next_chunk_folding` in place of `next_chunk`, once a chunk — so register
  allocation, accepted as 33.16's was; the two read closures trade places,
  per chunk. The long rows show the same, short of their gates.
- **The data-level parse's census**: `Counter::tier_of` rises by its
  `longest_counted` bound, asked of every counted field, against
  `BlockCensusExt::observe_row`'s restructure, which falls by nearly as much.
  Changed code, and the bound is 33.3.3's contract: a held field is counted
  as the head a streamed one keeps (`Kept::Head`) is. Accepted here, outside
  the scanner the row names; STATUS's "Decisions worth another look" carries
  the call.
- **The typed query**: neither rep pair reproduces it; layout (D108).

## Its count pair

**`169eaf6d` against the tree resolves a fall on the five control rows and
the `arrays` row, and neither way on the four long rows**
(`runs/count-pair-33.28-20261113/log`): a rise on none, so nothing is
placed, D108's falls unplaced, and the legs were not kept.

- **The control rows fall by about the same count**, the metadata-level
  parse most in proportion, as a row's test in place of its fold would:
  every row of the control passes the scanner, whatever reads it after.
  The `arrays` row falls by less, a different input.
- **The long rows do not resolve**: a cluster of 32 MiB lines holds few
  rows, so a saving per row is below their spread.
