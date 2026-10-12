# Seam findings

The seam register: what a read of the tree for the `seams` skill's
"Antipatterns" ([`../../.claude/skills/seams/SKILL.md`](../../.claude/skills/seams/SKILL.md))
found, ruled a defect, and left for a later pass, so a phase can close on a
tree that is not yet ideal and the next session in a module meets what is
owed there. It sits beside [`deficiencies.md`](deficiencies.md), and the line
between them is who bears the cost: a `KD<k>` costs a user or an embedder
something, an `S<k>` only the next person reading or changing the code. A
finding with an outside cost is a `KD<k>` whatever its shape.

Some findings are not entries. **One concept, several files** is a **Not in
one place** line under its concept in
[`../design/concepts.md`](../design/concepts.md), and **Sibling cycles** an
`OWED` or `ACCEPTED` entry in `scripts/module_graph.py`. An antipattern kept
with its reason is a comment beside the code saying why, as a property is
beside its mechanism. And filing is a ruling: unattended, a read lists what
it found under STATUS's "Decisions worth another look", and the answer files
it.

- **An entry is the symptom, never the remedy**: what the code does that the
  antipattern names, so a redesign that dissolves it another way still
  closes it. The cut a reviewer drew stays in the review. One line,
  `- **S<k>** — *<antipattern>.* <symptom>`, wrapped to at most four, the
  antipattern spelled as the skill spells it.
- **The detail is at the markers**: a `seam: S<k>` comment at each site the
  symptom spans, saying what of it sits there. A session editing the module
  meets it there, the cheapest moment to remove it.
- **Struck by the change that removes the symptom**, by whatever means: index
  line and every marker together. A partial removal rewrites the entry to what
  is still true.
- **The read is the authority, not the register.** A seams round reads the
  tree without opening this file, then reconciles: what it found again stays,
  what it did not is struck, what is new is ruled and filed. An antipattern
  struck from the skill strikes or re-files every entry naming it.
- **No count is a criterion.** A design is not weighed by the entries it adds
  or removes; one that adds an entry says so in its notes doc, and why.

`cd scripts && uv run seam_register.py` resolves each entry to its markers and
back, and its antipattern against the skill. Numbers up to the watermark are
allocated, and none is reused.
<!-- seam-watermark: S0 -->
