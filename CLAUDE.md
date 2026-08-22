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
cargo run -p pgdump_query-cli -- parse <file>      # binary is named `pgdq`
cargo run -p pgdump_query-cli -- info <file> [--verbose]

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
the HDD. It goes in a memory-limited container; build a static binary so any
base image works, and let the container itself write the log:

```sh
cargo build --release --target x86_64-unknown-linux-musl -p pgdump_query-cli
mkdir -p runs
sudo nerdctl run -d --name pgdq-koji -m 512m --memory-swap 512m \
  -v "$PWD/target/x86_64-unknown-linux-musl/release/pgdq:/pgdq:ro" \
  -v "$PWD/runs:/out" \
  -v "/path/to/dump.sql:/dump.sql:ro" \
  postgres:16-alpine \
  sh -c '/pgdq info /dump.sql --cache-path /out/koji.dqcache --verbose > /out/koji-scan.log 2>&1; echo "exit=$?" >> /out/koji-scan.log'
```

**Pass `--cache-path` into the mounted `/out`.** The dump is mounted read-only,
so the colocated default (`/dump.sql.dqcache`) lands in the container's
ephemeral writable layer and is destroyed with the container — throwing away an
hour of scanning without an error, since the write itself succeeds.

A later session reads `runs/koji-scan.log`; `sudo nerdctl inspect -f
'{{.State.Status}}' pgdq-koji` says whether it is still going.

## Architecture & design docs

`docs/design/roadmap.md` holds the project goals and indexes the phases; a
phase that has been specified gets its own doc, named
`docs/design/roadmap-phase<N>-<slug>.md`. Keep that convention when a new
phase's plan is written.

`docs/design/roadmap-phase1-mvp.md` is the source of truth for the built
architecture — read it before making architectural changes, rather than
inferring intent from code alone; its companion
`docs/design/roadmap-phase1-mvp-notes.md` records how that phase landed in code
(module map, and the implementation facts later phases inherit).
`docs/design/roadmap-phase6-scan-performance.md` is the performance design for
the local-file read path — read it before touching the batch layer or the cache
format, which it constrains ahead of its own phase.
`docs/design/pg-dump-compatibility.md` tracks which `pg_dump` options/variants
are tested/untested/unsupported.
`docs/design/postgres-invariants.md` is the evidence layer beneath the design
docs: each entry is a property of `pg_dump` output that a design decision
treats as guaranteed, with the source that proves it and how to re-verify it
when a new PostgreSQL major lands. Add an entry whenever a decision starts
depending on `pg_dump` behaving a particular way.
`docs/manual/` is the user-facing manual — written for someone who will never
read the source. Keep design rationale out of it.

`docs/design/historical/initial.md` is frozen — historical only.

A phase large enough to land in slices numbers them `<N>.<M>` and gives each
its own notes doc, `roadmap-phase<N>.<M>-<slug>-notes.md`, written as that
slice lands. At the end of the phase they are consolidated into a single
`roadmap-phase<N>-<slug>-notes.md` and the per-slice files removed. Phase 2 is
the first to work this way — see its "Implementation slices".

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
in the plan doc itself and the reasoning/evidence in a history entry, linked
from the plan doc — don't inline the narrative into the plan. Write each entry
as the day's settled facts looking back, not a log of how the day unfolded: no
supposition-then-correction chains, no "resolved"/"original note" pairs, no
in-progress status that has since resolved. Rewrite sections in place as things
settle — full rules in `docs/status/history/README.md`. Update
`STATUS.md` as part of any change that alters implementation state — don't
let it drift. During a sliced phase, `STATUS.md` is a **terse checklist** of
what has landed this phase, each item linking to the subphase notes doc that
holds the detail — not a prose summary duplicating them. `roadmap-phase1-mvp.md`'s "Parser robustness requirements" is the
spec the `COPY`-block scanner (`pgdump_query/src/scan.rs`, `copy.rs`)
implements — read it before changing scanner behaviour.

## Writing style
Do not document what _was_, document what _is_.  If we learn something important
enough to persist as historical reference, I'll ask you explicitly to do so.
This holds inside `docs/status/history/` too — a dated filename records *when*
something was learned, not licence to narrate *how*.

## Memories
Prefer to store memories in this project rather than in user memories.  We may
move development to another machine in the future.
