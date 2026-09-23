# Development process

What documents exist, where each fact goes, and what landing work obliges. This
describes the process; `pgdump_query`'s `docs/` tree is its worked example.

**The code is the ground truth.** Documents locate it quickly and record what it
cannot explain: why this shape rather than the obvious one, what was measured
and refused, what would reopen the question. Anything a reader gets by reading
the module belongs in the module, as rustdoc. **A fact is stated once and cited
everywhere else** — a decision in `decisions.md`, a number in `measurements.md`,
an external guarantee in an invariants register, what exists in `STATUS.md`. A
second copy is a future contradiction nobody can adjudicate.

**Document what *is*, not what *was***, the docs whose filename is a date
included. **Every standing constraint carries its own check**, and every check
its own tests: a rule nobody can mechanically verify is already being violated.
**A citation names a document and a section**, and the citation check resolves
every one, so run it after rewriting a heading. **A change to this document
earns no ledger row, slice number or history entry** — the rule text is its own
record.

## The loop

**1. Grill the phase.** Interview the maintainer until the design tree has no
unvisited branches, in rounds of at most three questions, each with a
recommended answer. **Write settled decisions into the docs after every round**:
one living only in the conversation dies with the context window. Finding facts
is the agent's job — anything the filesystem, upstream source, a container or a
test run answers. **Drain the phase's inbox first**; the grilling reads it first
and deletes it last. Protocol: `.claude/skills/grilling/SKILL.md`.

**2. Write the spec**, `docs/design/roadmap-P<N>-<slug>.md`: **what** the phase
will do and **why**, never how it lands in code, and **only for the current
phase** — later ones are sketched to corner-avoidance depth and say so. Editing
a spec to match what the code turned out to do erases the record of intent.

**3. Slice it.** Order slices so **each makes the next one's mistakes visible**;
cheap, no-code, evidence-gathering slices go first. Then, in the same change,
write the slice list into `STATUS.md` as an unchecked checklist and set the
phase's index row to `Current`.

**Size a slice by its review, not by its scope**: a slice pairing a
self-contained new module with a rework of an already-tested core path is two
slices, the confidence in each half differing, and bundling them forces the
review to accept both at one confidence. Split at spec time; the seam that
recurs is a row committing to a mechanism *and* to the evidence it needs, and
the evidence half lands first.

**4. Land a slice, write its notes** — `roadmap-P<N>.<M>-<slug>-notes.md`, as it
lands, even for a slice that lands no code, stating outcome: what the next phase
inherits and the negative results, never a restatement of the spec. Tick the box
in the same change and point it at the notes.

**5. Wrap the phase.** Consolidate the slice notes into one
`roadmap-P<N>-<slug>-notes.md`, delete the per-slice files, and delete the
checklist while setting the index row to `Complete` — one edit. **A wrap
opens with a repoint** over the phase's mechanisms (below). **After a
keystone the wrap is an audit, not a transcription**: mechanism decisions belong
in the register, so the wrap moves in whatever the slices decided that has not
reached it, and the notes doc keeps only the negative results and facts aimed at
the next phase. It may be short; it is still written, an absent one reading as a
skipped wrap.

**6. Grill again** before specifying phase N+1; a completed phase invalidates
guesses about later ones. **7. At the keystone**, fold the phase's decisions
into the register and delete its docs.

### Two phases in flight

A phase is discovered work and does not wait for a boundary: grill the second
one, give it its own `P<k>`, run it alongside. They must touch **disjoint
mechanisms** — the defence against a half-finished phase is that its slice list
says what is missing — and each keeps its **own spec and checklist**, a merged
one being unable to say which phase is short.

## The decision register

`docs/design/decisions.md` is **capped at 575 lines** of numbered entries, each
at most seven lines: **Decision**, **Why**, **Rejected** (and why), **Reopens**,
**Code** (the item), **Evidence** (a figure id, an invariant, a test).

- **`D<k>` is allocated on discovery, never renumbered and never reused**, as
  `P<k>` is: entries get cited, and a citation that renumbers lies.
