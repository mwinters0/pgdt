# P33.29 notes — the readers off a long field's parse reworked

**M276's rise is accepted; `ResolvedTerm::eval`'s is reworked, and the
rework's pair resolves only its expected fall.** Both were placed by 33.26
(`runs/count-pair-33.26-20261112/callgrind/reading.txt`, "The account":
`gap0` for M276, `gap3` for `eval`); this slice read the kept legs'
disassembly for M276's mechanism
(`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.26/legs-33.3.1/pgdt-count-pair-tree`
against `legs-33.3.2/pgdt-count-pair-parent`, the two ends of the
`d096761c..0e51cc2b` gap whose only diff touching the readers is M276's).

## M276, placed and accepted

- **The rise is the refusal's cold arm inlined into each reader**:
  `within_workbuf` tests a text's length against the input function's work
  buffer and only past it calls `parse_date_time_splits`, whose
  `parse_date_time` builds a `Vec<Field>`. Inlined, that call and the
  `Vec`'s drop loop sit in `date_days`, `time_of_day_micros` and
  `timestamp_micros_wide`, so every call saves six callee-saved registers and
  sets up a frame where the parent's `date_days` pushed no register;
  `time_of_day_micros`, inlined into its callers before, is now out
  of line. The test that runs on every value is one compare. Changed code;
  no layout.
- **Accepted** (D108): what would undo it is an inline or cold attribute on
  the refusal's arm, the rework resting on codegen D108 refuses, or a split
  test that allocates nothing beside `ParseDateTime`'s port, a second
  statement of I83's buffer for the readers to drift from.

## `ResolvedTerm::eval`, reworked

- **33.14, 33.14.1 and 33.15 each added an accessor asked of every row**
  (`runs`, `walks`, `jsonb`), each re-testing the term's unrepresentable
  read and its comparison to say whether a field is left escaped, and `eval`
  and `is_false` then dispatched on the three together, an `unreachable!`
  arm where all were `None`.
- **Now one decision a row** (`predicate/eval.rs`, `ResolvedTerm::reader`):
  a `Reader` — `Whole`, `Runs`, `Walk` or `Jsonb` — whose `reads_escaped`
  says whether the field is left as `COPY` wrote it, and `eval_escaped` the
  one dispatch both callers share. What each term reads is unchanged, held
  by `predicate`'s tests.

## Its count pair, a fall

**`0cc9f5c4` against the tree resolves one row, a fall on `pgdt sql -c`**,
whose `WHERE id IS NOT NULL` is the one standing row that evaluates a term;
the other nine rows, whose code did not change, do not move
(`runs/count-pair-33.29-20261116/log`). No rise resolves, so callgrind places
nothing (D108's falls unplaced).

- **The pair prices the rework against its parent, not against 33.26's
  account**: how much of the rise that account placed in `eval` the fall takes
  back, and whether `eval` still stands out of line, is unread, D108 asking
  no placing of a fall. Were it read and `eval` found out of line, the rework
  left is an inline attribute, which D108 refuses as resting on codegen.
