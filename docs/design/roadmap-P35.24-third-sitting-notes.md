# P35.24 notes — the third sitting

What the wrap inherits from the `release-profile` figure's sitting on the
tree 35.12 left, taken at `ef442ad6` (`runs/measure-20261012T073914/`, log
and handoff under `runs/release-profile-35.24-20261012-0735/`) and published
as [`measurements.md`](measurements.md), "What link-time optimization buys
the shipped build", whose prose is the account. No code changed.

## What the sitting licenses

- **The downstream-build rule passes on every workload**
  ([`roadmap.md`](roadmap.md), "A downstream build is within 3% of ours"):
  `pgdt sql` and `typed` within 3% of `release-fat` in wall clock, `strings`
  and `parse` past it with the cells meeting. No workload failed, so the
  attribution clause owed no differential and none was recorded.
- **The spec's criterion holds on the tree the phase closes on**
  ([`roadmap-P35-seams.md`](roadmap-P35-seams.md), "Why now"): the default
  leg slowed on no shape across the four sittings, and every widening of its
  gap to fat is the fat leg moving. This figure owes no `#[inline]` at a
  crate boundary, `pgdt-mapping`'s on `parse`'s path included.
- **The count ranks the profile**: it tells thin from fat on every row, fat
  retiring fewer instructions on the three query rows, `pgdt sql` first, and
  the default more than fat on every row; D105 rests on it
  ([`decisions.md`](decisions.md), "D105").
- **The legs answered alike and none was killed**: every `pgdt sql` run
  returned one byte-identical answer, and no `memory.events` reported an OOM
  in the 512 MB container.

## What the wrap inherits

- **`strings` passes by one rep, a second sitting running**: here through fat's
  second rep, taken with the machine 8% busy, under the gate's limit. The
  count's gap on it is resolved, so a reading of it as failing would owe the
  remedy rather than layout's prose. A `strings` differential is possible only
  until the next sitting rebuilds `runs/pgdt-release-{default,fat}` (`cd
  scripts && uv run measure.py --profile-recipe`, its last part); after that
  it needs a sitting of its own.
- **Thin LTO retires more instructions than Cargo's default** on `pgdt sql`
  and `strings`, and fewer on `typed` and `parse`: unattributed, and no decision rests
  on it.

## Negative results

- **`parse`'s move since `2df93720` is not the seventh crate.** Its wall
  clock rose in all three legs with the `dd` floor and in system time; its
  user time and its gap to fat held.
- **The apparatus line's I/O stall disqualifies nothing.** It is the first
  two reps' container term, peaking around a run inside its cell; what it is
  made of is unattributed.
