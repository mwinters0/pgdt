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

**What the process governs is iteration on the product, not on itself.** Every
obligation below — the ledger's admission rule, slice numbering, what landing
work requires — applies to changes to the software and to the design record
that describes it. A change to *this document* is not one of them: it earns no
ledger row, no slice number, and no history entry. The rule text is its own
record. What that gives up is the ability to date a wording change or trace a
dangling reference back to the revision that orphaned it, which is a small and
bounded loss; what it avoids is a meta-process with its own bookkeeping, which
is unbounded.

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

**2. Write the phase spec.** `docs/design/roadmap-P<N>-<slug>.md`. This is
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
in STATUS, never in the spec" below. **Set the phase's index row to `Current`
in that same change**, since the checklist and the state are the two halves of
"this phase is in flight" and either alone is a lie — the mirror of the wrap's
`Complete`, in step 5.

**4. Land a slice, write its notes.** Each slice gets
`roadmap-P<N>.<M>-<slug>-notes.md`, written as it lands, while it is
fresh — **including a slice that lands no code at all**, because a
fixture-only or evidence-only slice is precisely the kind whose findings the
next slice inherits. In the same change, tick the slice's box in `STATUS.md`
and point it at the notes doc.

**5. Wrap the phase.** Consolidate the per-slice notes into one
`roadmap-P<N>-<slug>-notes.md` and delete the per-slice files. Rewrite
`STATUS.md`. **Deleting the phase's checklist and setting its index row to
`Complete` is one edit**, because from that moment the index is the only thing
that can say the phase ran and finished — see "Known deficiencies" below, which
reads that cell.

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

### Two phases in flight

The loop above is drawn serial and usually runs that way, but a phase is
discovered work, and the work does not wait for a boundary. Finding a second
phase's worth of it mid-phase is normal: grill it, give it its own `P<k>`, and
run it — including alongside the phase already open. Do not renumber the
roadmap to make the new one look next; the number is identity and the roadmap's
section order is the schedule.

**Two conditions, and the first is the real one.** The phases must touch
**disjoint mechanisms**, because the whole defence against a half-finished
phase is that its slice list says what is missing, and two phases editing one
mechanism produce a state neither checklist describes. And each keeps its
**own spec and its own STATUS checklist** — a merged checklist cannot say
which phase is short.

They then wrap independently, in whatever order they finish, which is a wrap
like any other.

**The trigger that makes this worth allowing** is the long-running job. A phase
gated on hour-scale runs has real idle time in it, and a disjoint second phase
is a better use of that time than either idling or widening the first phase's
scope to fill it.

---

## The doc set

| Path | Answers | Must not contain |
|---|---|---|
| `README.md` | What is this, what state is it in, where are the docs | Anything duplicated from the docs it indexes |
| `CLAUDE.md` | How an agent works in this repo: commands, doc pointers **with read triggers**, standing rules | Design rationale; implementation status; anything machine-specific |
| `CLAUDE.local.md` | Facts about *this machine and this operator* | Anything another machine would need |
| `docs/design/historical/initial.md` | The original handoff, frozen | Edits. It is history, not a live doc |
| `docs/design/roadmap.md` | Project goals, standing policies, the phase index | Full phase specs (they get their own files) |
| `docs/design/roadmap-P<N>-<slug>.md` | **What** phase `P<N>` does and **why** — the binding spec | How it landed in code |
| `docs/design/roadmap-P<N>-<slug>-notes.md` | **How** it landed: module map, and the facts later phases inherit | Restatement of the spec; a changelog |
| `docs/design/roadmap-P<N>.<M>-<slug>-notes.md` | The same, for one slice, until the phase wraps | Anything that should have gone in the spec |
| `docs/design/roadmap-P<N>-<slug>-inbox.md` | Facts an *earlier* phase found that `P<N>`'s grilling must not miss | Anything with a proper home elsewhere; speculation about `P<N>`'s design |
| `docs/design/<architecture>.md` | How the built system works, filed by **subject** — exists only after a keystone | Phase history; what any phase was committed to |
| `docs/design/<invariants>.md` | Every external behaviour a decision assumes, with proof | Assumptions without a re-verification step |
| `docs/design/<compatibility>.md` | Which external variants are tested / untested / unsupported | Untracked "probably fine" rows |
| `docs/design/<standing-constraint>.md` | A rule that cuts across all phases (e.g. layering) | Phase-scoped decisions |
| `docs/status/STATUS.md` | What **is** built, right now | How it got that way |
| `docs/status/history/YYYY-MM-DD.md` | Mid-work pickup state; discoveries that changed the plan | Routine progress; narration of the day |
| `docs/manual/` | How to use the thing, for someone who will never read the source | Design rationale |

