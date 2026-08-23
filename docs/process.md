# Phased development process

A working process for a large, long-running project built by one maintainer
plus an AI agent, where the backlog is far bigger than any one plan can hold.

**This doc is project-agnostic.** It describes the process, not this
repository; copy it into a new project and use it from the first day, when all
that exists is a rough design handoff. `pgdump_query`'s `docs/` tree is the
worked example, and every file named below has a real counterpart there.

What it is for: work measured in months, where decisions made in week two must
still be legible in month six, and where the context that produced a decision
is routinely gone before the decision is questioned. What it is not for: a
two-week project, a spike, or anything whose whole design fits in one
document and stays there.

The cost is real — roughly one document write per meaningful decision. What it
buys is that no session ever re-derives a settled decision, re-discovers a
known limitation as a bug, or re-litigates a question the maintainer already
answered.

---

## The loop

```
initial.md  →  roadmap  →  ┌─ grill phase N ─→ spec ─→ slice ─→ land ─→ notes ─┐
                           └────────── wrap phase N ←── consolidate ←──────────┘
```

**0. Bootstrap.** Write the initial design handoff — problem, philosophy, use
cases, first decisions. One file. Then freeze it (see "Bootstrapping" below).

**1. Grill the phase.** Before writing a phase spec, interview the maintainer
until the design tree has no unvisited branches. Work in rounds of at most
three questions, each with a recommended answer; write the settled decisions
into the docs *after every round*, not at the end. Finding facts is the
agent's job, never the maintainer's — if a question needs a fact from the
filesystem, the upstream source, or a live tool, go get it. See
`.claude/skills/grilling/SKILL.md` for the full protocol.

**2. Write the phase spec.** `docs/design/roadmap-phase<N>-<slug>.md`. This is
what the phase will do and why, decided before any code exists.

**3. Slice it.** A phase too large to land at once is broken into numbered
slices, ordered so that **each slice makes the next one's mistakes visible**.
Cheap, no-code, evidence-gathering slices go first, because they de-risk the
least-evidenced parts of the spec while they are still cheap to change.

**4. Land a slice, write its notes.** Each slice gets
`roadmap-phase<N>.<M>-<slug>-notes.md`, written as it lands, while it is
fresh. Update `STATUS.md` in the same change.

**5. Wrap the phase.** Consolidate the per-slice notes into one
`roadmap-phase<N>-<slug>-notes.md` and delete the per-slice files. Rewrite
`STATUS.md`.

**6. Grill again.** A completed phase produces discoveries that invalidate
guesses about later phases. Re-grill the roadmap before specifying phase N+1,
rather than trusting a plan written before the evidence existed. This is the
step that keeps the roadmap from becoming fiction.

---

## The doc set

| Path | Answers | Must not contain |
|---|---|---|
| `README.md` | What is this, what state is it in, where are the docs | Anything duplicated from the docs it indexes |
| `CLAUDE.md` | How an agent works in this repo: commands, doc pointers **with read triggers**, standing rules | Design rationale; implementation status; anything machine-specific |
| `CLAUDE.local.md` | Facts about *this machine and this operator* | Anything another machine would need |
| `docs/design/historical/initial.md` | The original handoff, frozen | Edits. It is history, not a live doc |
| `docs/design/roadmap.md` | Project goals, standing policies, the phase index | Full phase specs (they get their own files) |
| `docs/design/roadmap-phase<N>-<slug>.md` | **What** phase N does and **why** — the binding spec | How it landed in code |
| `docs/design/roadmap-phase<N>-<slug>-notes.md` | **How** it landed: module map, and the facts later phases inherit | Restatement of the spec; a changelog |
| `docs/design/roadmap-phase<N>.<M>-<slug>-notes.md` | The same, for one slice, until the phase wraps | Anything that should have gone in the spec |
| `docs/design/<invariants>.md` | Every external behaviour a decision assumes, with proof | Assumptions without a re-verification step |
| `docs/design/<compatibility>.md` | Which external variants are tested / untested / unsupported | Untracked "probably fine" rows |
| `docs/design/<standing-constraint>.md` | A rule that cuts across all phases (e.g. layering) | Phase-scoped decisions |
| `docs/status/STATUS.md` | What **is** built, right now | How it got that way |
| `docs/status/history/YYYY-MM-DD.md` | Mid-work pickup state; discoveries that changed the plan | Routine progress; narration of the day |
| `docs/manual/` | How to use the thing, for someone who will never read the source | Design rationale |

