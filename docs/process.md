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
                                                │
                                       (at the keystone)
                                                ↓
                            strike the centering ─→ subject-filed docs ─→ resume
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

**Size a slice by its review, not by its scope.** A slice that pairs a
self-contained new module with a rework of an already-tested core path is two
slices: the confidence in each half is different, and bundling them forces the
review to accept both at one confidence. Split at spec time — the seam is
usually visible there — because the alternative is discovering it mid-slice,
when the only options left are landing the risky half unreviewed or shipping
the slice half-done.

**In the same change that writes the spec, write the whole slice list into
`STATUS.md` as an unchecked checklist** — every slice, with its title and a
one-line description, before any of it exists. The checklist is where the
phase's progress is tracked, and it is the *only* place; see "Progress lives
in STATUS, never in the spec" below.

**4. Land a slice, write its notes.** Each slice gets
`roadmap-phase<N>.<M>-<slug>-notes.md`, written as it lands, while it is
fresh — **including a slice that lands no code at all**, because a
fixture-only or evidence-only slice is precisely the kind whose findings the
next slice inherits. In the same change, tick the slice's box in `STATUS.md`
and point it at the notes doc.

**5. Wrap the phase.** Consolidate the per-slice notes into one
`roadmap-phase<N>-<slug>-notes.md` and delete the per-slice files. Rewrite
`STATUS.md`.

**6. Grill again.** A completed phase produces discoveries that invalidate
guesses about later phases. Re-grill the roadmap before specifying phase N+1,
rather than trusting a plan written before the evidence existed. This is the
step that keeps the roadmap from becoming fiction.

**Drain the phase's inbox first.** If earlier phases filed facts for this one
(see "Inboxes" below), that file is the first thing the grilling reads and the
last thing it deletes.

**7. At the keystone, strike the centering.** Once the system's *shape* stops
being in question, the phase-by-phase record becomes scaffolding around a
structure that no longer leans on it. Re-file the doc set by subject, retarget
the read-triggers, and delete the phase docs. This happens once or twice in a
project's life, not every phase — see "The keystone: striking the centering"
below. Then the loop resumes: the next phase is grilled and specified exactly
as before.

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
| `docs/design/roadmap-phase<N>-inbox.md` | Facts an *earlier* phase found that phase N's grilling must not miss | Anything with a proper home elsewhere; speculation about phase N's design |
| `docs/design/<architecture>.md` | How the built system works, filed by **subject** — exists only after a keystone | Phase history; what any phase was committed to |
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
sliced phase it is a **terse checklist**, populated in full and unchecked when
the spec is written, then ticked slice by slice:

```
## Phase 3 progress

- [x] **3.1** The `objects` fixture schema — the TOC kinds neither existing
      schema produces, plus large objects. No library code. Notes:
      `docs/design/roadmap-phase3.1-objects-fixture-notes.md`
- [ ] **3.3** The TOC enrichment layer: owner, kind labels, `Tablespace:`,
      TOC-coverage reporting.
```

A landed item links to the slice notes that hold the detail — never prose
duplicating them. **A box is ticked only when the slice's whole spec row is
delivered.** Partially complete work is never "done"; a tick that can mean
"about half" makes every other tick worthless. An unfinished slice keeps its
empty box and says, in its own checklist entry, what landed and what did not.

Three sections earn their keep beyond the checklist:

- **Not started** — so the boundary of what exists is explicit, not inferred.
- **Known gaps** — deficiencies that are known and *accepted*, each with why
  it is safe or what it costs. This is the section that stops the next session
  from re-discovering a deliberate limitation as a bug, and it is worth more
  than the checklist above it.
- **Decisions worth another look** — calls made without the maintainer
  present that a person should still weigh in on: cautionary and
  informational, never blocking. Each entry states the call, why it was made
  that way, and what would change if it were reconsidered. An entry leaves
  when the maintainer has looked at it — either settled into the design docs
  or reversed. This is the pressure valve that makes unattended work honest
  rather than silent; without it the choice is between stalling and burying
  the decision in a diff.

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

