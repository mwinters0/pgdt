# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

See `README.md` for a project overview and documentation pointers.

## Commands

```sh
cargo check --workspace                          # type-check everything
cargo build --workspace
cargo test --workspace
cargo insta review                                # accept changed snapshots
cargo clippy --workspace
cargo fmt --check                                 # config: rustfmt.toml
cargo run -p pgdump_query-cli -- parse --source <file>       # binary is `pgdq`; the only scanner, resumes
cargo run -p pgdump_query-cli -- info --source <file> [--verbose]   # never scans; reads the cache
cargo run -p pgdump_query-cli -- info --dqcache <path>       # cache-only, no dump file needed

cd scripts && uv run generate_fixtures.py [--version 13|16|18]  # regenerate fixtures/
cd scripts && uv run generate_fixtures.py --skip-dumps          # re-take the comparison oracle only
cd scripts && uv run generate_fixtures.py --skip-dumps --skip-oracle  # re-take the ADBC floor oracle only
cd scripts && uv run python -m unittest test_adbc_floor  # the committed ADBC floor files
cd scripts && uv run python -m unittest test_comparison_oracle  # case table vs. the committed answers
cd scripts && uv run oracle_differences.py        # where two adjacent majors disagree, vs. the committed file
cd scripts && uv run oracle_differences.py --write  # re-file it after regenerating an oracle
cd scripts && uv run python -m unittest test_oracle_differences  # the differ's own tests
cd scripts && uv run oracle_register.py           # register arms vs. oracle cases, both ways
cd scripts && uv run python -m unittest test_oracle_register  # that check's own tests
cd scripts && uv run floor_mapping.py             # ADBC floor rows vs. `builtin_scalar`, both ways
cd scripts && uv run python -m unittest test_floor_mapping  # that check's own tests

cd scripts && uv run measure.py --list            # every figure, and what invalidates each
cd scripts && uv run measure.py --stale           # which figures a diff has made stale
cd scripts && uv run measure.py --check           # figure markers vs the doc, and each figure's consumers
cd scripts && uv run measure.py --verify-additive # inputs regenerated at two revisions, compared byte for byte
cd scripts && uv run measure.py --figure <id>     # re-take one figure — one whole table
cd scripts && uv run measure.py --all             # the whole sweep: ~1 h, detach it
cd scripts && uv run measure.py --profile-recipe  # the sampling-profile sequence, printed; minutes, not detached
cd scripts && uv run python -m unittest test_measure   # the harness's own tests

cd scripts && uv run deficiencies.py              # deficiency register: index vs. detail entries vs. code markers vs. the slice pairing vs. the roadmap's phase index
cd scripts && uv run python -m unittest test_deficiencies  # that check's own tests
```

## Long-running processes (>10 minutes)

**Never wait on them, and never set a monitor or a completion notification
for them.** Waiting past ~10 minutes expires the prompt-token cache, so the
whole conversation is reloaded on the next turn — several hundred thousand
tokens of cost for a result that a later session could have read for free.

So, for anything expected to take more than 10 minutes:

- Launch it fully detached, with stdout/stderr redirected to a file under
  `runs/` (gitignored), along with whatever exit status/timing the reader
  will need.
- Do **not** arm a `Monitor`, a background `wait`, or any other mechanism
  that notifies on completion.
- Treat the result as a **future session's** input. Record in
  `docs/status/history/<today>.md` what was launched, the log path, and what
  the next session should check.
- If nothing else can proceed until it finishes, wrap the session up —
  including all doc updates — rather than idling.

A full `scripts/measure.py --all` sweep is one of these: ~18 GiB of generated
input and roughly an hour of runs. Detach it, let it write
`runs/measure-<stamp>/`, and let a later session read `tables.md` there.
**Nothing else may build or test while it runs** — a `cargo` job across 24
cores moves the very numbers it is taking, which is the same rule as
`measurements.md`'s "a koji figure taken while local work ran is not a figure".
Start it with `setsid` and stop it by **process group** (`pkill -g <pgid>`):
killing the harness alone orphans whichever generator it had running, and that
generator keeps writing a multi-gigabyte file.

