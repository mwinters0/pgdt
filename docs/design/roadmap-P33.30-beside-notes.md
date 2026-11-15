# P33.30 notes — the floor takes off what stays beside the line

**`ArrangementRequest` tells its holdings apart**: `statistics`, `granted`
and `lines`, summed by `held()`, which is what the count is solved against
and what the statistics' draw opens at, as the one `held` field was. The
floor then takes `statistics + lines` off what one reader is left, under
every found allowance, `pgdt`'s uncarved estimate included, and
`Floor::beside` carries them into `LineOutgrowsMemory`, whose message names
them where they are not zero (D161). `ScanBudget::draw` passes its billed
maps as `statistics`, the pool's limit as `granted` and the live scans'
carved lines as `lines`; every `pgdt` pass passes its statistics heap as
`statistics`.

## What the next slices inherit

- **The floor is `max(leaves, room) − beside`**, the room's guarantee taken
  before the subtraction. The ledger lends a line at most `room − held()`,
  its statistics' draw opening at all three, and `beside ≤ held()`, so no
  line the ledger lends room for is refused (`arrangement/tests.rs`'s
  `the_floor_takes_off_what_stays_beside_the_line` asserts it).
- **A mapping pass's statistics are the ones its cache handed it**, fixed at
  the carve; what its account grows to afterwards moves the ledger's
  statistics' draw and never the floor, so a parse of a cache a back-fill
  grew refuses more, the same each run.
- **A provider scan's floor depends on what else the session runs**: the
  billed maps and every live scan's carved line, as its count already did
  (`budget.rs`'s `a_draw_s_floor_takes_off_what_stays_beside_its_line`); a
  join's second scan may be refused a line it would read alone, first
  planned, first served, as `KD38` states of its budget.

## Negative results

- **The grant still lowers the count**: it stays in `held()`, so a pool's
  limit costs readers and opens the statistics' draw as before; only the
  floor ignores it.
- **No count pair**: nothing on a row's or a byte's path changed (D108's
  trigger is per row or byte).
