"""The committed measurement harness: it runs the sweep behind
docs/design/measurements.md and emits that doc's tables.

Every re-take before this one was a throwaway script under runs/ that died
with the session, so each re-take re-derived the apparatus from scratch and
ended with a hand-written parser scraping medians out of a shell log --
which is where the transcription errors lived. This owns both halves: it runs
the measurements *and* emits the markdown, so folding a figure into
measurements.md is a paste rather than a transcription.

What it enforces, from that doc's standing rules:

* the timer is inside the container (`bash -c 'time ...'`), never around it,
  because `sudo nerdctl run` costs ~0.75 s and that is larger than several of
  these figures;
* a comparison table is re-taken **whole**, in one interleaved sweep -- each
  rep runs every file-and-mode combination in turn, and the second half of the
  reps runs them in the opposite order, because within a pair the second run is
  the warmer one;
* a warm figure is read from tmpfs and a cold one from the SSD with
  `drop_caches` before *every* run, including before the floor; a `cold-nvme`
  figure is the same discipline on the third device class, which is the only
  one where I/O and parse are within a small factor of each other and
  therefore the only one that can price a readahead or chunk-size default;
* every table carries its per-rep readings, so a wrong median is visible
  against the numbers that produced it (an SE from one sweep is not an error
  bar -- that doc's ninth rule);
* one libc: the default glibc build, in a glibc image pinned by digest, and
  the glibc each figure ran under named in the stamp or in its own marker.

**Selection is per figure, never finer.** One figure is exactly one table, and
a table is atomic -- half of one may not be re-taken. Figures that share a
reading say so and pull the other figure in rather than measuring it twice
(the allocator table's reference `parse` row *is* the warm scan-throughput
table's `COPY` row, and must be the same number).

**Each figure declares the paths that invalidate it**, so `--stale` can say
which figures a diff has made stale. That is the half a harness alone does not
fix: a one-line change to `map::Builder::on_row` invalidated two figures and
nothing announced it. **A section the register does not hold
declares one too** (`Outside.depends`), and its marker in the doc names the
commit its readings were taken at: being outside means the harness cannot
re-take them, not that nothing is told when they go wrong.

**Not every figure times `pgdt`.** `xz-decode-scaling` times the `xz_decode`
example instead, which reaches past the library to the decoder's own bulk entry
point -- the figure is about the decoder rather than about what the library
does with it.
The harness builds it (an *example* target, so the shipped binary is never
replaced), stages `.xz` inputs beside the plain ones, and gives that figure its
own container memory and its own contention row, both of which its table
declares. `inflate-throughput`'s inflate rows time the `gzip-inflate`
apparatus member the same way, built in the same image, over a `.gz`
compression of the control staged beside it, and `gzseek-backend` times
`pgdt-gzip-seek`'s `gz_read` example, one build an engine in a target dir of its own,
each asked its engine before it is timed, and `gzseek-spacing` the shipped
engine's build at each candidate spacing. The dynamic-filter figures time **`pgdt sql`**, DataFusion's CLI,
with `pgdt parse` building the cache it reads first: it states its worker
count as its session's `target_partitions` — held by a test and said in its
tables.

**No figure is a build of another tree.** A subtraction between two
builds measures everything that differs between the two trees, which grows
every time the tree moves and the pinned side does not; what one mechanism
costs inside a run is an attribution, read off a profile
(`docs/design/roadmap.md`, "Attribution is introspective; only the gate is
blind"), not a second build differenced against the first. The builds of
another tree publish nothing: `--count-pair`'s is of one change's parent alone
— the working tree's, or a named commit's — and prices one slice's change in
`instructions:u`, and `--count-span`'s is of a run of landed commits, read only
as the account the pairs on them must sum to.

Machine facts stay out of here: every path is an environment variable whose
default suits the machine CLAUDE.local.md describes, and the procedure lives in
CLAUDE.md.

**What another script owns is imported, never transcribed**: a generator's
columns, bands and writer versions, `release`'s build and image, `contained`'s
limit, `acknowledged`'s register. A value spelled again here would survive a
rename there as a failure in the middle of a sweep. Each is import-safe:
stdlib-only at import, everything executable behind its `__main__` guard,
zlib-ng imported only where a file is written through it.

**The package**, one-way from the bottom up, each module's own doc saying what
it holds (`module_graph.py` holds the first, `test_layout` the second):

* `host` -- running a command;
* `contention`, `machine_state` -- the machine a reading is taken on;
* `config`, `git` -- where things are, and what git says of them;
* `formatting`, `parsing` -- the numbers a table prints, and what a run printed;
* `inputs`, `runspec`, `shapes`, `memory_model`, `gzip_charge` -- what is run,
  over what, and what the library is modelled to hold while it runs;
* `builds`, `gating`, `figures.declaration`, `figures.stand_ins`,
  `figures.register`, `staging`, `preflight` -- the binaries, the register's
  declarations, and the inputs staged for them;
* `timing` -- the session that runs a sitting;
* `borrowing`, `figures.prose`, `figures.gate_table`, and a module per figure
  family -- how each figure is taken and rendered, `figures.runners` binding
  each to its id;
* `consumers`, `staleness`, `drift`, `emit`, `replay`, `check`,
  `verify_additive`, `count_pair`, `recipes` -- what the command line does
  with them (`scripts/measure.py`).

The order is described, not checked: declaring it as `layering.py` declares
D68's crate graph would hold the scripts stricter than a crate's modules,
which are held acyclic alone.

A figure family spans that order rather than one module: its command shapes in
`shapes`, below the session that runs them; its declaration in
`figures.register`, below the session and the stager that read it by id, with
what a dry run of its legs reports and the cache they read built ahead of them;
its taking and rendering in its own module above both, bound to its id in
`figures.runners`.
"""