Running `pgdq` against the multi-hundred-GB koji sample (see
`CLAUDE.local.md`) is exactly this case: a full scan is roughly an hour on
the HDD. It goes in a memory-limited container, and — because a scan's
throughput is a performance figure — on the **default glibc build in a glibc
image**, per `docs/design/measurements.md`'s standing rule that the allocator
is part of the apparatus. A host-built binary runs in `postgres:16`. **There is
no musl recipe here any more**: only glibc is measured, so a static musl build
is an untested configuration and an untested portability claim is worse than
none. Let the container write the log.

**The invocation is not written out here.** `cd scripts && uv run measure.py
--koji-recipe` prints it with every path filled in, and `--koji-recipe --wrap`
prints the stop-report-resume-compare sequence. It lived in three
hand-maintained copies until the harness took it, which is how a documented
command was found that could no longer execute. The harness prints koji's
recipe and never runs it.

Two of its details are load-bearing and easy to lose again; the third is the
512 MB cgroup, which is part of the apparatus. `scripts/test_measure.py`
asserts all three:

**`exec` is load-bearing, not style.** It makes `pgdq` PID 1, so a later
`nerdctl stop` reaches the interrupt guard. Leaving `sh` in front — which
`; echo "exit=$?"` forces, since a compound command cannot be `exec`'d — makes
`sh` the signal's recipient, and it does not forward: the runtime's `SIGKILL`
follows and the guard never runs. Read the exit code from
`sudo nerdctl inspect -f '{{.State.ExitCode}}' pgdq-koji` instead, which
reports it whether the run finished or was signalled.

**Pass `--dqcache` into the mounted `/out`.** The dump is mounted read-only,
so the colocated default (`/dump.sql.dqcache`) lands in the container's
ephemeral writable layer and is destroyed with the container — throwing away an
hour of scanning without an error, since the write itself succeeds.

A later session reads `runs/pgdq-koji-scan.log`; `sudo nerdctl inspect -f
'{{.State.Status}}' pgdq-koji` says whether it is still going.

**Stopping one is safe.** `sudo nerdctl stop` sends the image's stop signal,
which `parse` catches either way: it saves everything scanned so far to the
`--dqcache` path and exits by signal, and re-running the same command resumes
from there. So a scan that has to be cut short costs the block in flight, not
the run. Note the `postgres` images set `STOPSIGNAL SIGINT`, so `nerdctl stop`
gives exit **130**, not 143. **`--stop-signal SIGTERM` on `nerdctl run` does
not change that** — it is accepted and then ignored; the container's
`io.containerd.image.config.stop-signal` label still reads `SIGINT` and
`nerdctl stop` sends what the label says. To exercise the `SIGTERM` path, send
it directly: `sudo nerdctl kill -s SIGTERM <name>` (verified: exit 143).

**A wrap-scale verification run is stop-report-resume-compare**, in one
detached script: start the parse, signal it partway, report the interrupted
cache (`pgdq info --dqcache <path> --verbose`) and check it comes back typed,
then resume the same command to completion and compare block/row/byte counts
against the previous full run. That sequence is what buys the interrupt guard's
only real-scale test — a signal inside a hundred-gigabyte block, against a cache
already holding dozens of completed ones — which no fixture can construct. The
script itself is a `runs/` artifact, not a `scripts/` one: it hardcodes one
machine's dump and nothing in the repo consumes its output.

**Never edit a `runs/` orchestration script while it is running.** `sh` reads
a script incrementally, so an edit mid-run shifts the byte offset it is about
to read from and can execute garbage. Let it finish, or kill it first.

## Architecture & design docs

`docs/design/architecture.md` describes how the built system works, filed by
subject. **Read the section for the mechanism you are touching** — the scanner,
the file map, `DumpIndex`, the preamble grammar, type resolution, the decoders,
the zero-copy Arrow path, the query passes, the cache, the CLI, fixtures, the
comparison oracle, or the testing approach — before changing that mechanism.
Each section carries its own *Rejected:* paragraphs, which are the part that
cannot be recovered from the code, **and that mechanism's known deficiencies** — a register entry's
detail lives beside the mechanism, not in a central list, so reading the section
is how you meet it. Two sections are hard constraints rather than description: "Parser
robustness requirements (hardcoded)" is what the `COPY`-block scanner
(`scan.rs`, `copy.rs`) implements, and "`Event` is the scanner's contract"
names every site that must change when a scanner event is added.

