# `P19.18` — the compressed path's resident account

What `19.16` and `19.13` inherit. The sitting itself is diagnostic and
publishes nothing; what it produces is a **name** for the term four earlier
slices carried as a residual, and the arrangement `19.16`'s constant is chosen
against.

## The apparatus, and the three things that changed in it

The figure was registered by `19.17` and re-steered before it was taken
(`roadmap-P19-efficient-defaults.md`, "Amended: the two paragraphs above
applied a figure's rule to a diagnosis"). Three changes landed here, all in
`scripts/measure.py`:

**The two allocator legs are dropped, not re-aimed.** They were registered to
separate glibc fragmentation from a block retained behind a live `Bytes` view.
That hypothesis died on arithmetic before this sitting ran, and the mechanism
that replaced it — glibc's dynamic mmap threshold — is one **no allocator leg
can test**: jemalloc and mimalloc do not have the threshold, so swapping them
removes the mechanism instead of measuring it.
`test_no_leg_of_this_figure_swaps_the_allocator` is what stops a later session
re-adding one, because the leg looks reasonable and its outcome is
foreseeable.

**The arena leg stays**, and it is the one mechanism leg. `MALLOC_ARENA_MAX=2`
bounds *how many* arenas can hold a retained block, which is the live
hypothesis stated as a count rather than as an allocator.

**A fifth family: the instrument legs** (`_reserve_instrument_specs`). The
flagless command shape on the `introspect` build, over `control_xz`, at every
container limit the axis carries, plus one capped leg at the mechanism limit so
that the black-box delta and the introspective one describe one arrangement.
They differ from the black-box flagless legs by `RunSpec.binary` alone, which
`key` carries, so neither can be read as a rep of the other.

`ensure_instrument_binary` builds it — its own target dir, `--features
introspect` on top of the shipped default set, `--version` read back through
`binary_instrument`. That last is the mirror of `binary_allocator`'s refusal
pointed the other way: there an instrumented build must not be timed, here a
leg that declares the instrument must carry one. A leg pointed at the default
binary writes no report, and `_read_instrument` can only report that as one of
three apparatus faults an hour in.

## What the renderer computes, and the two traps it is written against

**The two families are not commensurable and the account says so.**
`live_peak_bytes` is what passed through Rust's `GlobalAlloc`; every
`mallinfo_*`/`malloc_*` field is glibc's view of the whole process. `liblzma`
is the active `.xz` backend and allocates through C `malloc`, so its per-reader
dictionary — `XZ_DICT_BYTES`, 8,388,608 bytes, read off a heaptrack stack
rather than modelled — is **added back by hand**. A decomposition that
subtracted the two families without it charges 8 MiB a reader to retention,
which is the term the sitting exists to name.

**Every term is a high-water of its own**, so a row bounds any single instant
rather than describing one, and the last column is a residual of maxima. Said
in the emitted prose, because the arithmetic reads like an identity and is not.

**The fit is evaluated inside its own window.** `live_peak = fixed + readers ×
per_reader` is printed with its prediction and residual at the smallest
arrangement in its own data — the check nobody ran on the `403 MiB` intercept
that this phase spent three sessions on.

**The account ends on a name or an explicit no-name.** The criterion is
printed with the answer rather than applied silently: the remainder is *named*
where glibc's own `fordblks` covers at least half of it at every surviving leg,
and otherwise the emitted text says it has no name here, in those words, and
names `measure.py --heaptrack-recipe` as what would supply one.

## The check, and why its two halves are asymmetric

The black-box legs measure the same arrangement through `getrusage`, sharing no
mechanism with the report — which is the independence `19.15` and `19.18` never
had, having shared a fitting window.

- **Exact half:** whether the instrument build resolved the *shipped build's*
  `(jobs, budget)` pair. A different arrangement means the attribution
  describes a run the shipped build does not make, and nothing beside it can be
  believed.
- **Coarse half:** resident, against `INSTRUMENT_TOLERANCE_PCT`. It cannot be
  held to the shipped leg's own spread — this is a different binary, its
  `.text` is its own and every allocation goes through a counter — so the
  tolerance is stated rather than read off three reps of something else.

## What the sitting found

`runs/measure-20260911T214041`, `--alone`, **NOT PUBLISHABLE**, 3 reps,
nothing killed. Five instrument legs, four black-box limits, the arena leg and
the path step, on the arrangement `19.19` and `19.20` left.

**The program has no fixed term.** `live_peak = 4 MiB + 49.0 MiB a reader`
over the five instrument legs, and the fit predicts its own smallest
arrangement — 4 readers, 200 MiB — to within a megabyte. Every earlier fixed
term in this phase was a property of a fitting window that excluded the small
end; asked directly, the program says there is nothing there.

**The charge is right to 2%.** 49.0 MiB of Rust plus `liblzma`'s 8.0 MiB
dictionary is **57.0 MiB** a reader against the **58.0 MiB**
`XzSource::block_reader_bytes` bills. `19.14`'s divisor is not what makes a
flagless scan sit against its ceiling.

**What does is glibc retention, and it is named rather than inferred.**
`hblkhd` is **0** at every leg on runs that decoded 24 MiB blocks throughout,
which is the dynamic mmap threshold's signature and the opposite of what a
first-allocation-only reading would show; `fordblks` — freed by the program,
held by the allocator, still resident — is 102.5/169.5/367.1/412.7 MiB across
the four limits and covers 81–238% of the gap between the two high-waters. The
arena count tracks the reader count: 6, 15, 24, 26.

**The arena cap moves it, in the place the instrument predicted.**
`MALLOC_ARENA_MAX=2` at `512m` holds 316.99 MiB against the reference's
340.95 (−7%) black-box, and introspectively takes `fordblks` from 102.5 MiB to
68.2 with arenas 6 → 2. The two instruments agree on a mechanism, which is what
neither could do alone.

**The check passes at every limit.** The instrument build resolved the shipped
build's arrangement exactly — 4, 13, 22 and 24 readers — and its resident set
sat 0–3% from the black-box leg's. The two share no mechanism, so this is the
independence `19.15` and `19.18` never had when they agreed to 1% on a shared
fitting window.

## What `19.16` inherits, and what it must not do

**The reserve is a reserve for arena retention, not for a fixed program term.**
That is what the constant is covering, and it scales with the number of threads
that have ever decoded a block — so it rises with the count the allowance
affords, which is the count the allowance is being computed for.

**The thin cell in this sitting is `1536m` at 22 readers, 8.8% headroom** on
the 24 MiB leg, with `512m` at 33.2%, `1g` at 16.6% and `2g` at 29.0%. The
criterion `19.16` measures against is *worst rep leaves ≥20% of the limit*, so
two of the four cells fail it as shipped — the middle of the range, exactly
where `19.19` left it.

**Do not read a fixed term off the black-box fit.** It reports 129 MiB fixed
at 24 MiB blocks and 358 MiB at 128 MiB blocks over four and three legs, and
the introspective account says the program's own fixed term is 4 MiB. The
difference is retention that scales with the arena count, folded into an
intercept by a fit whose window starts at four readers.

## What is not answered, and what would answer it

**Where the retained bytes were allocated is not in this reading.** `fordblks`
is a total; the counter is blind to C and glibc cannot attribute. The one
instrument that can is heaptrack — `cd scripts && uv run measure.py
--heaptrack-recipe` — whose `--diff` between two of these arrangements names
the site rather than the term. It was not needed here, because the account did
not have to go below "retention"; it is what a slice bounding the retention
would start from.
