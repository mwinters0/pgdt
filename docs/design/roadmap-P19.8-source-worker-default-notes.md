# P19.8 — the source states its own worker default

`--jobs` no longer falls back to a constant. `ByteRangeSource::default_workers`
is a defaulted recommendation, `LocalFileSource` inherits its **one** and
`XzSource` answers `available_parallelism()`; `ParallelArgs::resolve` takes the
open source and asks it whenever the flag was omitted. `DEFAULT_JOBS` is gone.
The mechanism is [`architecture.md`](architecture.md), "Execution model and API
surface".

## What the next slice inherits

**`resolve` takes a source, so every call site had to already have one.** All
four did — `open_for_scan` runs before `scan_options` in both scanning commands,
because a cache written for another file is refused before a byte is read. That
ordering is what makes a source-dependent default cost nothing: no extra open,
no extra `stat`, and on an `.xz` file the footer walk was already paid.

**The recommendation is a raw count and the divisor is untouched.** Nothing was
added to clamp `available_parallelism()` against the budget, because
`stream::worker_count` already divides every count — stated or recommended —
by the source's own per-partition footprint. A source reasoning about memory
here would make the budget bind twice. `19.13`'s budget rule therefore inherits
this unchanged: it moves what `memory_bytes` is, not how a count is spent.

**A worker default can be the source's; the byte reserve cannot**, and the
asymmetry is now written beside the trait method rather than only in the spec.
This method is downstream of recognition, which the CLI has already paid for;
`Parallelism::discover()` — `19.13`'s — is the primitive an embedder calls with
nothing open, so its reserve stays one constant taken from the compressed leg.

**Nothing in the library reads the method.** `resolve` is the only caller, so
an embedder's silence still means `Parallelism::default()` and the library still
spawns no threads unasked. That was deliberate: reading the recommendation
inside a scan entry point would reverse the library's own default and put a
source lookup in every one of them.

## The status line lost `(default)` on the compressed path, and `19.9` is where it comes back

A flagless `.xz` `parse` now prints `jobs=24 memory_bytes=67108864` — the budget
bare, where it used to read `67108864 (default)`. `Parallelism::Workers` has
nowhere to record "nobody stated a budget": its `memory_bytes` is a `u64`, not
the `Option` `Serial` gained in `19.4`. So the marker survives only where the
resolved arrangement is serial, which on a plain file is still every flagless
run.

This is not a defect to work around in `19.9` — it is the slot `19.9`'s
provenance work already owns. The line is to gain `(stated)`,
`(discovered: …)` and `(default: no limit found)`, and whatever carries that
distinction is what also carries "nobody asked" at a worker count above one.
Until then the manual says so in the user's own terms
([`../manual/dump-inspection.md`](../manual/dump-inspection.md), "Status on
stderr").

## What the manual now claims, and why it changed here rather than in `19.10`

`19.10` owns the manual pass for this phase, but three of its sentences went
false the moment this landed, and a falsified claim is corrected by the change
that falsifies it ([`../process.md`](../process.md), "Where does this fact
go?"). What moved:

- **`--jobs` defaults to 1** became "left unstated, the file decides", with the
  two answers named and a pointer at `scan started` for reading back which one
  a run got.
- **"Restricting the container's CPUs is not a substitute"** became "a partial
  substitute" — `available_parallelism()` reads the CPU quota (`RT7`), so a
  narrowed container *does* lower a flagless `.xz` run's worker count, and does
  nothing to a stated one.
- **The `koji.dump.xz` transcript** reads `jobs=24` with a bare
  `memory_bytes`, and the paragraph under it explains both halves.

`19.10` still owns the `MALLOC_ARENA_MAX` recommendation as `M76` leaves it,
both flags' help text, and the moved whole-block-decode threshold.

## Testing

**The default is pinned three ways, because no figure exercises it** — every
registered command shape states `--jobs`, which is the apparatus rule.

- `io.rs`: `a_source_that_does_not_advise_recommends_the_serial_path` reads the
  defaulted body through `BareSource` and asserts `LocalFileSource` inherits it;
  `an_xz_source_recommends_the_cores_it_was_given` asserts `XzSource` asks
  `std` rather than carrying a constant, against `available_parallelism()`
  itself rather than a literal, since the count is the machine's.
- `main.rs`: `stating_no_parallelism_flag_asks_the_source` drives `resolve`
  over a `Recommends(n)` double and pins precedence in **both** directions — a
  stated `--jobs 8` beats a serial recommendation, and a stated `--jobs 1`
  beats one of 24.
- `status_output.rs`:
  `an_xz_parse_defaults_to_the_cores_and_a_plain_one_to_serial` is the
  end-to-end leg, and it asserts the plain leg beside the compressed one on
  purpose — on a one-CPU runner the two counts coincide, and what the test
  pins is that they *can* differ and by which number.

`determinism.rs` needed no change: its reference leg already stated `--jobs 1`
rather than inheriting it, "so it cannot follow the default wherever that goes
next".

**`bytes` is now a dev-dependency of the CLI**, at `pgdump_query`'s own version.
`ByteRangeSource::read_range` names `Bytes` in its signature and the library
does not re-export it, so a test double cannot be written without it. Re-exporting
`bytes::Bytes` from `pgdump_query` is the other answer and is worth taking the
next time an embedder — rather than a test — needs to implement the trait.