- **Mechanism description is not an entry**: how the thing works lives in the
  code and its rustdoc, and an entry says why it is that shape.
- **A code comment cites `D<k>` in one line**, never re-arguing it or quoting a
  measured number — a measured quantity is stated once, in `measurements.md`,
  and an entry cites the figure.
- **The cap is the mechanism**: folding a phase's decisions in prunes those that
  no longer bind.

## STATUS.md

Present tense, rewritten in place, kept current **inside** the change that
alters implementation state. During a sliced phase it is a terse checklist —
`## P<N> progress`, then one `- [ ] **<N>.<M>** …` line per slice, each landed
one linking its notes doc. **A box is ticked only when the whole spec row is
delivered**; an unfinished one keeps its empty box and says what landed and what
did not. **Progress lives here, never in the spec.** Then **Not started**, and
one register; the deficiency register is its own file beside it (below).

- **Decisions worth another look** — calls made without the maintainer present
  that a person should still weigh in on: cautionary, never blocking, each
  stating the call, why it was made that way, and what reconsidering changes.
  **Five entries, hard**; a sixth waits for one to be closed or withdrawn.

  - **Closing an entry means filing it and then deleting it**, never editing it
    to record that it was reviewed. Where the review changed an artifact the
    fold-in has already filed it; where it **affirmed the call and changed
    nothing**, write the reasoning beside the mechanism it governs *first* — the
    only case whose content lives nowhere else. **The session that hears the
    answer closes it**, before the work the answer unblocked; deletion is the
    acknowledgement.
  - **Name the decision, or file it elsewhere.** If the honest answer to *what
    is the maintainer being asked to decide* is "nothing — they would nod", it
    is a known deficiency, an inbox entry or an out-of-band row, and goes there.

## Known deficiencies

`docs/status/deficiencies.md`, beside STATUS and present tense like it: one
indexed line per entry, at most eight wrapped lines, `- **KD<k>** — <one
sentence>`, a stance, `Detail: [<name>](<path>)`. `KD<k>` is allocated on
discovery and never reused; two letters, so it cannot alias a sibling
namespace's `D<k>`.

- **Each entry declares one of three stances**, since the difference decides
  whether anyone should act: **(a) deliberate tradeoff**, never to be worked
  because closing it gives up something chosen; **(b) owned by
  `<destination>`**; **(c) unowned**, in that word — a legitimate resting
  state, naming whatever would promote the entry. A `(b)` owner is read from
  the roadmap's phase index, so one owned by a `Complete`, `Struck` or
  unlisted phase drops to `(c)` unless a phase absorbs it.
- **The register is an index; the detail lives at the code marker** — one
  `deficiency: KD<k>` comment at the mechanism, in the `.rs` file the index
  line names, never in a document: locality is what makes a session touching
  the mechanism meet its limitations, and `deficiencies.py` resolves each
  indexed entry to exactly one marker and each marker back.
- **An entry is struck by the change that closes its last part**, not at a
  phase boundary — index line, detail and marker together. Partial closure
  **rewrites** it to what is still true, and a part closing into a *property*
  migrates beside its mechanism. The allocated range lives in a
  `<!-- deficiency-watermark: KD<k> -->` marker: a spent number the index
  does not carry *is* struck.
- **A slice that anticipates closing an entry, and that entry, name each
  other**, so **a slice that splits re-targets it**. A **ticked** line is a
  record and may cite a struck entry but not an unallocated number; an entry
  is present tense and may not name a ticked slice.

## History

`docs/status/history/YYYY-MM-DD.md`, one file per day, for two things: what a
future session should pick up mid-work, and a discovery that changed the plan.
Not a changelog, not a diary; full rules in `docs/status/history/README.md`.

- **An entry is short** — pointers and settled facts, under the cap its own
  check asserts.
  Reasoning that must outlive the day goes beside its mechanism; evidence is a
  figure, a test, an invariant or a `runs/` artifact, cited from here.