### Inboxes: facts filed by destination

Notes docs are filed by **origin** — "what phase 3 learned". That works for
the next phase and fails for a distant one: a fact phase 3 turned up that
phase 7 needs ends up in phase 3's notes, and nothing prompts anyone to read
those when phase 7 comes up. It is the "doc pointer with no trigger" failure,
applied to facts instead of documents.

So a fact addressed to a phase that has no spec yet goes in that phase's
**inbox**, `roadmap-phase<N>-inbox.md`, created the moment it gets its first
entry and never as an empty stub.

Three fields per entry, and the third is what keeps it honest:

- **Fact** — stated flatly, as the thing that is now true.
- **Why this phase cares** — the decision it bears on. An entry that cannot
  name one is not a fact for this phase; it is a note to nobody.
- **Origin** — slice and date. A reader re-checks the origin rather than
  trusting an entry that has aged; add **contingent on** when something
  specific would falsify it.

**The threshold is: no other home.** An external behaviour goes in the
assumptions register. Something the *next* phase inherits goes in the phase
notes. A rule that outlives phases is a standing constraint. A wanted feature
is a roadmap "Future" item. An accepted deficiency is a `STATUS.md` known gap.
What is left — evidence found in phase N that constrains a design decision in
phase M, where M is far enough out to have no spec — is the only thing an
inbox is for. Most forward-looking remarks fail this test, and filing them
anyway is how the file stops being read.

**An inbox is drained, not archived.** Step 6's grilling walks every entry,
folds it into the spec or discards it as stale, and **deletes the file**. That
lifecycle is the whole defence against a dumping ground: a document that is
explicitly consumed cannot quietly accumulate forty entries nobody trusts, the
way a permanent "future considerations" page does.

**Both sides need a trigger**, and they go in `CLAUDE.md` like every other
pointer: read the inbox before grilling or specifying a phase; file into one
the moment a slice turns up a fact a later phase will need. The write-side
trigger is the weaker of the two — it fires when you are mid-slice and
thinking about something else — and a half-used inbox is worse than none,
because the next session reads it as complete. Nothing in the process fixes
that; it is a cost to accept knowingly.

### Out-of-band work

Not everything is a phase. A CLI ergonomics change after real users try the
thing, a defect fix that changes no decision — these fit one session and
belong to no phase's intent. Making them slices corrupts what a slice means:
the phase's checklist stops being "what this phase committed to" the moment
unrelated work is appended to it, and between phases there is nothing to
append to at all.

So the roadmap carries a standing **out-of-band ledger**: an item gets a
number (`M1`, `M2`, …) and **one terse line** — date, what changed, and a
pointer to the dated history entry that says why. No spec, because there was
no intent doc; **no notes doc, because the history entry is the notes.** That
last part is only sound because out-of-band work is by definition not
something a later phase inherits — and where it turns out one does, the fact
goes in that phase's *inbox*, the mechanism that already exists for it.

**The admission rule is the load-bearing half.** An item is out-of-band only
if it changes no decision any spec records *and* fits one session. Anything
that changes a decision goes back through grilling → spec amendment → a
numbered slice. Without that rule the ledger becomes where design work goes to
avoid being reviewed, which is the failure mode it has to be built against.

The ledger is an index, not an account: it grows for the life of the project,
so its lines stay one line. Detail lives in the history entry.

---

## The keystone: striking the centering

A masonry arch is built over *centering* — a temporary wooden frame that holds
every stone in place while the arch is incomplete. The frame is not part of the
arch and was never meant to be. When the keystone drops in, the arch carries
itself, and the centering is struck: removed entirely, because from that moment
it holds nothing up and only obstructs the space beneath.

Phase docs are centering. While a phase is in flight its spec is a contract
under review and its notes are how a slice hands off before the phase can
consolidate; while the system is half-built, "module C is here because phase 6
will consume it" is the only thing making module C legible. Both stop being
true at the same moment, and it is not a phase boundary.

### When

