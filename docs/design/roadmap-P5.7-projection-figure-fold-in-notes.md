# P5.7 — Take the figure and retire what it replaces

What the next slice — and the phase wrap after it — inherits. Progress is
[`../status/STATUS.md`](../status/STATUS.md); what the phase committed to is
[`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md).

## What landed

`projection-widths` was taken and folded in, `composite-isolated` and its whole
apparatus were deleted, and every table in
[`measurements.md`](measurements.md) was replaced from one sweep, re-stamped at
`b70589f`.

The readings came from a detached pair (`runs/p57-sweep-20260830-1915/`), two
full sweeps of twelve figures back to back with a 120 s settle, both `rc=0`,
no figure failing in either. The published tables are sweep 1's; `session-drift`
is derived across the pair.

## The three things this fold-in decided rather than transcribed

**The `INSERT`/`COPY` per-byte ratio moved from 14.4× to 16.7×, and it is
carried as a magnitude everywhere it is quoted.** Both legs moved inside their
own drift envelopes — the warm `COPY` leg 0.537 → 0.500 s, the `INSERT` leg
7.71 → 8.37 — so what moved is a ratio of two warm absolutes, which is the
least stable thing a warm sweep produces. Five sweeps taken under a
witnessed-quiet apparatus now read 14.4×, 14.5×, 14.6×, 14.9× and 16.7×.
`architecture.md`, `roadmap.md`, `pg-dump-compatibility.md`, `STATUS.md`'s
`KD9` line and the P7 inbox all say "mid-teens" rather than a number now.
**Do not re-introduce a three-figure value for it** — the next sweep will move
it again and every one of those five documents will be wrong at once.

**`census-attribution`'s census-off gap changed sign, and the prose that
depended on the sign is gone.** It read −0.058 s under the previous stamp and
+0.036 s here, so "the gap does not merely vanish, it reverses" was a narration
of a reading inside the instrument's own resolution. What both stamps actually
support is the same claim: with the census off, 97% of the untyped baseline's
file-to-file gap disappears. The section now says that and names the sign flip
as reproduction rather than as a finding.

**`map-only` reproduced 4–14% faster, and that retired an apparatus note rather
than producing one.** The previous stamp took that table under a witnessed CPU
episode (12% machine-busy against ≤5% elsewhere) and said so in its own
paragraph. This sweep is ≤5% for every table and sweep 2 reproduces it within
1.0%, so the note is deleted. The consequence downstream is real: the map is
now **18.8 s of a 20.3 s** throttled 4000-block `parse`, leaving ~1.5 s of
cache rather than the ~0.2 s the distorted pair implied. `architecture.md` and
the P7 inbox both carried "all but a fraction"; both now carry "most", with the
remainder named.

## Deleting `composite-isolated` had to take its generator support with it

The spec asked only for the register entry. That is not a stable state: the
whole point of `measure.UNTAKEN` is that a built-and-unrun instrument is
**named** rather than latent, so a `--weak-composite` flag with no entry
pointing at it is exactly what that list exists to prevent. So the deletion is
the flag, the `composite_text` input, `run_composite_isolated`,
`_same_rows_diffs`, the `IsolatedPair` tests and the
`perf_generator_fidelity.rs` case pairing the two files.

**The evidence that this moves no published figure is byte-for-byte, and it was
taken.** All five inputs `generate_perf_data.py` still produces — control,
`--seed 43`, `--composite`, `--arrays --composite` and the 2 MB one-block
control — are identical at 20 MiB between `HEAD` and the working tree. That is
what `--verify-additive` computes, and it is what the acknowledgement below
should cite.

## Two obligations this slice could not discharge, both needing a commit

**`session-drift` reads stale and needs an acknowledgement naming the fold-in's
own sha.** `scripts/measure.py` is that figure's only declared path, and the
register move touches it. No executable line on any timing path differs, and
`--stale` already prints the softer resolution — the figure is *derived*, so
`uv run measure.py --drift <sweep1> <sweep2>` re-derives it from the two
`raw.json` files without measuring anything. Either resolution is honest; the
acknowledgement is the one the launch session planned, and it goes in
`scripts/acknowledged.py` (which no figure declares) in the commit *after* the
fold-in, because no entry can name its own sha.

**`generate_perf_data.py` is a declared path of nine published figures**, so
the deletion above marks all nine stale. `--stale` names them:
`census-brace-free`, `census-arrays`, `scan-throughput-cold`,
`scan-throughput-warm`, `nested-end-to-end`, `census-attribution`,
`cross-file-floor`, `per-block-quadratic` and `projection-widths`. The same
acknowledgement covers them, with `uv run measure.py --verify-additive` as its
`verified` command — which is the byte-identity check above, run by the harness
at 30 MiB instead of by hand at 20.

## Facts the wrap should not have to re-derive

**A `--filter` term's spacing was this slice's open question, and `P5.9`
answered it.** At the time of this slice everything after the operator was the
value, spaces included, so `--filter 'v_date < 2020-01-01'` looked for the date
` 2020-01-01`; the manual and `STATUS.md` were corrected to the unspaced
spelling and the spec was not, because a spec is not edited for anything but a
decision change. `P5.9` then made whitespace outside quotes not data, so both
spellings now mean the same thing — see
[`roadmap-P5.9-filter-term-grammar-notes.md`](roadmap-P5.9-filter-term-grammar-notes.md).
The wrap should carry `P5.9`'s rule, not this one.

**Both manual claims were executed, not reasoned about.** Projecting away a
`KD2` column (`(a,"{{1,2},{3,4}}")` in a `public.boxed`) turns a hard
`Error::FieldDecode` into a clean read, and `--filter 'v_date<2020-01-01'`
selects `-infinity`'s row and succeeds with `--column id` while failing to
build `Date32` without it. Both against real invocations of the debug binary.

**The warm resolution floor now has an observation outside it.** The standing
rule says a warm absolute compared across sessions resolves to no better than
~8%; this pair moved one file's warm tmpfs `dd` floor **13.6%**, three minutes
apart, in the fast direction. The rule is unchanged and now says explicitly
that it bounds warm *figures* rather than every warm reading, since the floors
move further — which is why each warm table co-measures its own. Whether the
sweep-disqualification threshold that reads off the same number should move is
the maintainer's call, and it is filed under "Decisions worth another look".

**`measure.UNTAKEN` is empty and its tests are vacuous.** `Untaken` in
`scripts/test_measure.py` loops over an empty list; the class docstring says so
and says why the cases are kept — an instrument gets registered there the
moment one is built, and a register whose checks went out with its last entry
acquires an entry with nothing holding it.
