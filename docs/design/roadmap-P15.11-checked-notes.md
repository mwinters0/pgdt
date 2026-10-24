# P15.11 notes — single-member gzip, a read behind the first pass's frontier checked

A read behind a gzip first pass's frontier is compared as the indexed source's
piecewise arm compares a unit (`KD132` struck).

- **`first_pass/pass.rs`'s decode behind is one of two kinds**, `Behind`:
  inside a closed unit — one whose end the index already holds,
  `gz_seek::Index::unit` answering it — the unit alone, a
  `gz_seek::UnitRead` begun from its checkpoint, so its plaintext is compared
  against the CRC-32 the unit recorded and its end against where the index
  places it; inside the open unit, the forward decode resumed at its
  checkpoint it always was, decoding the frontier's own bytes from the
  window the frontier began from. A read running on past a closed unit's end
  begins the next unit's decode, the open unit's forward one at the last.
- **Compared when the decode leaves the unit**, as `LiveBlock` is: reaching
  its end, giving way to a decode a read elsewhere begins (`Pass::leaving`,
  drained before the new decode is begun, so the pass never holds a third
  decode), or `FirstPass::complete_reads`, which now leaves the decode
  behind before asking the indexed source's. So a read's bytes go out
  before their unit is compared, and the refusal lands on the read that
  leaves the unit or on the run's completion. Both feeds drive the leave
  through the same loop as a read (`Job`), the fetched one fetching between
  decode steps.
- **The charge is unchanged**: a unit decode holds less than the forward
  decode `DECODE_BYTES` prices, so `PASS_BYTES` still bills two.
- **The evidence**: `first_pass/tests.rs`'s
  `a_read_behind_the_frontier_is_checked_against_its_units_crc` (both feeds,
  over a resumed index whose provider damages one closed unit's window: a
  read leaving it refused as `gz_seek::Error::UnitCheck`, one stopped inside
  it refused once by `complete_reads` and once by the read that leaves it for
  another, an undamaged closed unit and the open unit read correctly); on the
  tree before this slice that read handed out the damaged bytes. And
  `a_read_behind_the_frontier_reads_from_its_checkpoint`, now reading across
  a closed unit's end and from the last closed unit into the open one, its
  `complete_reads` passing.

## What later slices inherit

- **15.8's speculative pass** closes units the behind decode reads through
  `gz_seek::Index::unit` on the index `Pass::index` answers: units found
  out of order must reach that index only once their CRC-32 and end are
  known, or a read behind would begin a decode with nothing to compare.

## Negative results

- **Comparing the resumed forward decode's own CRC-32s** at each checkpoint
  it places was not taken: it compares only at a unit's end, so a decode left
  mid-unit — a read stopped there, or one elsewhere replacing it — compares
  nothing, and the forward decode checks no placement against the index.
- **A closed unit decoded whole before its bytes go out**, as the block arm
  does, was not taken: it holds a unit's plaintext the pass's charge does not
  name, where the first pass is the piecewise arm's shape, one partition
  through one decode kept across reads.