### The four that carry the process

**The roadmap** is an index plus the standing decisions, and it is the one
place a phase's **position** is stated: its section order is the schedule,
because a `P<k>` identifier deliberately does not encode one. Head it with an
index — identifier, slug, state, pointer — so the order is read off the table
rather than inferred from the numbers. Phases that have been
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
## P3 progress

- [x] **3.1** The `objects` fixture schema — the TOC kinds neither existing
      schema produces, plus large objects. No library code. Notes:
      `docs/design/roadmap-P3.1-objects-fixture-notes.md`
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
- **Known deficiencies** — a register of the deficiencies that are known, each
  with what it costs. This is the section that stops the next session from
  re-discovering a deliberate limitation as a bug, and it is worth more than the
  checklist above it.

  It is a **register with a stable identifier per entry** (`KD1`, `KD2`, …),
  allocated on discovery and never reused, for the same reason phase numbers
  are: an entry gets cited, and a citation that renumbers is a citation that
  lies.

  **The sigil is two letters on purpose.** Single letters are a scarce
  namespace shared by every project templated from this document — `P` for
  phases, `M` for out-of-band items, `I` for invariants, `L` for layers, and
  `D` for a decision log, which is a stronger claim on that letter than
  deficiencies have. Take two rather than contend for one. It costs nothing:
  `KD7` and a sibling project's `D7` cannot alias each other, because there is
  no word boundary between `K` and `D`, so a pattern matching one never matches
  inside the other. Name the concept with a word formal enough that ordinary
  prose does not reach for it — the first attempt here was "gap", which made
  every document that used the word in its plain sense disclaim it, and that is
  prose paying for a naming mistake.

  Eight rules keep it working, and the first is the one whose absence is
  hardest to see:

  - **Each entry declares one of three stances**, because they are not one kind
    of thing and the difference decides whether anyone should act. **(a)** A
    consequence of a deliberate tradeoff — it will never be worked, because
    closing it means giving up something that was chosen. **(b)** A defect with
    a known fix and a named destination. **(c)** A defect with a known fix and
    no owner. The `(a)`/`(c)` split is the one that costs most while it is
    invisible: re-proposing an `(a)` wastes a session on a question that was
    already settled, and a `(c)` is unowned work that reads identically to it.
  - **Unowned is a legitimate resting state, said in those words.** An entry
    is not obliged to acquire a roadmap row, and forcing one manufactures intent
    nobody holds. What must not happen is that the absence of a plan cannot be
    told from an oversight. Where something specific would promote the entry —
    a real input hitting it, a phase that would absorb it — the entry names
    that trigger.
  - **A property is not a deficiency.** If the remedy is already available to
    the user today, it is how the system works: it belongs beside the
    mechanism, with no register entry. A section that mixes "this is how it
    works" with "this is worse than we want" teaches the reader to skim both.
  - **The register is an index; the detail lives beside the mechanism.** The
    property being protected is that a session touching a mechanism cannot miss
    that mechanism's limitations — not that every session reads every entry.
    `CLAUDE.md`'s read-triggers already oblige reading the mechanism's own
    section before changing it, so an entry written there is read *because* the
    session is touching it, where a central list has to be recognised as
    relevant first. Locality is therefore the stronger guarantee, and it is
    also what keeps this section readable as the set grows: `STATUS.md` carries
    one line per entry — identifier, one sentence, stance, destination — and the
    paragraph explaining it sits wherever that mechanism is documented.
  - **The index is reconciled mechanically, not by discipline.** An index that
    has drifted from its detail is worse than either alone, and "someone will
    remember" is the assumption every other register in this process was built
    to avoid. A check resolves every `KD<k>` in the index to its entry and every
    entry back to the index, and fails on either half — and it covers the
    slice pairing below by the same argument, since a pointer nobody resolves
    is exactly a half of the index nobody checks. Where the *code* would
    otherwise mislead — a line that reads as a complete, deliberate choice and
    gives no sign that a limitation hangs off it — mark it with the identifier
    and let the same check cover markers too, so one outliving its entry is an
    error rather than a slow lie.
  - **An entry is struck when its last part closes, in the change that closes
    it** — not at a phase boundary. Allocation happens on discovery rather than
    at a boundary, and discharge mirrors it: the register's whole job is that a
    session touching a mechanism meets its limitations, and an entry describing
    a defect the code no longer has runs that backwards, sending someone to
    code around nothing or to fix it twice. It is a present-tense document, so
    the moment the last part closes it is already wrong. **Partial closure
    rewrites the entry rather than annotating it** — the entry says what is
    still true, never "four of five remain" — and a part that closes into a
    *property* migrates beside its mechanism rather than being deleted, since
    the property is what is now true. Striking is one atomic change: index
    line, detail paragraph, and any code marker, which the reconciliation
    already fails on if you drop one. **The number is spent, never reused**, so
    leave a one-line watermark saying which are allocated and which were
    struck; an identifier cited in an old commit message must still resolve to
    something, and "struck" has to be tellable from "typo".

    **The watermark carries a marker, and resolution keys off the range rather
    than off the named struck entries.** Those names are provenance — they
    answer *when*, and `git log` answers *when* — so a keystone deletes them and
    leaves the high-water mark exactly as the out-of-band ledger does, which a
    check reading a named list would not survive. Reading only the mark tolerates
    both forms by construction: a number at or below it that the index does not
    carry *is* struck. The mark therefore goes in an
    `<!-- deficiency-watermark: … -->` marker rather than in the sentence around
    it, which is rewritten at every strike; deriving it instead as the highest
    indexed entry breaks exactly when the highest-numbered entry is struck,
    which is when it is needed.
  - **A slice that anticipates closing an entry, and that entry, name each
    other.** The forward reference alone rots: a slice list is rewritten as a
    phase is grilled, split and re-sliced, and the entry that named a slice
    number quietly starts pointing at work that no longer exists. Naming both
    directions makes the pair self-checking — landing a slice is also the
    moment its entry is re-read, and **a slice that splits is obliged to
    re-target the entry** rather than leaving it aimed at a number that has
    changed meaning. It also answers, at spec time rather than at wrap, the
    question a `(b)` stance raises and cannot itself answer: *which* piece of
    the owning phase actually discharges this.

    **The reconciliation carries this half too**, so a re-slice fails the check
    instead of owing a re-target on discipline — which is what a self-checking
    pair means, since nothing else notices that a slice number has quietly
    changed meaning. Two boundaries keep it from firing where there is no
    obligation, and both are read from the checklist rather than configured: a
    phase with **no slice list** is not sliced yet, so an entry owned by it
    names no slice and is not asked to — writing the list is what turns the
    obligation on; and a **ticked** line is a record of what a slice closed, not
    a promise, so it is not held to a pairing the entry beside it has already
    been rewritten out of.

    **The two directions are asymmetric, and deliberately so.** A checklist line
    is a record, so its identifier is a *citation* and resolves against the
    allocated range: it may name a struck entry — that is what the watermark is
    for — and may not name a number nobody allocated. An index entry is present
    tense, so it may name only live slices, and **an entry naming a ticked slice
    is an error**: partial closure rewrites the entry in the change that ticks
    the box, so either that slice closed its part and the entry should no longer
    name it, or it did not and the entry is aimed at the wrong slice. Making it
    fail is what enforces the partial-closure rewrite mechanically, and that is
    the rule most likely to be skipped — striking an entry is a visible
    ceremony, rewriting one because a single row closed is quiet.
  - **A `(b)` entry's owner is read from the roadmap's phase index, which is
    where "this phase finished" lives.** A completed phase's checklist is
    deleted at its wrap, so an entry owned by it otherwise reverts to "no
    checklist means not sliced yet" and goes quiet at the moment its pointer is
    most wrong — still naming a slice number that now exists nowhere. That is
    why the index carries a `Complete` state at all: it is the only cell that
    distinguishes *not yet sliced* from *sliced, run and finished*, and a
    keystone that would strike the phase may be years after its wrap. A `(b)`
    entry owned by a phase that is `Complete`, `Struck`, or absent from the
    index **fails, and the failure says the entry drops to `(c)` unless a phase
    actually absorbs it** — a finished phase is not a named destination, and
    inventing a new owner to preserve the stance manufactures intent nobody
    holds. Keeping a completed phase's checklist until the keystone is the
    alternative and needs no index at all; it is rejected because phases
    complete routinely while a keystone lands once or twice, so the ticked boxes
    accumulate, putting chronology back into the document the keystone exists to
    take it out of.

    **The cell a person sets is the whole discipline, so pin it at both ends.**
    A wrap sets `Complete` and deletes the checklist; a *slicing* sets `Current`
    and writes one. Stated as a rule the check can hold: **a phase carrying a
    slice checklist is `Current`, and a `Current` phase carries one.** That is
    what makes the index legible without reading it against `STATUS.md` — the
    three states a phase passes through each have a distinguishing artifact, so
    a transition that half happened is visible from either side. A wrap that
    deletes the checklist and forgets the state leaves `Current` with nothing
    under it; a slicing that writes the checklist and forgets the state leaves
    `Specified` with a checklist under it. Both fail.

    Without that rule the edge is real and one-sided: `Specified` would mean
    both *not yet sliced* and *sliced, run, and wrapped by someone who forgot
    the cell*, and only the second is a problem. Splitting `Specified` into
    `Specified` and `Sliced` was the alternative considered, and it is the same
    rule under a new word — `Current` was already in the vocabulary, meaning
    exactly this, so the split bought nothing the index did not already have.
    The cost is one cell edited when a phase is sliced, which is the same cost
    the wrap already pays.