### The four that carry the process

**The roadmap** is an index plus the standing decisions. Phases that have been
specified get one line and a pointer. Phases that have not are sketched **only
to the depth needed to keep the current phase from painting us into a corner**
— explicitly annotated as such, with the note that each gets its own full
grilling when it becomes current. This is the anti-over-planning rule, and it
is what makes the roadmap survivable: it is allowed to be vague about month
five.

The roadmap is also where **standing policies** live — the ones that would
otherwise be re-argued every phase. `pgdump_query`'s is "Pre-1.0: no
compatibility obligations", written as a decision not to be reopened, with the
one place it genuinely costs something named explicitly. Write these as
closed, and say they are closed.

**Spec vs. notes** is the sharpest split in the set, and the one most likely to
collapse if you let it. The spec is written before the code and states intent;
the notes are written after and state outcome. A spec is not edited to match
what the code turned out to do — that erases the record of intent, and with it
the ability to notice that the code drifted. A spec *is* edited when the
decision itself changes; the reasoning for that change goes in a history entry
and is linked from the spec.

The notes doc is written for **the next phase**, not for a reader of this one.
Its test: "what would a future session otherwise have to re-derive from the
code?" A module map, the load-bearing invariants of the implementation, the
non-obvious calls and why. Not a changelog.

**STATUS.md** describes the present tense and is rewritten in place. During a
sliced phase it is a **terse checklist** of what has landed, each item linking
to the slice notes that hold the detail — never prose duplicating them. Two
sections earn their keep beyond the checklist:

- **Not started** — so the boundary of what exists is explicit, not inferred.
- **Known gaps** — deficiencies that are known and *accepted*, each with why
  it is safe or what it costs. This is the section that stops the next session
  from re-discovering a deliberate limitation as a bug, and it is worth more
  than the checklist above it.

Keeping STATUS.md current is part of any change that alters implementation
state, not a separate chore. It drifts within days otherwise.

**History** is dated, one file per day, and exists for exactly two things:
what a future session should pick up mid-work, and a discovery that changed
the plan. It is not a changelog and not a diary.

The rule that makes history useful rather than sedimentary: **write the end
state, not the path to it.** An entry exists to be read tomorrow, not to record
today. No supposition-then-correction chains, no "resolved:"/"original note:"
pairs, no in-progress status that has since resolved — rewrite the section in
place when the question is answered. A discarded assumption earns a line only
where a future session would otherwise re-derive it: *"the obvious approach is
X; it fails because Y."* Later days supersede earlier ones rather than editing
them, but an earlier entry that has become actively misleading gets corrected
or deleted, not left as a trap.

Put a `README.md` in `history/` stating these rules. The directory is the
easiest one in the tree to let rot.

### The assumptions register

Give it a project-specific name (`postgres-invariants.md`, `kernel-abi.md`,
`vendor-api-guarantees.md`). Every entry is one property of something outside
your control that a design decision treats as guaranteed:

- **Claim** — what is assumed, stated precisely enough to be falsifiable.
- **Proof** — the source that establishes it. Upstream source code beats
  documentation; documentation beats observed behaviour; observed behaviour
  beats a hunch, and gets labelled as observed.
- **Scope limit** — where the invariant stops holding.
- **Verified against** — which versions, exactly.
- **Relied on by** — which design doc would need fixing if it broke.
- **Re-verify** — the literal command to re-check it.

The register exists because **an upstream release can quietly invalidate one
of these, and the resulting bug surfaces as wrong data rather than an error.**
Its payoff is a single ritual: when a new upstream version lands, walk the
file. That ritual is only possible because every entry carries its own
re-verification step. An entry without one is an entry you will not check.

Add an entry the moment a decision starts depending on external behaviour —
that is the trigger, and it belongs in `CLAUDE.md` so the agent applies it
without being asked.

The **compatibility matrix** is the register's sibling: it tracks which
external variants you have actually exercised. Its value is entirely in
distinguishing *tested* from *assumed* — a row that says "untested
(deliberately), because X" is doing more work than a row that says "supported".

