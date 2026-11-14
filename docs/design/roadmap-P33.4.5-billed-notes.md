# P33.4.5 notes — a live scan's line charge is billed to later scans

**`ScanBudget` holds each live draw's `line_charge` until its `Draw` drops,
and every later draw is carved with their sum in `ArrangementRequest::held`**,
beside the billed maps and the pool's limit (D160). It comes off the margin's
ceiling alone and opens the later scan's statistics' draw, so that scan's
ledger lends none of the bytes an earlier scan's longest line was carved
around. `BudgetAccount::lines` names it, and a budget-quoting plan note's
clause says "carved … byte(s) for their longest lines" beside what the live
scans drew; `pgdt-sql.md` quotes the clause and says so under "Memory".

## What the next slices inherit

- **A billed line lowers a later scan's count first**, as every holding does
  (`Parallelism::within_shared`): what the ceiling cannot absorb at one reader
  comes off that reader's budget. A later scan at the count's floor is then
  not guaranteed room for its own longest line; placing it under the cap at
  one reader is 33.5's floor, which reads this same room.
- **Under no allowance found the charges are billed and change nothing**:
  the holdings come off no margin there and the ledger is without limit.
- **`pgdt query` is untouched**: one process runs one replay, whose
  arrangement already carves its own line.
- **33.4.6 lowers what is billed** wherever it lowers `Table::line_charge`,
  since the draw bills exactly the charge it carved.

## Negative results

- **Only the longest line is billed, not the ledger.** An earlier scan's
  ledger was sized at its draw, so lines it holds beside its longest still
  lend from a room a later scan's budget and ledger now also count. That is
  the slack a carved first scan leaves, first planned, first served (`KD38`);
  one ledger per session was refused for it (D160).
- **No count pair**: nothing on a row's or a byte's path changed.
- **The test is a unit test of two draws** (`budget.rs`'s
  `two_live_scans_ledgers_lend_no_byte_twice`, failing with the billing
  removed); no join over two dumps reads it end to end.
