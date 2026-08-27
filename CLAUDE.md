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

Running `pgdq` against the multi-hundred-GB koji sample (see
`CLAUDE.local.md`) is exactly this case: a full scan is roughly an hour on
the HDD. It goes in a memory-limited container, and — because a scan's
throughput is a performance figure — on the **default glibc build in a glibc
image**, per `docs/design/measurements.md`'s standing rule that the allocator
is part of the apparatus. A host-built binary runs in `postgres:16`; the
static musl build and `postgres:16-alpine` stay available for portability, but
figures taken with it are not comparable. Let the container write the log:

```sh
cargo build --release -p pgdump_query-cli
mkdir -p runs
sudo nerdctl run -d --name pgdq-koji -m 512m --memory-swap 512m \
  -v "$PWD/target/release/pgdq:/pgdq:ro" \
  -v "$PWD/runs:/out" \
  -v "/path/to/dump.sql:/dump.sql:ro" \
  postgres:16 \
  sh -c 'exec /pgdq parse --source /dump.sql --dqcache /out/koji.dqcache >> /out/koji-scan.log 2>&1'
```

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

A later session reads `runs/koji-scan.log`; `sudo nerdctl inspect -f
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
the zero-copy Arrow path, the query passes, the cache, the CLI, fixtures, or
the testing approach — before changing that mechanism. Each section carries its
own *Rejected:* paragraphs, which are the part that cannot be recovered from
the code. Two sections are hard constraints rather than description: "Parser
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
grill it, amend the spec, and give it a slice number.

A phase that has been specified gets its own doc,
`docs/design/roadmap-phase<N>-<slug>.md`; keep that convention when a new
phase's plan is written. `docs/design/roadmap-phase7-scan-performance.md` is
the performance design for the local-file read path — read it before touching
the batch layer or the cache format, which it constrains ahead of its own
phase.

`docs/design/roadmap-phase<N>-inbox.md` holds facts an *earlier* phase found
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

`docs/design/historical/initial.md` is frozen — historical only. The specs and
notes for phases 1-3 were struck at the keystone review (`docs/process.md`,
"The keystone: striking the centering") and live only in git;
`architecture.md` replaces them. **Don't cite a phase number for something
already built** — cite the mechanism's section in `architecture.md` instead.

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

## Writing style
Do not document what _was_, document what _is_.  If we learn something important
enough to persist as historical reference, I'll ask you explicitly to do so.
This holds inside `docs/status/history/` too — a dated filename records *when*
something was learned, not licence to narrate *how*.

## Memories
Prefer to store memories in this project rather than in user memories.  We may
move development to another machine in the future.
