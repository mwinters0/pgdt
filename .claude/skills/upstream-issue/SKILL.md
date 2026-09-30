---
name: upstream-issue
description: Record a dependency's defect or limit that we work around or wait on upstream to fix, in `docs/status/upstream.md` — what an entry holds, where its code markers go, and when it is rewritten or struck. Use when a session works around a bug in DataFusion, arrow, chrono or any other dependency, leaves a limit in place only upstream can lift, or finds upstream's state of one has changed.
---

**Invoke the `process` skill first** when the entry comes with a `KD<k>` or
lands inside a slice: those rules govern the deficiency and the change. The
register is `docs/status/upstream.md`; upgrading a dependency and checking the
register against it is the `upgrade-deps` skill.

**What belongs.** A defect or limit whose fix belongs to a dependency, which we
are waiting on: we work around it, or we leave the limit in place because only
upstream can lift it. It is added **in the change that adds the workaround or
first leans on the limit**, not later. A behaviour we merely depend on is an
invariant (`docs/process.md`, "The assumptions register"), not an entry; one
upstream calls correct and we disagree with is an entry only while we are
asking for the change.

**An entry** is `## UF<k> — <title>`, then six fields, each `- **<field>.**`,
in this order — `scripts/upstream.py` asserts the shape:

1. **The issue.** What is wrong or missing upstream, and what a user of ours
   meets because of it. Where it is also a deficiency of ours, name the
   `KD<k>`; the mechanism's detail stays at that entry's code marker, never
   restated here.
2. **Upstream.** A link to every issue, PR, discussion and fixing commit, with
   each one's state and the date it was read. **Where nothing relevant was
   found, say so**, with the date and what was searched, so a blank cannot be
   mistaken for nobody having looked. Opening an issue or asking for a
   backport publishes something: the maintainer's call, never done unasked.
3. **Fixed when.** The condition an upgrade can test at a version: a commit
   contained in the release, an API present, a bound moved.
4. **Workaround.** What we do meanwhile and where, what a user is told and
   where, and each alternative rejected, with why — this is the one place that
   reasoning lives, since it is struck with the workaround.
5. **Watch.** The test or check that fails when upstream moves. Prefer a test
   asserting the upstream behaviour itself, which fails on purpose at the
   version carrying the fix; name it.
6. **When it lands.** Every change the fix obliges, as pointers into our docs
   and code: the workaround to remove, tests to invert, the `KD<k>` to strike,
   decisions, invariants, manual lines and roadmap items to revisit.

**Each code site to revisit carries an `upstream: UF<k>` comment** — the
workaround, the watch test, any code whose shape the defect decided. A doc is
pointed at from the entry, never marked. `cd scripts && uv run upstream.py`
resolves every entry to its markers and back, holds each named `KD<k>` live,
and runs in `mise run check` through its tests. `UF<k>` is allocated on
discovery and never reused; the watermark marker carries the range.

**An entry is struck** in the change that takes the fix — heading, fields and
markers together, the watermark left as it is — and an entry whose fix has not
shipped is rewritten, never annotated: its Upstream field states what is true
on the date it names.
