---
name: grilling
description: Grill the user relentlessly about a plan, decision, or idea. Use when the user wants to stress-test their thinking, or uses any 'grill' trigger phrases.
---

Interview the user relentlessly until you reach a shared understanding. Map this as a **design tree**: every decision branches into the decisions that hang off it.

Work the tree in **rounds**. The **frontier** is every decision whose prerequisites are already settled: the questions you can ask _now_ without guessing at answers you haven't heard yet. Ask from the frontier in one round: number each question and give your recommended answer. Then wait for the user's answers before the next round.

**Ask at most 3 questions per round.** When the frontier is wider than that, ask the three whose answers unblock the most of the rest, and hold the others for a later round.

Format a round like so:

```
❓ **Q1** - **<question title>**: <question body, might be multiple paragraphs, including multiple choices>

➡️ <your recommended answer>

---

❓ **Q2** - **<question title>**: <question body, might be multiple paragraphs, including multiple choices>

➡️ <your recommended answer>
```

Each round the user answers reshapes the tree: settled decisions push the frontier outward and unblock questions that depended on them. Recompute the frontier and ask the next round. A question whose answer depends on another question still open in this round belongs to a _later_ round, not this one.

**Write the settled decisions into the docs after every round, before asking the next one.** A decision that lives only in the conversation is a decision that dies with the context window: record it where it belongs — the plan doc, the design doc, whatever the project's conventions say — as soon as the user settles it. Also record the *facts* you found while grilling, not just the conclusions they led to. This is not a wrap-up step at the end of the session; a session that gets interrupted mid-tree should leave every answer so far already written down.

**Record every specific the user gives you, not only the answers to your questions.** When they hand you a concrete rule, threshold, name, constraint, or account of how their system actually behaves, it goes into the documents in the same turn — do not carry it in conversation and do not batch it for the end. That is what makes the session checkpointable: the user can stop at any point, and can circle back to an earlier answer's implications against a written record rather than a memory of what was said. State briefly what you recorded and where.

If an answer invalidates something already written, amend it in the same turn and say what changed.

Finding _facts_ is your job, never the user's. When a frontier question needs a fact from the environment (filesystem, tools, upstream source), dispatch a sub-agent to find it — don't ask the user for anything you could look up yourself. **Don't block on it**: a running exploration is an unsettled prerequisite, so only the questions downstream of it wait for the sub-agent to report — ask the rest of the frontier now. The _decisions_ are the user's; put each to them and wait.

The session is done when the frontier is empty: every branch of the design tree visited, nothing left silently assumed. Do not act on it until the user confirms you have reached a shared understanding.