The **keystone** is when the system's *shape* stops being in question: every
use case has an implementation, however crude, and everything remaining is
expansion (more of a thing that exists) or ergonomics (a nicer way to reach
it), not structure.

The test is a reader, not a checklist: **can someone who knows the problem
domain read the code and predict where a new feature goes?** If yes, the
scaffolding is load-bearing for nobody. If a module still only makes sense
once you know which phase is going to consume it, the keystone has not landed
and striking the centering early will cost more than it saves.

This happens once or twice in a project's life. It is close to a 1.0, but it
is about the *doc set*, not about the public interface — a project can reach a
keystone with its API still openly in flux, which is exactly the pre-1.0 case.

### What it changes

**The doc set stops being filed by *when* and starts being filed by *what*.**

Filed-by-time is right while the system is being built, because the questions
are "what did we commit to" and "did we deliver it", and both are scoped to a
phase. Filed-by-subject is right once it is built, because every question a
reader now brings is about a mechanism — how does the cache decide staleness,
what closes a statement — and a mechanism assembled over four phases has its
answer smeared across four documents, none of which is wrong and none of which
is complete.

That smearing is the actual cost, and it is worse than the page count
suggests: a reader who finds the phase-2 answer has no way to know a phase-3
document amended it.

### The test each artifact must pass

An artifact survives the review if **it answers a question a future session
will actually ask, *and* reading it here costs less than re-deriving the answer
from the code plus `git log`.**

Both halves. Something that fails the first is dead weight however expensive it
was to produce. Something that fails the second is a cache of a value that is
cheaper to recompute — and a stale one, since the code moves and the doc does
not.

Applied to the material, six categories fall out. The dispositions differ, and
the mistake is treating the whole phase record as one thing:

| Category | Looks like | Disposition |
|---|---|---|
| **Provenance** | "landed in 3.3", "bumped to v5 in 3.2.2", "Phase 1's default" | **Delete.** It answers *when*, `git log` answers *when*, and nobody asks. |
| **Citation** | "(`roadmap-phase3.md`, "Span boundaries")" | **Retarget, or inline the sentence.** A pointer into a deleted doc is worse than no pointer: a session spends a tool call following it. |
| **Rationale in situ** | "rejected — it would leak a mapping concern into L1's event contract" | **Keep; strip the phase number.** The reason is durable, the number is not. |
| **Negative result** | "an earlier attempt put a start floor on `Builder`; it tiles, but…" | **Keep, and promote.** Not recoverable from the code *at any price* — the code records what was built, never what was tried and abandoned. |
| **External fact** | the assumptions register; "`--with-statistics` does not exist" | **Keep untouched.** Never phase-filed to begin with. |
| **Live obligation** | "reserved; not populated until phase 7" | **Keep; re-point by name, not by number.** The obligation is stable; the numbering ahead of the keystone is about to be re-grilled. |

Measurements are the one category needing a judgement call rather than a rule:
keep a figure only if the command that reproduces it survives with it. A
baseline nobody can re-run is not a baseline, it is a rumour with a decimal
point.

**Negative results are the reason this is a distillation and not a deletion.**
Everything else in the phase record is either recoverable from the code or
already filed elsewhere. What was *tried and rejected* is recoverable from
nothing, and losing it has a silent failure mode: the next session does not
know to look, so it re-derives the rejected design, implements it, and finds
out the hard way. Harvest these first, before touching anything.

### Order of operations

Deletion is last, and it is not where the saving comes from.

1. **Distill.** Write the subject-filed docs *first*, with the phase docs still
   in front of you. A section per mechanism, each carrying its own rejected
   alternatives.
2. **Retarget.** Repoint `CLAUDE.md`'s read-triggers at the new docs. **This is
   where the token cost actually drops.** Orientation cost is set by what the
   triggers pull into context at the moment of work, not by what exists on
   disk; an untriggered file costs nothing. A trigger that says "read this
   50KB spec before touching the cache format" is the expense, and it is paid
   on every visit.
3. **Sweep the code.** Strip provenance, repoint or inline citations, de-number
   forward references.