`docs/design/layering.md` assigns every module to one of four layers and states
the rules that keep dependencies pointing downward. **Read it before adding a
module, moving code between modules, or wiring a concern across existing
ones** — it is a standing constraint, and it pre-answers where new code goes.

`docs/design/roadmap.md` holds the project goals, the standing rules that cut
across all work, and the index of phases still ahead. **Read its "Standing
rules" before making a design decision that a later phase inherits**; read the
phase section before grilling or specifying that phase. Its "Out-of-band work"
section is the ledger for work that belongs to no phase — **add a one-line row
there when landing a change that changes no spec'd decision and fits one
session**, pointing at the history entry that says why. Such a change gets no
spec and no notes doc. If it would change a decision, it is not out-of-band:
grill it, amend the spec, and give it a slice number. `M<k>` numbers work like
`P<k>`: allocated on discovery, never reused, and the ledger's row order is
allocation order rather than landing order. The rows themselves are struck at
each keystone; that section's watermark says which numbers are already spent,
so the next item takes the number after it.

**A phase is identified by `P<k>`, which is not a position.** Phases are
allocated numbers as they are *discovered*, run in whatever order suits, and
sometimes two at a time; the schedule is the roadmap's index table, top to
bottom. So: never renumber a phase, never reuse a struck one's number, and
never drop or add the `P` to signal that a phase has matured — an identifier
that changes makes every history entry and commit message naming it wrong. The
slug beside it is an informal caption, free to change and free to repeat a
slug some earlier phase used. Full rule: `docs/process.md`, "Phase identity is
`P<k>`".

A phase that has been specified gets its own doc,
`docs/design/roadmap-P<N>-<slug>.md`; keep that convention when a new
phase's plan is written. `docs/design/roadmap-P7-scan-performance.md` is
the performance design for the local-file read path — read it before touching
the batch layer or the cache format, which it constrains ahead of its own
phase. **No phase is open right now**: the next one is grilled and specified
before any of its code is written, and step 6 of `docs/process.md`'s loop
re-grills the roadmap first.

`docs/design/roadmap-P<N>-<slug>-inbox.md` holds facts an *earlier* phase found
that phase N will need — filed by destination, because a notes doc filed by
origin never gets read at the right moment. Two triggers, and the second is the
one that decays: **read a phase's inbox before grilling or specifying it**, and
drain it (fold each entry into the spec, then delete the file) as part of that
grilling; and **file into one whenever a slice turns up a fact a phase with no
spec yet will need** — at the moment you find it, not at wrap. An entry is the
fact, why that phase cares, and where it came from; if you can't name why that
phase cares, it isn't an inbox entry. Most forward-looking remarks belong
somewhere else instead — see `docs/process.md`, "Inboxes: facts filed by
destination", for the threshold.

`docs/design/postgres-invariants.md` is the evidence layer beneath everything
else: each entry is a property of `pg_dump` output that a design decision
treats as guaranteed, with the source that proves it and how to re-verify it
when a new PostgreSQL major lands. **Add an entry whenever a decision starts
depending on `pg_dump` behaving a particular way**, and walk the file when a
new major is released. `architecture.md` cites its `I<n>` numbers throughout.

`docs/design/measurements.md` holds every performance figure the design relies
on, each with the command that reproduces it. **Read it before making a
performance claim, and add to it rather than to a notes doc when you measure
something.** A figure whose regeneration command is gone should be deleted, not
kept.