- **Decisions worth another look** — calls made without the maintainer
  present that a person should still weigh in on: cautionary and
  informational, never blocking. Each entry states the call, why it was made
  that way, and what would change if it were reconsidered. This is the
  pressure valve that makes unattended work honest rather than silent;
  without it the choice is between stalling and burying the decision in a
  diff.

  Four rules keep it from silting up. The first is the one that fails on its
  own, because it is the only one whose moment is a conversation rather than a
  file:

  - **Closing an entry means filing it and then deleting it** — never editing
    it to record that it was reviewed. Where the review changed an artifact —
    the call reversed, sharpened into a design doc, or promoted to a numbered
    item — the fold-in has already filed it, so delete the entry. Where the
    review **affirmed the call and changed nothing**, its reasoning has no
    home yet: write it beside the mechanism it governs, as a
    rejected-alternative paragraph, *then* delete. That second case is what
    actually produces sediment, and it is invisible without this rule, because
    it is the only kind whose content lives nowhere else and so cannot
    honestly be deleted on the spot.
  - **The session that hears the answer does the closing**, in that same
    session, before the work the answer unblocked. Deletion is the
    acknowledgement: nothing else in the repo records that a review happened,
    so an entry left standing is indistinguishable from one nobody read.
  - **Five entries, hard.** A sixth is not written until one is closed or
    withdrawn. The cap *is* the mechanism, not a target to stay under: this
    section's whole value is being short enough to read every session, so a
    list long enough to need an index has already stopped working as a flag
    and as a record at the same time.
  - **Name the decision, or file it elsewhere.** Before writing an entry, say
    what the maintainer is being asked to decide. If the honest answer is
    "nothing — they would nod", it is not a decision: it is a known deficiency,
    an inbox entry, or an out-of-band row, and it goes there now. This is the
    out-of-band ledger's admission rule applied to the milder version of the
    same failure — that ledger becomes where design work hides from review,
    and this section becomes where observations go to avoid being filed.

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

