# CLAUDE.md

Guidance for Claude Code in this repository. `README.md` is the project
overview. **The code is the ground truth**: read the module before its doc,
and the data (`runs/`, `measurements.md`'s tables, a profile) before a claim
about it. Docs here exist to locate ground truth quickly and to record
decisions the code cannot explain, nothing more.

## Commands

```sh
mise run check [--affected] [--verify]            # the round's checks once; summary here, logs in runs/check/
cargo check --workspace
cargo nextest run -p <crate> [--test <target>] [<filter>]   # part of the suite; nextest pinned in mise.toml
cargo fmt                                         # config: rustfmt.toml
INSTA_UPDATE=always cargo test -p <crate> --test <target>   # accept snapshots (no cargo-insta here)
cargo run -p pgdt -- parse --source <file>      # binary is `pgdt`; scans ahead, resumes
cargo run -p pgdt -- info --source <file> [--detail]   # never scans; reads the cache
cargo build --release -p pgdt --features introspect --target-dir <own>  # the instrument; never timed

cd scripts && uv run generate_fixtures.py [--version 13|16|18] [--skip-dumps] [--skip-oracle]
cd scripts && uv run measure.py --list|--stale|--check|--figure <id>|--all|--render <run-dir>
cd scripts && uv run measure.py --koji-recipe [--wrap] | --profile-recipe | --heaptrack-recipe   # printed, never run
cd scripts && uv run release.py image|build|bench|suite|suite-archive|notices   # the release image; pgdt built in it, held to its glibc; a bench, the suite, the notices there
python3 scripts/release_ci.py preflight|archive|smoke|checksums|draft|publish   # what the two release workflows run beside release.py
python3 scripts/debian_window.py      # the image's Debian pin against the window a release allows; `/release`'s step
cd scripts && uv run citations.py     # every `<doc>.md`, "section" citation resolved
cd scripts && uv run deficiencies.py  # KD index vs the code marker carrying each detail, vs phase index
cd scripts && uv run upstream.py      # the upstream register vs the `upstream: UF<k>` marker at each site
cd scripts && uv run pg_refuses.py    # each `pg-refuses: I<n>` marker vs its invariant's "Relied on by"
cd scripts && uv run major_differences.py  # the `VD<n>` register, and no `VD<n>` cited outside it
cd scripts && uv run repoint.py       # the record's caps, and its growth since the last blind read; red means /repoint
cd scripts && uv run oracle_register.py && uv run floor_mapping.py && uv run oracle_differences.py
cd scripts && uv run emitter_register.py [--extract]   # the emitter register; --extract reads the upstream checkouts
cd scripts && uv run python -m unittest test_<script>   # one script's own tests
```

Every `scripts/*.py` has a `--help` and a `test_*.py`; the harness's own rules
are asserted there, not remembered here.

## Long-running processes

Anything over ~10 minutes (a `measure.py --all` sweep, any `pgdt` run against
the koji dump in `CLAUDE.local.md`) is launched fully detached with `setsid`,
logging under `runs/` (gitignored), and **never waited on, monitored, or given
a completion notification** — waiting expires the prompt cache and reloads the
whole conversation. Record what was launched and the log path in today's
history entry; a later session reads the result. Stop a sweep by process group
(`pkill -g <pgid>`). Nothing else may build or test while a sweep runs.
`measure.py --koji-recipe` prints the koji invocation with every path filled
in; its load-bearing details are asserted by `scripts/test_measure.py`.
Never edit a `runs/` orchestration script while it is running.

## Where things are, and when to read them

- `docs/design/decisions.md` — the decisions the code cannot explain, one
  numbered `D<k>` entry each, capped at 700 lines: anticipatory shapes, chosen
  defaults, measured refusals, and the layering rules. **Read the entries for
  a mechanism before changing it or proposing an optimization to it**; most
  obvious optimizations carry a refusal already. Cite an entry, never restate
  it. Adding an entry may mean striking one.
- `docs/design/roadmap.md` — goals, "Standing rules", and the phase index.
  **Read "Standing rules" before a design decision a later phase inherits.**
  A phase is `P<k>`, allocated on discovery, never renumbered or reused; its
  spec is `docs/design/roadmap-P<k>-<slug>.md`, its inbox
  `…-inbox.md` (read and drained when the phase is grilled). The open
  phase is the index's `Current` row; the index's order says which is next,
  and a phase is grilled and specified before code is written.
- `docs/design/out-of-band.md` — work belonging to no phase. **Read it whole
  before allocating an `M<k>`** and before picking up unscheduled work.
- `docs/status/deficiencies.md` — the `KD<k>` register. **Read it before
  grilling or specifying a phase, admitting an `M<k>`, or proposing a change
  to a mechanism**; each entry's detail is at its code marker.
- `docs/status/upstream.md` — the `UF<k>` register: dependency defects and
  limits we wait on upstream to fix. **Invoke `upstream-issue` before working
  around a dependency's defect, and `upgrade-deps` before any dependency
  upgrade**; the first holds the register's rules.
- `docs/design/postgres-invariants.md` (`I<n>`) and
  `docs/design/runtime-invariants.md` (`RT<n>`) — properties of `pg_dump`
  output and of the process's environment that decisions depend on. **Add an
  entry when a decision starts depending on one; walk them at a new major.**
  A difference between majors nothing depends on yet is a `VD<n>` in
  `docs/design/postgres-major-differences.md`: **file one when you find it,
  and read the file before adding an invariant about a major.**
- `docs/design/measurements.md` — every performance figure, with the command
  that reproduces it; "The apparatus" holds the rules for taking one. **Read
  before any performance claim.** `scripts/measure.py` takes the figures and
  emits the tables; **a table is never hand-edited** and a re-take is
  `--figure <id>`, never a one-off script. **Run `measure.py --stale` before
  reasoning from a figure**, and re-take a red one first. Red is otherwise
  the resting state: no commit owes an acknowledgement, no wrap a sweep.
- The `evidence` skill governs what a reading may be concluded to mean.
  **Invoke it before concluding anything from a benchmark, profile or RSS
  reading, or fitting a model.** Which instrument to reach for is
  `roadmap.md`, "Attribution is introspective; only the gate is blind".
- `docs/design/pg-dump-compatibility.md` — which `pg_dump` variants are
  tested, untested, unsupported.
- `docs/manual/` — user-facing; no design rationale.
- `docs/process.md` — the development process. **Don't read it directly:
  invoke the `process` skill** before implementing or wrapping a phase or
  slice, writing a spec, repointing, or updating `STATUS.md`. It holds the
  rules for the deficiency register (`KD<k>`), "Decisions worth another
  look", inboxes, repointing, and the keystone.
- `docs/status/STATUS.md` — what is built. `docs/status/history/YYYY-MM-DD.md`
  — only what a future session must pick up and discoveries that changed the
  plan, written as settled facts; rules in `docs/status/history/README.md`.

A citation names a document and a section; `citations.py` resolves every one.
Run it after rewriting a heading.

## Standing rules

- **Pre-1.0**: no backwards-compatibility or API-stability guarantee. No
  shims, no migration caveats.
- **The library never replaces cache data automatically** — see
  `decisions.md`'s cache entry before touching a cache path.
- **A build carrying `introspect` is never timed.**
- **Find code with `graft`, not `grep`** — `graft ask "<q>" --source`;
  [`.claude/skills/graft/SKILL.md`](.claude/skills/graft/SKILL.md) has the rest.
- **Document what _is_, not what _was_**, inside `docs/status/history/` too. A
  fact is stated once, in one place, and cited everywhere else; a number is
  stated only in `measurements.md`. If something is worth keeping as history,
  the maintainer will ask.
- **We do not make design decisions based on implementation cost** — we
  shouldn't reinvent available wheels, but the elegance and correctness of our
  final 1.0 design is all that matters.
- **Memories** go in this project, not user memory; development may move
  machines.
- **The session scratchpad is not a home.** Before a session or round
  reports, whatever a later one needs moves to `runs/` or to a data directory
  `CLAUDE.local.md` names, and the record (notes doc, history entry, handoff
  doc) names it there; everything else in `scratchpad/` is deleted. A file
  the record does not name is one no later session can find, so it is
  transient by definition. Nothing expected past ~100 MiB — a build of
  another commit, its target dir, a generated dump — is written to the
  scratchpad even briefly; it goes to its data directory from the start. A
  `SessionEnd` hook (`.claude/helpers/scratchpad-end.sh`) removes the
  session's directory as a backstop, not as the protocol.