`scripts/measure.py` is the harness that takes those figures and emits that
doc's tables. **Run it rather than writing a one-off script when a figure needs
re-taking** — every re-take before it was a `runs/` script that died with the
session, so each one re-derived the apparatus from scratch and ended with a
throwaway parser scraping medians out of a log, which is where the
transcription errors lived. Its unit tests are `scripts/test_measure.py`.
**Run `uv run measure.py --stale` before claiming a figure still holds**: every
figure declares the paths that invalidate it, so the harness answers "which
figures did this diff make stale" instead of someone remembering to — which is
the half that failed twice. A declared path is coarse, so a change inside one
that provably moves nothing still reads stale; **acknowledge that commit where
mechanical evidence exists, and where it does not, leave the figure red with
the reason written down** — what must never happen is red with no explanation,
because a signal that is always on is no signal. Two oracles are mechanical:
byte-identity of the regenerated inputs (`--verify-additive`, generator changes
only) and **reachability** — a change no registered command shape executes.
Neither a stale figure nor a phase boundary obliges a sweep: a full sweep is an
hour of a quiet machine, and it belongs to a phase that is about performance,
not to every wrap.
`--verify-additive` regenerates every published figure's inputs at two
revisions and compares them byte for byte — that is the evidence an
acknowledgement carries, and it settles generator changes only. Library and
harness changes have no cheap oracle and stay stale until a sweep. Selection is
per figure and a figure is exactly one whole table; a full sweep replaces every
table at once, which is what that doc's session stamp records.

**A sweep preflights.** Everything knowable before the first measurement —
whether each figure's inputs fit the staging area, whether the staging area
fits the machine, whether the disk holds the inputs still to generate — is
checked in the first second, because the alternative is finding out twenty
minutes in with figures already lost. The tmpfs ceiling is computed from the
largest figure's own inputs plus a margin rather than configured, so it travels
to a machine with a smaller `/dev/shm` instead of being a number that happened
to work here.

**A figure declares both edges.** `depends` is what invalidates it; `quoted_by`
is what *it* invalidates — the documents that repeat its numbers or the claim
it licenses, which a fold-in must re-read. **The doc addresses a figure by an
`<!-- figure: <id> -->` marker, never by its heading**, so a heading may quote
a number and be rewritten when that number moves; `--check` reconciles the
markers against the register and prints each figure's consumers. The paths and sizes it uses are environment
variables (`PGDQ_MEASURE_*`) whose defaults suit this machine — see
`CLAUDE.local.md`.

**A profile is not a figure, and the harness prints its recipe too.** `cd
scripts && uv run measure.py --profile-recipe` emits the whole sampling-profile
sequence with every path filled in and runs none of it — the second invocation
the harness owns without executing, for the opposite reason to koji's: a
profile takes seconds, attributes cost per function rather than per
subtraction, and needs no quiet machine, because what it reports is a
proportion. So it produces a `runs/` artifact, never a median, an apparatus
line or a `measurements.md` marker. **Read it against a figure, not instead of
one**: the profiled invocations are the same command shapes the sweep times,
and `scripts/test_measure.py` is what holds them so. Five of its details decide
whether the profile describes what it claims to — the `profiling` binary rather
than `release`, `-C force-frame-pointers=yes` on the build line, `--call-graph
fp` matching it, a warm input, and libc's own symbols, without which half of a
`parse` profile is bare addresses — and each fails by returning a
plausible-looking profile of something else, which is why they are asserted
rather than remembered.

`docs/design/pg-dump-compatibility.md` tracks which `pg_dump` options/variants
are tested/untested/unsupported.

`docs/manual/` is the user-facing manual — written for someone who will never
read the source. Keep design rationale out of it.

`docs/process.md` is the development process this project runs on — the doc
set, where each fact goes, and what landing a slice obliges. **Don't read it
directly: invoke the `process` skill**, which requires reading it in full and
names the obligations. Trigger the skill before implementing a roadmap phase or
slice, wrapping one up, writing or revising a phase spec or notes doc, or
updating `STATUS.md`. Planning, grilling and ad-hoc exploration don't need it.

`docs/design/historical/initial.md` is frozen — historical only. **Every
completed phase's spec and notes have been struck** at a keystone review
(`docs/process.md`, "The keystone: striking the centering") and live only in
git; `architecture.md` replaces them, filed by subject. The out-of-band ledger
went the same way, leaving a watermark of spent `M<k>` numbers in
`docs/design/roadmap.md`. **Don't cite a phase or an out-of-band number for
something already built** — cite the mechanism's section in `architecture.md`
instead.

**Pre-1.0, nothing carries a backwards-compatibility or API-stability
guarantee** — see "Pre-1.0" in `docs/design/roadmap.md`. Don't design around
hypothetical downstream breakage, don't add compatibility shims, and don't
caveat proposals with migration concerns.