**Keep the matrix and the deficiency register from bleeding into each other.**
They answer different questions — the matrix says what has been *exercised* and
what is in scope, the register says what is *wrong* and who owns it. Say in the
matrix that a status is coverage rather than deficiency, and that a row earns
an identifier only by naming one. Without that, an `unsupported` row reads as a
deficiency nobody filed.

### Inboxes: facts filed by destination

Notes docs are filed by **origin** — "what `P3` learned". That works for
the next phase and fails for a distant one: a fact `P3` turned up that
`P7` needs ends up in `P3`'s notes, and nothing prompts anyone to read
those when `P7` comes up. It is the "doc pointer with no trigger" failure,
applied to facts instead of documents.

So a fact addressed to a phase that has no spec yet goes in that phase's
**inbox**, `roadmap-P<N>-<slug>-inbox.md`, created the moment it gets its first
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
is a roadmap "Future" item. A known deficiency is a register entry — indexed
in `STATUS.md`, detailed beside the mechanism.
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

**The number is allocated when the item is admitted, not when it lands.** An
item discovered mid-phase and queued for later gets its `M<k>` and its row at
the moment it is admitted, with the Date column empty until it lands. Deferring
allocation to landing costs two things: a queued item has no handle, so every
doc that wants to point at it points at a history entry's heading instead; and
the ledger stops being the allocation authority, so the next session reads the
last row and reissues a number already spoken for. The row is what records the
allocation, which is why it goes in early rather than the number being held
somewhere else.

