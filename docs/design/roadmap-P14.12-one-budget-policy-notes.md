# P14.12 — One budget policy, held by both `.xz` sources

What landed: `io::XzBudget`, the value `XzSource` and `FetchedXzSource` each
hold one of. It carries the two pools, the block unit, the decoder charge and
the two numbers a caller announces, and answers `apportion`,
`charged_chunk_bytes`, `block_worker_memory`, `block_path`,
`block_decode_bytes` and `partitions` for both. No behaviour moved: every
number, arm and refusal is the one that was there before, and the only tests
that changed are the ones naming a field's path. The spec's "D21" holds the
reasoning; no `D<k>` was added.

**The premise was re-tested before anything was written.** The six methods were
compared body for body with comments and whitespace stripped, and are identical
in both sources — which is what the row assumed and what makes this an
extraction rather than a merge.

## What the rest of the phase inherits

- **Composition, with no branch anywhere.** Nothing in `XzBudget` asks which
  provider it is under. The transport reaches it only as the `decode_bytes` its
  constructor is handed, so the two sources differ in that one argument and in
  nothing else the policy does.
- **`decode_bytes` is stated by each constructor, and so is its rationale.**
  `XzSource::over` passes `Layout::decode_footprint` and says there why the
  compressed input chunk is in it; `FetchedXzSource::assembled` passes
  `decoder_bytes` plus this file's widest window and says there why the window
  is charged rather than booked as unpooled. The field's own doc says only that
  it is the one term the transport moves.
- **Three more copies went with the six.** `hint_read_size` and
  `hint_parallelism` could only become delegations once the fields moved, and
  `BlockCache::for_table` — a seventh identical line the row did not name — is
  now inside `XzBudget::over`. Each source's trait body for the two hints is one
  line.
- **The `KD21` and `KD24` markers moved with their methods**, staying in
  `io.rs`, so the index lines are untouched and `deficiencies.py` still resolves
  one marker apiece.
- **`scripts/test_measure.py` pins the composition site by its source text.**
  `test_the_recommendation_and_the_affordability_charge_are_one_number` asserts
  the literal body of `default_worker_memory`, which now reads
  `self.budget.block_worker_memory()`; the property it pins — the charge the
  rule solves an allowance against is the charge the gate compares a budget to —
  is unchanged.

## Negative results

- **The table stayed out of the policy.** `partitions` takes a
  `&xz_seek::SeekTable` rather than the value holding a third alias of it: the
  cut points are a property of the file, which both sources already alias, and
  what this policy decides is only which arm they are priced for. Holding it
  would have read as the budget owning the file's shape.
- **No accessors.** `XzBudget` is module-private, so the two sources and the
  unit tests read `budget.pool`, `budget.blocks` and `budget.decode_bytes`
  directly. Wrapping them would have been boilerplate over a boundary that does
  not exist inside one module.
- **`hint_wait_policy` is not on the value**, being the one announcement the
  two sources genuinely answer differently — the local one sets the chunk pool's
  policy, the fetched one refuses every wait for where it obtains its buffer.
  Moving it would have put a provider difference inside the value this slice
  exists to keep free of one. `default_workers` stays out for the same reason.