For current implementation status (what's built vs. not), see
`docs/status/STATUS.md` — not this file or the design docs, which describe the
design, not its progress. `docs/status/history/` holds dated notes
(`YYYY-MM-DD.md`, one file per day) for two things only: what a future
session should pick up mid-work, and discoveries that changed the plan —
it's not a changelog, so skip routine progress already reflected in
`STATUS.md`. When a discovery changes the plan, put the resulting decision
in the doc that holds the decision and the reasoning/evidence in a history
entry, linked from it — don't inline the narrative. Write each entry as the
day's settled facts looking back, not a log of how the day unfolded: no
supposition-then-correction chains, no "resolved"/"original note" pairs, no
in-progress status that has since resolved. Rewrite sections in place as things
settle — full rules in `docs/status/history/README.md`. Update `STATUS.md` as
part of any change that alters implementation state — don't let it drift.
During a sliced phase, `STATUS.md` is a **terse checklist** of what has landed
this phase, each item linking to the subphase notes doc that holds the detail —
not a prose summary duplicating them.

`STATUS.md`'s **"Known deficiencies" is a register, not prose**: one indexed
line per entry — a stable `KD<k>`, one sentence, a declared stance ((a) a
deliberate tradeoff never to be worked, (b) a defect owned by a named
destination, (c) a defect with no owner, which says "unowned" in that word),
and the file whose paragraph holds the detail. **The detail goes beside the
mechanism**, in that mechanism's `architecture.md` section or wherever its
analysis already lives, never in `STATUS.md`. A limitation whose remedy the
user already has today is not a deficiency at all — it is a property, and it
belongs beside its mechanism with no identifier. Mark a *line of code* with
`deficiency: KD<k>` only where the code would otherwise mislead — where it reads
as a complete, deliberate choice and gives no sign a limitation hangs off it.
**An entry is struck by the change that closes its last part**, not at a phase
boundary — index line, detail paragraph and any code marker in one change,
rewriting rather than annotating on partial closure, and migrating a part that
closes into a *property* beside its mechanism instead of deleting it. The
number stays spent, and the `<!-- deficiency-watermark: KD<k> -->` marker beside
the watermark sentence is what records the allocated range. **Where a `(b)`
entry's owning phase has been sliced, the entry and the slice name each other**,
so landing one re-reads the other and a re-slice must re-target; slicing a phase
means checking the register for the entries it owns. The two directions are
asymmetric: a **ticked** checklist line is a record, so its `KD<k>` is a citation
that may name a struck entry but not an unallocated number, while an **entry**
is present tense and may not name a ticked slice — closing a part rewrites the
entry in the change that ticks the box. A `(b)` entry's owner is read from
`roadmap.md`'s phase index, so an entry owned by a `Complete`, `Struck` or
unlisted phase fails and drops to `(c) unowned` unless a phase absorbs it —
which is why **a phase wrap sets that row to `Complete` in the same change that
deletes its checklist**, and **slicing a phase sets it to `Current` in the same
change that writes one**. `cd scripts && uv run deficiencies.py` reconciles the
index, the detail entries, the markers, that pairing and the phase index, and
fails on any of them — so a re-slice that leaves an entry aimed at a number
whose meaning changed fails the check rather than owing a re-target on
discipline. Full rules: `docs/process.md`, "Known deficiencies".

**When the maintainer answers an entry under `STATUS.md`'s "Decisions worth
another look", close it in that same session** — before the work the answer
unblocked, and never by editing the entry to record that it was reviewed. If
the answer changed an artifact, the fold-in files it and the entry is deleted;
if the answer affirmed the call and changed nothing, write the reasoning
beside the mechanism it governs first, as a rejected-alternative paragraph,
*then* delete. Deletion is the only record that the review happened, and the
section is capped at five entries. Rules: `docs/process.md`, "Decisions worth
another look".

## Writing style
Do not document what _was_, document what _is_.  If we learn something important
enough to persist as historical reference, I'll ask you explicitly to do so.
This holds inside `docs/status/history/` too — a dated filename records *when*
something was learned, not licence to narrate *how*.

## Memories
Prefer to store memories in this project rather than in user memories.  We may
move development to another machine in the future.