**The admission rule is the load-bearing half.** An item is out-of-band only
if it changes no decision any spec records *and* fits one session. Anything
that changes a decision goes back through grilling → spec amendment → a
numbered slice. Without that rule the ledger becomes where design work goes to
avoid being reviewed, which is the failure mode it has to be built against.

The ledger is an index, not an account: its lines stay one line, and the detail
lives in the history entry. It grows until a keystone, which strikes it along
with the phase docs — see "The out-of-band ledger is struck too" below. Between
keystones, treat it as permanent.

---

## The keystone: striking the centering

A masonry arch is built over *centering* — a temporary wooden frame that holds
every stone in place while the arch is incomplete. The frame is not part of the
arch and was never meant to be. When the keystone drops in, the arch carries
itself, and the centering is struck: removed entirely, because from that moment
it holds nothing up and only obstructs the space beneath.

Phase docs are centering. While a phase is in flight its spec is a contract
under review and its notes are how a slice hands off before the phase can
consolidate; while the system is half-built, "module C is here because `P6`
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
suggests: a reader who finds the `P2` answer has no way to know a `P3`
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
| **Provenance** | "landed in `P3.3`", "bumped to v5 in `P3.2.2`", "`P1`'s default" | **Delete.** It answers *when*, `git log` answers *when*, and nobody asks. |
| **Citation** | "(`roadmap-P3-<slug>.md`, "Span boundaries")" | **Retarget, or inline the sentence.** A pointer into a deleted doc is worse than no pointer: a session spends a tool call following it. |
| **Rationale in situ** | "rejected — it would leak a mapping concern into L1's event contract" | **Keep; strip the phase number.** The reason is durable, the number is not. |
| **Negative result** | "an earlier attempt put a start floor on `Builder`; it tiles, but…" | **Keep, and promote.** Not recoverable from the code *at any price* — the code records what was built, never what was tried and abandoned. |
| **External fact** | the assumptions register; "`--with-statistics` does not exist" | **Keep untouched.** Never phase-filed to begin with. |
| **Live obligation** | "reserved; not populated until `P7`" | **Keep; re-point by name, not by number.** The obligation is stable; the numbering ahead of the keystone is about to be re-grilled. |

