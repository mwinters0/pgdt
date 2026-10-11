# P35.22 notes — `pgdt`, a module a subcommand

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt" tree, its `moves` block applied whole and then struck from that doc.
Every body moved whole. `module_graph.py` finds no cycle in `pgdt`, so its
`OWED` entry is gone and the table is empty: the tree owes no cycle. Doc
links against the edges rose from 192 to 194: `info/text.rs`'s to `info.rs`'s
`report` and `completion_line` and `term.rs`'s to `expr` are new, and
`expr.rs`'s to the crate root went with the cycle.

## Where the cut fell

- **`Command`'s variants wrap each subcommand's `#[derive(Args)]` struct**, and
  a variant's doc comment is still its subcommand's help: every
  `tests/help_text.rs` snapshot is unchanged. Each `run` takes its struct and
  destructures it into the names the arm bound, so its body is the arm's,
  dedented; its comments were not rewrapped.
- **Items no move line named**: `about_the_source` went to `source.rs` with
  `naming_the_source`, which reads it; `init_status_output` stayed at the root
  with `main` and `commands`, the dispatch.
- **Widened to compile**: what a sibling reads is `pub(crate)`; what `info`'s
  and `query/filter`'s children read of each other is `pub(super)`, and
  `ParseArgs::identity` is `pub(crate)` for the root's test reading it.
  `query/filter.rs` re-exports `parse_where` for `query.rs`, as
  `predicate.rs` re-exports for its readers; `expr.rs` names the term grammar
  by `super::term::` paths, where it named `crate::` ones.
- **A doc link to a private item in another module is a code span**
  (`text::compression_line` from `json.rs`, `dequote` from `labels.rs`):
  rustdoc does not resolve it. `cargo doc --document-private-items` warns on
  nothing in `pgdt`.

## Tests

- **The suite is the same 48 unit tests.** The root's `tests.rs` keeps the
  three that read `Cli` — the two-tunables rule, `sql`'s exemption, the bare
  `--strict-identity` — and the rest split beside `version`, `args`, `source`,
  `parallel`, `info/labels`, `query/filter` and `query/filter/term`.
  `expr.rs`'s and `info/statistics.rs`'s inline modules moved whole with their
  files, neither file being cut.

## What later cuts inherit

- **`concepts.md` has no "Not in one place" left**: this cut struck the last
  five, all `pgdt`'s.
- **A `deficiency:` marker moves with its item**: `KD29` and `KD50` now name
  `parallel.rs`, `KD52` `query/filter/term.rs`, `KD57` `query.rs`.
  `test_measure.py` reads the query's printed phrases from `query.rs`.