---

## Where does this fact go?

Four questions settle almost every case.

| If the fact is… | It goes in… |
|---|---|
| What we intend to build | the phase **spec** |
| What we built, that a later phase inherits | the phase **notes** |
| What exists right now | **STATUS.md** |
| Why we changed our mind, and the evidence | a **history** entry, linked from the doc holding the resulting decision |
| Something outside our control that we now depend on | the **assumptions register** |
| A rule that will still apply three phases from now | a **standing-constraint** doc, or a named roadmap section |
| Something a *user* needs, with no rationale attached | the **manual** |
| How the agent should behave in this repo | **CLAUDE.md** |
| A path, a host, a piece of hardware, an operator preference | **CLAUDE.local.md** |

The two that get confused:

**Decision vs. evidence.** They split across two files. The decision goes in
the plan or design doc, stated flatly, as the thing that is now true. The
reasoning and evidence go in a dated history entry, and the design doc carries
a one-line pointer to it. Never inline the investigation into the plan — a
plan doc that narrates how it was reached becomes unreadable as a plan.

**Status vs. history.** Both are about what happened, and they are separated by
tense, not by content. If it is true now, it is status. If it is a thing
learned on a particular day whose consequence is already reflected in status,
it is history — and if it is neither, it does not get written down at all.

---

## Naming and lifecycle

```
docs/
  design/
    historical/initial.md                          frozen at bootstrap
    roadmap.md                                     lives forever
    roadmap-phase1-mvp.md                          written at phase start
    roadmap-phase1-mvp-notes.md                    written as phase 1 lands
    roadmap-phase2-typed-columns.md
    roadmap-phase2.3-resolution-notes.md           deleted at phase 2 wrap
    roadmap-phase2-typed-columns-notes.md          consolidated at wrap
    layering.md                                    standing constraint
    postgres-invariants.md                         assumptions register
    pg-dump-compatibility.md                       coverage matrix
  status/
    STATUS.md                                      rewritten in place
    history/README.md
    history/2026-08-22.md                          append-only by day
  manual/
    <topic>.md
```

**Slice numbering.** A phase's planned slices are `<N>.1`, `<N>.2`, …. A third
level (`<N>.<M>.<K>`) is **earned, not planned**: it exists when a slice that
already landed turns out to have shipped the wrong contract, and fixing it is
its own increment. Do not pre-allocate them — the fact that they were earned
rather than planned is itself information about where the design was weak,
and the spec's slice table should say so.

**Consolidation at wrap** is not optional cleanup. Per-slice notes exist so a
slice's detail has somewhere to go while it is fresh, without waiting on the
phase; leaving five of them behind means the next phase reads five overlapping
partial accounts instead of one.

**Freezing.** `initial.md` moves to `historical/` and is never edited again.
Mark it frozen in the file, in the roadmap, and in `CLAUDE.md`. The point is
that it stays readable as *what we thought at the start* — which is only useful
if nobody has quietly updated it.

---

## CLAUDE.md vs. CLAUDE.local.md vs. docs

Three files, three audiences, and the split is about **portability**, not
secrecy.

**`CLAUDE.md`** is checked in and travels with the repo. It holds the command
reference, the standing rules, and — most importantly — **pointers to the docs
with a trigger attached to each**. Not a bibliography: a pointer says *when* to
read the thing.

> `docs/design/layering.md` assigns every module to one of four layers. **Read
> it before adding a module, moving code between modules, or wiring a concern
> across existing ones** — it is a standing constraint, not a phase.

That trigger phrasing is what turns a doc from something an agent might find
into something it reliably reads at the right moment. Write one for every doc
that constrains future work.

`CLAUDE.md` also holds process rules that are about *how the agent works*
rather than about the software: where new docs go, when to add an invariant,
what STATUS.md must look like during a sliced phase, the writing-style rule.
And operational rules with a cost behind them — `pgdump_query`'s is a
long-running-job protocol, because waiting on an hour-long scan expires the
prompt cache and reloads the entire conversation. Any rule of the form "doing
this the obvious way is expensive in a way you cannot see" belongs here.

**`CLAUDE.local.md`** is gitignored and describes *this machine*: hardware
constraints, which volume is HDD versus SSD versus NVMe, absolute paths to
sample datasets and upstream source checkouts, the container runtime,
credentials-adjacent connection details, local scratch services. The test:
would this sentence be wrong, or actively harmful, on a different machine? Then
it is local.