### The out-of-band ledger is struck too

An out-of-band item is filed by *when* exactly as a phase is, so the ledger is
centering by the same argument and goes the same way — otherwise the one part
of the doc set that is still a chronology outlives the review that removed
every other one.

It runs through the same six categories, and the split is not a blanket
delete. A ledger row is provenance: **delete**. A citation that resolves into
the ledger — "(the out-of-band ledger, `M4`)" — is a pointer into a row about
to disappear, so **retarget it at the mechanism's section or inline the
sentence**. A paragraph carrying a live obligation, such as one recording that
an item's *finding* was filed into a phase's inbox, is **kept and re-pointed by
name**.

**The one asymmetry with phase docs is the source you distil from.** Out-of-band
work has no notes doc by design — the history entry is the notes — so where a
phase sweep reads a notes doc, this one reads the dated entry. In a project
that has been filing each item's mechanism into the architecture doc as it
landed, that is a check rather than a harvest; where it has not, the entry is
the only place the mechanism is written down, and it is the last chance to
move it.

**What survives is a watermark, not a summary.** One line saying which numbers
are spent — "M1–M15 are struck; nothing below M16 is reused." Its job is
identical to the line saying phase numbering continues from `P4`: it is a live
obligation, not a record of what happened, and it exists so that an identifier
appearing in a history entry or a commit message can never be ambiguous.
Numbers already allocated to items that have not landed yet are spent too; word
it as a high-water mark rather than as a range that claims everything below it
is done.

The direction of a reference decides which of those rows it lands in.
**Backward** — naming a struck phase — is provenance or citation, so it is
deleted or retargeted at a subject; nothing is served by shortening it. **Forward**
— naming a phase not yet built — is a live obligation, and it is carried **by
name rather than by identifier**, because the phase ahead is about to be
re-grilled and may be split, merged or dropped: "the scan-performance work"
survives that and `P7` does not.

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

- **Renumber the remaining phases, or reuse a struck one's number.** Same
  reason the process already prefers earning a `P<N>.<M>.<K>` over renumbering
  a tail: it invalidates every surviving reference — inbox filenames
  especially — and erases the record that anything happened. History holds the
  numbers and may not be rewritten, so a renumbering makes every dated entry a
  lie. Phases keep their identifiers; the sequence just starts further in.
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
| A call made unattended, reviewed, and **affirmed with nothing changed** | beside the mechanism it governs, as a rejected-alternative paragraph — then the STATUS entry is deleted |
| What exists right now | **STATUS.md** |
| A deficiency we know about and are not fixing now | the **deficiency register** — one indexed line in STATUS, the detail beside the mechanism |
| A limitation whose remedy the user already has today | beside the **mechanism**; it is a property, not a deficiency |
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

**The manual's transcripts are illustrations, not captured runs.** Each one
shows every section of an output at once, over a dump chosen to have something
in each — which no fixture is. So a change to what a command prints re-reads
the manual for its **claims**, not for its numbers: the prose that says what a
figure *means* can go false and must be checked, while the numbers stand
because no run stands behind them and none is contradicted. Re-basing an
example on a real run is a trade, not an upgrade — it buys a number someone
could re-take and costs the sections the illustration was built to show.

---

## Naming and lifecycle