- **Write the end state, not the path to it**: no supposition-then-correction
  chains, no "resolved:"/"original note:" pairs, no in-progress status that has
  since resolved. One that has become actively misleading is corrected.
- **An entry carries yesterday's truth** once its day ends — until then the
  sessions writing it may condense it, and a later session may not touch it at
  all. It is not maintained, so a pointer
  that has outlived its target is not a defect; one that **never** resolved is a
  typo like any other. **Entries are deleted at each keystone**, once nothing
  outside `history/` cites them; a live doc needing a fact takes the fact.

## The assumptions register

Give it a project-specific name (`postgres-invariants.md`,
`runtime-invariants.md`). Every entry is one property of something outside your
control that a decision treats as guaranteed: **Claim** (falsifiable), **Proof**
(upstream source beats documentation beats observed behaviour, and observed says
so), **Scope limit**, **Verified against**, **Relied on by**, and **Re-verify**
— the literal command, without which the entry will not be checked. Add one the
moment a decision starts depending on external behaviour, and walk the file at a
new upstream version: a release can invalidate one quietly, and the bug surfaces
as wrong data rather than an error. The **compatibility matrix** is its sibling:
say in it that a status is coverage, not deficiency.

## Inboxes: facts filed by destination

Notes docs are filed by origin, which works for the next phase and fails for a
distant one. So a fact addressed to a phase with no spec yet goes in that
phase's inbox, `roadmap-P<N>-<slug>-inbox.md`, created at its first entry and
never as an empty stub. Three fields: the **fact**; **why this phase cares**, an
entry that cannot name a decision being a note to nobody; and **origin**, slice
and date, plus *contingent on* what would falsify it.

**The threshold is: no other home** — everything else the routing table below
places elsewhere, leaving only evidence from phase N that constrains a decision
in a phase far enough out to have no spec. **An inbox is drained, not
archived**: the grilling folds every entry into the spec or discards it as
stale, then **deletes the file**. Both sides need a trigger in `CLAUDE.md`: read
one before grilling a phase, file into one the moment a slice turns up a fact.

## Out-of-band work

A CLI ergonomics change, a defect fix that changes no decision — these fit one
session and belong to no phase, and making them slices corrupts what a slice
means. An item gets a number (`M1`, `M2`, …) and **one terse line**: date, what
changed, whether it blocks the open phase, and a pointer to the dated history
entry that says why. No spec, there having been no intent doc; **no notes doc,
because the history entry is the notes.** A finding that matters to a later
phase goes in that phase's inbox.

- **The admission rule is the load-bearing half.** An item is out-of-band only
  if it changes no decision any spec records *and* fits one session; anything
  else goes back through grilling → spec amendment → a numbered slice, or the
  ledger becomes where design work avoids review.
- **The number is allocated when the item is admitted, not when it lands**, the
  Date column staying empty until then — the ledger being the allocation
  authority, a session reading only its tail reissues a spent number.
- **Whether an item blocks is the admitting session's to say.** The row's
  `Blocks` column names the phase, is empty when the phase can be built around
  it, and is cleared when the item lands. The rows are a work queue as well as a
  record, so an unread tail is unstarted work — which is why the ledger stays
  **an index, not an account**, one line to a row, and moves out of the roadmap
  into its own file once reading it whole stops being automatic.

## Repointing: the record read against the code

Masonry outlives its centering by being repointed: failed mortar is raked out
and renewed, and no stone is touched. The stones here are the code; the mortar
is everything that says *why* — the register, the standing rules, the
invariants, the comments, `STATUS.md`. Mortar fails quietly: a claim is written
while true, the mechanism moves, and the copy that was not in the diff stays.
The wrap consolidates one phase's notes and the keystone strikes the
centering; neither reads the standing record against the code, and between
keystones nothing did. Repointing is that read, on a cadence set by how much
the record has grown rather than by phase boundaries.

**Three rules prevent most of what a repoint would otherwise find.**