4. **Delete.** Now. Deletion's job is to remove the *second authority* — so
   that nobody, human or agent, has to decide whether the spec or the
   distillate wins, and so that no future edit lands in the copy nothing reads.

Doing it in the other order is the tempting mistake: deleting first feels like
the point, saves nothing, and destroys the source you were about to distil
from.

### What a keystone review must not do

- **Renumber the remaining phases.** Same reason the process already prefers
  earning an `<N>.<M>.<K>` over renumbering a tail: it invalidates every
  surviving reference — inbox filenames especially — and erases the record
  that anything happened. Phases keep their numbers; the sequence just starts
  further in.
- **Touch the assumptions register or the compatibility matrix.** Neither was
  ever phase-filed, and the register is precisely what makes discarding the
  rest affordable: it is the external evidence that would otherwise have to be
  re-established from upstream source.
- **Rewrite or delete history.** Dated entries have no read-trigger, so they
  cost nothing to keep, and they are the raw evidence beneath the distillate —
  including the evidence for the review itself.
- **Freeze the distillate.** `initial.md` is frozen because it records what we
  thought at the start. The architecture doc records what is true *now*; it is
  edited like any live doc, and a keystone that produces a second frozen
  artifact has just moved the problem.
- **Rewrite the manual.** It was written for someone who will never read the
  source, which means it was already subject-filed.

### What it costs

The record of **intent** goes. After this you can no longer ask "what did phase
3 commit to, and did it deliver?" — and there is no reconstructing it from a
document that describes the result, because the whole point of the spec/notes
split was that the two are allowed to differ.

Accept that deliberately rather than discovering it later. It is affordable
only because the question expires at the keystone: once the shape is settled,
"did the phase deliver what it promised" has been answered by the system
existing, and every remaining question is about what the code does now.

The failure mode in the other direction is worse and more common — a project
that keeps its full phase archive read-triggered forever, where orientation
gets steadily more expensive and every mechanism must be reassembled from the
four documents that each hold a quarter of it.

---

## Where does this fact go?

Four questions settle almost every case.

| If the fact is… | It goes in… |
|---|---|
| What we intend to build | the phase **spec** |
| What we built, that a later phase inherits | the phase **notes** — or, after a keystone, the **architecture** doc's section for that mechanism |
| Something we tried and rejected | beside the mechanism it would have replaced, wherever that lives |
| What exists right now | **STATUS.md** |
| Why we changed our mind, and the evidence | a **history** entry, linked from the doc holding the resulting decision |
| Something outside our control that we now depend on | the **assumptions register** |
| A rule that will still apply three phases from now | a **standing-constraint** doc, or a named roadmap section |
| Something a *distant, unspecified* phase will need to know | that phase's **inbox** |
| A one-session change that belongs to no phase | the roadmap's **out-of-band ledger**, one line, pointing at a history entry |
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
    roadmap-phase7-inbox.md                        filed early, drained at phase 7's grilling
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
level (`<N>.<M>.<K>`) is **earned, not planned**, in one of two ways: a slice
that already landed turns out to have shipped the wrong contract, and fixing
it is its own increment; or a slice turns out to have been mis-sized, and the
part that did not land becomes its own increment. Do not pre-allocate them —
the fact that they were earned rather than planned is itself information about
where the plan was weak, and the spec's slice table should say so. Prefer
earning a third level over renumbering the tail: renumbering invalidates every
reference to a slice number and erases the record that the split happened at
all.

A mis-sized slice's spec row is **rewritten to the scope that actually
landed**, with the remainder moved into the new `<N>.<M>.<K>` rows. That is a
decision change, not progress-tracking — the reasoning goes in a history entry
and the spec's slice table says the split was earned. It is the one case where
the finished half is legitimately ticked.

**Consolidation at wrap** is not optional cleanup. Per-slice notes exist so a
slice's detail has somewhere to go while it is fresh, without waiting on the
phase; leaving five of them behind means the next phase reads five overlapping
partial accounts instead of one.