The boundary case worth naming: an operational *procedure* that references a
local path splits across both files. The procedure goes in `CLAUDE.md` (it is
part of how the project works); the path it needs goes in `CLAUDE.local.md`,
and `CLAUDE.md` refers to it by name — *"see `CLAUDE.local.md`"* — rather than
inlining it.

**`docs/`** holds the thinking. `CLAUDE.md` never duplicates it; duplication is
how the two drift into contradicting each other, and the agent has no way to
tell which one is stale.

---

## Rules that keep it from rotting

**Document what *is*, not what *was*.** The single most load-bearing rule in
the set. It applies to every doc, including the ones whose filename is a date —
a date records *when* something was learned, not licence to narrate *how*. If
something is important enough to keep as historical reference, it gets kept
deliberately, on request, not by default accretion.

**Only the current phase gets specified.** Future phases get exactly enough
detail to avoid corner-painting, and say so in the text.

**Write decisions down during the grilling, not after it.** A decision that
lives only in the conversation is a decision that dies with the context window.
An interrupted session should leave every answer so far already recorded.

**Update STATUS.md inside the change that alters state.** Not afterwards.

**Every standing constraint carries its own check.** The layering doc ends with
three greps that must produce no output. A rule nobody can mechanically verify
is a rule that is already being violated somewhere.

**Record known deviations rather than fixing them opportunistically.** The
layering doc names two modules that sit in the wrong layer, explicitly so that
nobody treats them as precedent and nobody "fixes" them outside of work that
reworks the module anyway.

**The agent finds the facts.** Any question answerable from the filesystem,
the upstream source tree, a live container, or a test run is the agent's to
answer. The maintainer's time goes to decisions only.

---

## Bootstrapping on day one

Do not create the whole tree up front. Most of these files earn their existence
at a specific moment; creating them empty just produces a directory of stubs
nobody trusts.

1. **Write `initial.md`** — problem, philosophy, use cases, the decisions you
   already know you have made, non-goals. Put it at
   `docs/design/historical/initial.md` from the start, so the eventual freeze
   is a one-line note rather than a move.

2. **Grill it.** First real session. The output is a roadmap, not code.

3. **Write `docs/design/roadmap.md`** — goals, standing policies, and the phase
   list with everything past phase 1 sketched at corner-avoidance depth only.

4. **Write `docs/design/roadmap-phase1-<slug>.md`** — the first phase spec, in
   full, before any code.

5. **Write `CLAUDE.md`** — commands, the doc pointers with their triggers, the
   writing-style rule, and the process rules from this document that you want
   enforced. **Write `CLAUDE.local.md`** the first time you need a machine
   fact.

6. **Create `docs/status/STATUS.md` and `docs/status/history/README.md`** when
   the first code lands.

Then, each at its own trigger:

| Create… | When… |
|---|---|
| the assumptions register | a decision first depends on external behaviour you did not verify |
| the compatibility matrix | you first say "we don't support that" out loud |
| a standing-constraint doc | a rule shows up in two different phase discussions |
| `docs/manual/` | something is usable by someone who did not build it |
| a phase-notes doc | the first slice of that phase lands |

---

## Smells

Each of these means a specific rule has stopped being followed.

- **STATUS.md reads as prose.** It has become a phase-notes doc. Cut it to a
  checklist with links.
- **A notes doc restates its spec.** It was written for a reader of this phase
  instead of for the next phase.
- **A spec doc matches the code exactly.** It was retro-fitted, and the record
  of original intent is gone.
- **History entries contain "turns out", "actually", "correction".** They were
  written as a log of the day rather than as the day's settled facts.
- **An invariant has no re-verification step.** It will not be checked at the
  next upstream release, which is the only reason it was written down.
- **`CLAUDE.md` explains a design.** It should be pointing at a doc instead.
- **A doc pointer has no trigger.** It will not be read at the moment it
  matters.
- **The roadmap's later phases are as detailed as the current one.** You are
  planning against evidence you do not have yet; that detail will be wrong and
  expensive to unwind.
- **Slice notes surviving past the phase wrap.** Consolidation was skipped.