- **A cap is a check.** Everything that accretes has a ceiling asserted by
  `scripts/repoint.py`: the register's line and entry caps, a dated entry at
  its stated length, `CLAUDE.md`, this document and each skill at the
  sizes that keep orientation cheap, no measured number in the register or in
  `STATUS.md`'s capability table, no phase provenance in a source comment.
  Adding under a full cap means striking; a session never raises one to fit
  what it is landing, and only the maintainer moves a ceiling, deliberately.
- **An argument is written once, where it will be struck.** A decision's
  reasoning lives in its `D<k>` entry, or in the dated entry while it is still
  moving. A standing document, a comment and the manual carry the conclusion
  and a citation. Reasoning written into a standing document is the copy nobody
  finds when the decision changes.
- **Subtract as you add.** The change that moves a fact hunts every copy before
  it lands: by handle where there is one (`D<k>`, `KD<k>`, a figure id,
  `I<n>`), by wording where there is not — and a claim with no handle is given
  one or deleted. A change that changed no decision and grew the record has
  usually restated something.

**When.** `scripts/repoint.py` reads a stamp in `STATUS.md` naming the commit
the record was last read against, and goes red once the live record — the
standing docs, the skills and the source comments; not the centering, not
history — has grown past its budget since. Red is the trigger; so is a wrap,
before its consolidation; a keystone includes one. It is never deferred to the
next keystone, which is how the record went unread for the life of a phase.

**What it does** (`.claude/skills/repoint/SKILL.md` is the procedure):

1. **Read blind.** For each module the record touched since the stamp, a
   reader given only the code and the record lines that name it returns every
   claim the code does not bear out. A reader who already knows what the
   record says will read it as true.
2. **Correct or strike.** A false claim is corrected where the code is right,
   or becomes a `KD<k>` where the record states the intent and the code falls
   short. An entry that no longer binds is struck, and a rejected alternative
   nobody would propose again goes with it.
3. **Trim to the caps**, and run every consistency check.
4. **Re-stamp**, and commit as one change.

It never changes what the code does, raises a cap, rewrites a dated entry, or
distils struck material into a subject-filed document. Unattended, a
correction stays inside "the record is refuted"'s three conditions and its
phase-local scope; what cannot be corrected there is listed in the day's entry
for the maintainer.

## The keystone: striking the centering

A masonry arch is built over *centering*, a temporary frame holding every stone
until the keystone drops in and the arch carries itself; phase docs are
centering. **The keystone is when the system's *shape* stops being in
question**: every use case has an implementation, however crude, and what
remains is expansion or ergonomics. The test is a reader — **can one who knows
the domain read the code and predict where a new feature goes?**

1. **Harvest**, then **prune to the cap.** Pull each decision the code cannot
   explain into `decisions.md` as a `D<k>` entry, and each negative result with
   it — the code records what was built, never what was tried and abandoned —
   then drop the entries that no longer bind.
2. **Retarget.** Repoint `CLAUDE.md`'s read-triggers. **This is where the token
   cost actually drops** — orientation cost is what the triggers pull into
   context, not what exists on disk.
3. **Sweep the tree.** Strip provenance, repoint or inline citations, de-number
   forward references. **Scope the sweep by the live doc set, never by the
   struck phase's own citations** — following those misses documents describing
   the phase in other words, above all the **inbox of a phase it never named**.
4. **Delete.** The spec, the notes, the ledger rows, the history entries nothing
   outside `history/` cites; set each struck phase's index row. Deletion removes
   the *second authority*, so nobody has to decide which copy wins. No prose is
   distilled into a subject-filed document: what survives is the code, the
   register, and the registers that were never phase-filed.

### The test each artifact must pass

An artifact survives if **it answers a question a future session will actually
ask, *and* reading it here costs less than re-deriving the answer from the code
plus `git log`** — failing the first is dead weight, failing the second a stale
cache of a cheaper computation.

Six categories fall out, and the mistake is treating the phase record as one
thing. **Provenance** ("landed in `P3.3`") is deleted; a **citation** into a doc
about to go is retargeted or its sentence inlined; **rationale** and a
**negative result** become a `D<k>` entry's `Why` and `Rejected` lines, the
second recoverable from nothing else; an **external fact** is untouched; a
**live obligation** is kept but re-pointed **by name**, the phase ahead being
liable to split, merge or drop. A measurement survives only with the command
that reproduces it.