**After a keystone**, the `roadmap-phase<N>-*` files for completed phases are
gone and `architecture.md` stands in their place; `roadmap.md` carries only
goals, standing policies and the phases still ahead. The standing-constraint,
assumptions-register, compatibility and manual files are unaffected — they were
already filed by subject, which is why they survive a transition that removes
40% of the tree.

**Freezing.** `initial.md` moves to `historical/` and is never edited again.
Mark it frozen in the file, in the roadmap, and in `CLAUDE.md`. The point is
that it stays readable as *what we thought at the start* — which is only useful
if nobody has quietly updated it.

---

## Working unattended

An agent session with no maintainer present can still land a slice. What it
cannot do is decide, on its own, that a risky change is acceptable — because
the thing it is short of is not capability but a reviewer.

**Stop at the last clean boundary when ambiguity meets a wide blast radius.**
Concretely: an unattended session does not rework an already-tested core path
on a judgement call. It lands the part it is confident in, leaves the rest,
and hands over.

**Never mix high-confidence and low-confidence work in one review cycle.**
This is the operative rule, and it is the reason for the one above. A diff
that contains both forces the maintainer to accept the uncertain half in order
to get the certain one, which is exactly the review they were meant to
provide. Two changes, reviewed separately, cost less than one change reviewed
badly.

**Stopping is not the same as being blocked.** The session finishes the
unaffected work, ticks nothing it did not finish, and leaves three things: the
slice's box unticked with its entry saying what landed and what did not, an
entry under "Decisions worth another look" for each call the maintainer should
weigh, and — if the work is mid-flight — a history entry saying where to pick
it up.

**A judgement call that had to be made anyway goes in "Decisions worth another
look", not in silence and not in a blocking question.** The section exists so
that proceeding and flagging is available as a third option; use it. An entry
there is cheap to write, cheap to read, and cheap to reverse.

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

**Progress lives in STATUS, never in the spec.** A phase spec is written once
and then left alone for the duration of the phase: no ✅ marks on the slice
table, no "landed"/"deferred" annotations, no rewriting a slice's row to
describe what it turned out to do. Those all destroy the same thing — the
record of what the phase was *committed* to, which is the only baseline the
finished phase can be measured against. The spec is still edited when the
**decision** changes (with the reasoning in a history entry, per "Spec vs.
notes"), and that is the sole reason to touch it. Everything else — what has
landed, what was cut, what was deferred and why — goes in the STATUS checklist
and the slice notes.

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
| a phase **inbox** | you first find a fact a phase with no spec yet will need |

---

## Smells

Each of these means a specific rule has stopped being followed.

- **STATUS.md reads as prose.** It has become a phase-notes doc. Cut it to a
  checklist with links.
- **A notes doc restates its spec.** It was written for a reader of this phase
  instead of for the next phase.
- **A spec doc matches the code exactly.** It was retro-fitted, and the record
  of original intent is gone.
- **The spec's slice table carries ✅ marks, or a slice row describes what was
  deferred.** Progress has leaked into the spec; the phase can no longer be
  measured against what it committed to. Move it to the STATUS checklist.
- **A slice landed with no notes doc**, because "it was only fixtures" or
  "it's all in the history entry". Its findings are now scattered, and the
  phase wrap has nothing to consolidate.
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
- **An inbox that survived its phase's grilling.** It was read and not
  drained, so it is now a permanent "future considerations" page — the exact
  thing the delete-on-drain rule exists to prevent.
- **An inbox entry that cannot say why its phase cares.** It is a passing
  remark filed as evidence, and it is what turns the file into a dumping
  ground nobody reads.
- **Answering "how does X work" requires reading four phase docs and knowing
  which one amended the others.** The keystone has passed and the centering is
  still up. Re-file by subject.
- **A code comment cites a phase number for something already built.** Either
  it is provenance (delete it) or it is a citation whose target should be a
  doc about the mechanism, not about a month.
- **A keystone review that deleted the phase docs without harvesting the
  rejected alternatives out of them first.** Those were the only category that
  could not be rebuilt, and they were the reason not to simply `rm` the
  directory.