```
docs/
  design/
    historical/initial.md                          frozen at bootstrap
    roadmap.md                                     lives forever
    roadmap-P1-mvp.md                              written at phase start
    roadmap-P1-mvp-notes.md                        written as P1 lands
    roadmap-P2-typed-columns.md
    roadmap-P2.3-resolution-notes.md               deleted at P2's wrap
    roadmap-P2-typed-columns-notes.md              consolidated at wrap
    roadmap-P7-scan-performance-inbox.md           filed early, drained at P7's grilling
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

**Phase identity is `P<k>`, and it is not a position.** A phase is a grouping
of work that gets *discovered*, so phases are only partially ordered when they
are planned and fully ordered only in hindsight, as they land. A label has to
carry identity; a bare integer also asserts a position, and it asserts one no
matter what the surrounding prose says — so the label carries a sigil, which
is the cheapest thing that makes a reader stop counting with it. `P9` may be specified before `P5`, run beside `P4`, and wrap
first; none of that makes it misnamed, and none of it needs an apology in the
roadmap.

**The sigil is permanent.** It is a statement about what kind of thing the
number is, not a maturity marker, so it is never dropped — a label that
changes is a label that every history entry and commit message now names
incorrectly, and those may not be rewritten. Numbers are allocated in
discovery order and **never reused**, including for a phase that was struck at
a keystone or abandoned before it was specified.

**Order lives in the roadmap's section order**, which is the only place it is
true. Head that file with an index whose rows are the schedule, and give each
row an explicit state — sketched, specified, current, complete, struck —
rather than leaving maturity to be inferred from how large the number is.

**Two of those states are set in the same change as a `STATUS.md` edit, and
neither is optional.** `Current` is set when the phase is sliced, in the change
that writes its checklist; `Complete` at the wrap, in the change that deletes
it. Together they are what lets a reader — and a check — tell a phase that ran
and finished from one nobody has sliced yet, which no other cell in the table
can say once the checklist is gone.

**The slug is informal.** `P<k>` alone resolves; the slug beside it in a
filename or a heading is a caption, there because
`roadmap-P7-scan-performance-inbox.md` tells you what you are filing into and
`roadmap-P7-inbox.md` does not — which matters most for an inbox, whose write
trigger fires mid-slice while you are thinking about something else. Because it
carries no identity, a slug is free to be wrong: it may change when grilling
reshapes the phase, and it may repeat a slug some earlier phase used, since
`P<k>` is what distinguishes them. **Every open phase carries one** — open
meaning specified, in flight, or merely sketched. A struck phase carries
neither: its docs are gone and the architecture doc holds its mechanisms by
subject.

**Slice numbering.** A phase's planned slices are `P<N>.1`, `P<N>.2`, …, and
here the integer *is* honest: `process`'s own rule orders slices so that each
one makes the next one's mistakes visible, and that order is fixed at spec time
inside a single phase. Numbering is kept exactly where it asserts something
true and dropped one level up, where it does not. A third level
(`P<N>.<M>.<K>`) is **earned, not planned**, in one of two ways: a slice
that already landed turns out to have shipped the wrong contract, and fixing
it is its own increment; or a slice turns out to have been mis-sized, and the
part that did not land becomes its own increment. Do not pre-allocate them —
the fact that they were earned rather than planned is itself information about
where the plan was weak, and the spec's slice table should say so. Prefer
earning a third level over renumbering the tail: renumbering invalidates every
reference to a slice number and erases the record that the split happened at
all.

A mis-sized slice's spec row is **rewritten to the scope that actually
landed**, with the remainder moved into the new `P<N>.<M>.<K>` rows. That is a
decision change, not progress-tracking — the reasoning goes in a history entry
and the spec's slice table says the split was earned. It is the one case where
the finished half is legitimately ticked.

**Consolidation at wrap** is not optional cleanup. Per-slice notes exist so a
slice's detail has somewhere to go while it is fresh, without waiting on the
phase; leaving five of them behind means the next phase reads five overlapping
partial accounts instead of one.

**A wrap after a keystone is an audit, not a transcription.** Once a
subject-filed architecture doc exists, the fact-routing table above sends
mechanism facts *there*, not into the phase notes — so consolidating a phase's
slice notes verbatim rebuilds the second authority the keystone was run to
remove. The wrap instead checks the architecture doc for anything the slices
learned that has not reached it, moves that in, and leaves the consolidated
notes doc holding what subject-filing has no home for: the phase's negative
results, and facts addressed at the next phase that are not already inbox
entries. That doc can legitimately be short. It is still written, even then —
an absent notes doc cannot be told from a skipped wrap, and "slice notes
surviving past the phase wrap" is a smell someone will look for.

**After a keystone**, the `roadmap-P<N>-*` files for completed phases are
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

**Writing an entry and closing one are different sessions' jobs, and neither
is the keystone's.** The unattended session writes; whichever session hears
the maintainer's answer files the reasoning and deletes the entry, per
"Decisions worth another look" above. A keystone that arrives to find a
backlog there has found a filing failure, not a housekeeping chore — every
entry in it was answered months earlier and kept because answering left no
trace.

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
   list with everything past `P1` sketched at corner-avoidance depth only.

4. **Write `docs/design/roadmap-P1-<slug>.md`** — the first phase spec, in
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
- **A "Decisions worth another look" entry describes its own review** — "was
  reviewed on D and stands". It was closed and then kept. Either the fold-in
  already filed it, in which case delete it, or the review affirmed the call
  and changed nothing, in which case its reasoning has never been written down
  anywhere and must go beside the mechanism before the entry does.
- **That section runs past five entries.** It is no longer short enough to be
  read, so it has stopped buying the flag *and* the continuity it exists for —
  and a section nobody reads cannot be the pressure valve unattended work
  depends on.
- **A known deficiency that cannot say which of its three stances it is.**
  The classification is what tells a reader whether to act; without it the
  entry is prose that will be re-litigated, which is the thing the section
  exists to prevent.
- **A register entry's explanation lives in STATUS rather than beside its
  mechanism.** The
  index has become the document. It will be skimmed, and the session that is
  actually editing that mechanism will not see it.
- **A register entry whose remedy is already available to the user.** It is a
  property filed as a deficiency, and it dilutes every real entry beside it.
- **A code marker naming a deficiency that no longer has an entry**, or an
  entry nothing resolves to. The reconciliation is not being run, so the
  register has started lying in whichever direction is not checked.
- **A register entry whose defect the code no longer has.** It outlived its
  closure, and the reconciliation cannot see it — every identifier still
  resolves. The next session reads a limitation that is gone and either codes
  around it or closes it a second time.
- **A register entry with a `(b)` stance that names a phase but no slice**,
  once that phase has been sliced. Nothing will re-read the entry at the moment
  it comes due, and a re-slice has nothing obliging it to re-target.
- **A `(b)` entry owned by a phase that has finished**, or that names a slice
  whose box is ticked. Both are the same failure seen from two sides: the entry
  is present tense and its pointer is aimed at work that is over, so it is
  either owned by whoever picks it up next or it is `(c)` unowned.
- **A phase's index state and its slice checklist disagree** — a `Current` row
  with no checklist, or a checklist under a row that still says `Specified`.
  Half a transition landed: either the wrap deleted the checklist and left the
  state, or the slicing wrote the checklist and left it. The register reads that
  cell, so the half that is missing is the half nobody will notice.
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
- **The roadmap explains why a phase's number is out of order.** The
  explanation is the artifact: an identifier that needs an apology is being
  read as a position by the very document that defines it. State the rule
  once and delete the apology.
- **A phase identifier changed** — a sigil dropped once the phase was
  specified, a tail renumbered, a struck number reused. Every dated entry and
  commit message naming it is now wrong, and none of them may be rewritten.
- **A slug is being defended.** Arguing about whether a phase's slug will still
  be accurate, or whether some later phase might want it, means the slug is
  carrying identity it was never given. `P<k>` resolves; the slug is a caption.
- **A keystone review that deleted the phase docs without harvesting the
  rejected alternatives out of them first.** Those were the only category that
  could not be rebuilt, and they were the reason not to simply `rm` the
  directory.