### What a keystone review must not do

- **Renumber the remaining phases, or reuse a struck one's number**, which
  invalidates every surviving reference and makes every dated entry a lie.
- **Distil into the assumptions register or the compatibility matrix**, never
  phase-filed — though **their citations are still swept**, a pointer *out* of
  one into a struck doc being repaired, both being present tense.
- **Rewrite history, or repair its citations.** A dated entry asserts what was
  true on its date, so a pointer that has outlived its target still records the
  day accurately, and a citation check must exempt dated entries or go red at
  the first keystone. Not exempt: one that never resolved.
- **Freeze the register**, which records what is true now where `initial.md`
  records what we thought at the start; or **rewrite the manual**, already
  written for someone who will never read the source.

**What it costs** is the record of **intent** — you can no longer ask what phase
3 committed to and whether it delivered. Accept that deliberately.

### The out-of-band ledger is struck too

An out-of-band item is filed by *when* exactly as a phase is, so the ledger goes
the same way: a row is provenance and is **deleted**, a citation into it is
**retargeted** at the decision entry, a live obligation is **re-pointed by
name**, and the harvest reads the dated entry, there being no notes doc. **What
survives is a watermark, not a summary**: one line of spent numbers, as a
high-water mark, unlanded items' numbers being spent too.

## Where does this fact go?

| If the fact is… | It goes in… |
|---|---|
| How a mechanism works | the **code**, and its rustdoc |
| Why it is that shape, what was refused, what would reopen it | a **`D<k>`** entry |
| A measured quantity | **measurements.md**, once; everything else cites the figure |
| What exists right now | **STATUS.md** |
| A deficiency we know about and are not fixing now | the **`KD<k>` register** — one indexed line in `deficiencies.md`, the detail at the code marker |
| A limitation whose remedy the user already has today | beside the **mechanism**; it is a property, not a deficiency |
| A call made unattended, reviewed, and **affirmed with nothing changed** | the `D<k>` entry it governs, as its `Rejected` line — then the STATUS entry is deleted |
| Why we changed our mind, and the evidence | a **history** entry, cited from the doc holding the decision — never inlined into it |
| Something outside our control that we now depend on | the **assumptions register** |
| A rule that will still apply three phases from now | a **standing-constraint** doc, or a named roadmap section |
| Something a *distant, unspecified* phase will need to know | that phase's **inbox** |
| A one-session change that belongs to no phase | the **out-of-band ledger**, one line, pointing at a history entry |
| Something a *user* needs, with no rationale attached | the **manual** |

Status and history are separated by tense, not content. **A falsified claim is
corrected by the change that falsifies it**, not by a later slice that owns the
file — binding absolutely in the manual, read by someone who cannot check it.

## Naming and lifecycle

**Phase identity is `P<k>`, and it is not a position.** Phases are *discovered*,
so `P9` may be specified before `P5`, run beside `P4` and wrap first, none of
which needs an apology in the roadmap. **The sigil is permanent** — a label that
changes makes every history entry and commit message naming it wrong — and
numbers are allocated in discovery order and **never reused**, a struck or
abandoned phase's included. **The slug beside it is a caption**, free to be
wrong, to change, and to repeat an earlier phase's.

**Order lives in the roadmap's section order.** Head that file with an index
whose rows are the schedule, each carrying a state: `Sketched`, `Specified`,
`Current`, `Complete`, `Struck`. **A phase carrying a slice checklist is
`Current`, and a `Current` phase carries one** — the slicing sets the cell and
writes the checklist together, the wrap deletes the checklist and sets
`Complete` together. That is what lets a check tell a phase that ran and
finished from one nobody has sliced.

**Slice numbering.** A phase's planned slices are `P<N>.1`, `P<N>.2`, …, and
here the integer *is* honest: slice order is fixed at spec time inside one
phase. A third level (`P<N>.<M>.<K>`) is **earned, not planned** — a landed
slice shipped the wrong contract, or a mis-sized one's undelivered part becomes
its own increment. Prefer earning one over renumbering the tail, which
invalidates every reference; the mis-sized row is rewritten to the scope that
landed, the remainder moving to the new rows with the reasoning in a history
entry. A third-level remainder is the next increment under the same parent:
there is no fourth level, and a remainder never takes the next free phase-level
number, though a slice *admitted* after spec time does. **A phase whose first
slices exist to produce evidence says so in its spec**, its later numbers being
allocation order rather than schedule. **`initial.md`** is written at the
bootstrap, moved to `historical/` and never edited again — marked frozen there,
in the roadmap and in `CLAUDE.md`.

## Working unattended

An agent session with no maintainer present can land a slice; what it cannot do
is decide on its own that a risky change is acceptable, being short of a
reviewer rather than of capability. **Never mix high-confidence and
low-confidence work in one review cycle**: a diff holding both forces the
maintainer to accept the uncertain half to get the certain one, which is exactly
the review they were meant to provide.

**Stop at the last clean boundary when ambiguity meets a wide blast radius**: an
unattended session does not rework an already-tested core path on a judgement
call. **Stopping is not being blocked** — finish the unaffected work, leave the
box unticked with an honest entry, and write a history entry if the work is
mid-flight. **A judgement call that had to be made anyway goes under "Decisions
worth another look"**, not into silence or a blocking question.

**An answer may come from a stand-in rather than the maintainer.** A stand-in
may agree only where the recommendation **follows from something already
written** — a standing rule, a `D<k>` entry, an invariant, this document, a
precedent an earlier slice set — and **escalates whenever the call would bind
beyond the open phase**: a standing rule or invariant changed, a `KD<k>`
re-targeted onto a phase nobody has grilled, a spec rationale reversed, a rule
added to a standing-constraint doc, to `CLAUDE.md` or to this one. Inside the
phase the work sits under an approved spec; outside it, nothing. **The list is
illustration; the criterion is the rule** — what escalates is a **rule** added
or changed, not an edit re-describing what the project already has.

**A stand-in may also find the record *wrong*, and deferring to it is then the
failure**: a record's authority is the maintainer's approval against the
evidence of its day, and evidence postdating that was approved by nobody. So
there is a further disposition, **the record is refuted**, on three checkable
conditions — **settled without a new measurement** (code read, arithmetic done,
a reading already in the tree; needing a run, escalate); the **falsifying
artifact named exactly** (file and line, a figure's cell, a logged number, the
commit that moved the premise); and **the record amended in the same round**,
working around a false document being what keeps the trap.

**The escalation boundary is unchanged**: what may be refuted in-loop is
**phase-local** record — the open phase's spec, its slice rows, a deficiency
entry, a claim in a dated entry — while a standing rule, an invariant, a
standing-constraint doc, `CLAUDE.md` or a skill **escalates even when provably
wrong**. That holds because the exchange is **recorded verbatim**, and every
closure files its reasoning beside the mechanism.

## CLAUDE.md vs. CLAUDE.local.md vs. docs

Three files, three audiences, split by **portability**. **`CLAUDE.md`** is
checked in and travels with the repo: the command reference, the standing rules,
and **pointers to the docs with a trigger attached to each**. Not a bibliography
— a pointer says *when* to read the thing, which is what turns a doc from
something an agent might find into something it reliably reads at the right
moment. It also holds rules about how the agent works, and operational ones of
the form "the obvious way is expensive in a way you cannot see".

**`CLAUDE.local.md`** is gitignored and describes *this machine*: hardware,
which volume is which, absolute paths to datasets and upstream checkouts, the
container runtime, local scratch services. The test: would this sentence be
wrong, or harmful, on a different machine? A *procedure* referencing a local
path splits across both — the procedure in `CLAUDE.md`, the path in
`CLAUDE.local.md`, named rather than inlined. **`docs/`** holds the thinking;
`CLAUDE.md` never duplicates it.
