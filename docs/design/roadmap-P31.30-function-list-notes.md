# P31.30 — The emitter register's function list follows the readers: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"The emitter register"; the drift it answers is
[`../status/history/2026-10-04.md`](../status/history/2026-10-04.md), "The
emitter register's function list follows the readers". No product code
changed.

## What exists

- **`READS`**, in `scripts/emitter_register.py`: every keyword the five reader
  modules recognise, as a `Read` naming the listed functions writing the
  statement it dispatches on, or a `Clause` naming the `Read` it lies inside.
  `reader_keywords` reads the keywords out of the Rust source: each constant a
  keyword-matching call takes (`strip_kw`, `eat_keyword`, …) in any case, and
  each constant shaped as one anywhere in the non-test code. `reads_problems`
  fails a keyword with no row, a row no reader holds, a `Read` naming an
  unlisted function, and a `Clause` outside a `Read` of its own reader; the
  join runs it, so `mise run check` does.
- **The query rule is per append** (`query_appends`): an append is a query's
  text only where every read of its buffer after it, before a reset, is an
  execute call. A diagnostic's read and a byte rewritten in place are no read;
  a buffer `appendShellString` quotes into is a command line
  (`command_buffers`); a constant inside `strlen`/`strcmp`/`strncmp` is not
  written. `archprintf` and `archputs` are append calls.
- **`FUNCTIONS` holds `Emitter`s**, each with an optional `first` and `last`
  major: `dumpTableAttach` from 14, `StartRestoreBlobs`/`EndRestoreBlobs` to
  15 and `StartRestoreLOs`/`EndRestoreLOs` from 16, which is the same
  function renamed. Added: `dumpTableAttach`, `dumpTableData`,
  `dumpTableData_copy`, `dumpTableData_insert`, `dumpConstraint`,
  `dumpShellType`, `dumpUndefinedType`, `dumpExtension`, `dumpCollation`,
  `dumpSearchPath`, `RestoreArchive`, `_doSetFixedOutputState`,
  `_doSetSessionAuth`, `_selectOutputSchema`, `_selectTablespace`,
  `_selectTableAccessMethod`, the four large-object functions,
  `buildACLCommands`, `buildDefaultACLCommands`, and `pg_dumpall`'s `main`
  and `dumpRoleMembership`.
- **Fixtures**: the `emitters` schema gained a `UNIQUE` constraint with
  `INCLUDE`, deferral and the table's replica identity, a split-locale
  collation, a table with no columns and one with a `GENERATED ALWAYS`
  identity (both reached by `dumpall-data-only`'s `--inserts`), and a grant
  `WITH GRANT OPTION` passed on by its grantee; sidecars at 15 (`NULLS NOT
  DISTINCT`), 16 (an ICU collation's `rules`), 17 (a `builtin` collation) and
  18 (`WITHOUT OVERLAPS`). Its cluster script makes three roles and a
  membership `WITH ADMIN OPTION`, and takes version sidecars as a schema does
  (`ClusterScript.files`): 16's grants one `WITH INHERIT FALSE, SET FALSE`.
  `drop_fixture_db` drops the roles. A setting variant,
  `emitters/standard-conforming-strings-off`, writes `SET
  escape_string_warning = off;` and doubles a comment's backslash.
- **I87–I89** exempt the six literals no producer the generator runs writes:
  `SET ROLE ` and `COMMIT;\nBEGIN;\n` (restore-only options), `_selectOutputSchema`'s
  two (a search path is always set), and 13–14's
  `binary_upgrade_set_record_init_privs` for a default ACL.

## Findings

- **No defect.** Every new fixture tiles, resolves and passes the suite; the
  zero-column table's `COPY` reads as its two rows, and the off-standard-strings
  dump lexes. Two tests were re-pinned for the new fixtures alone: the
  persisted-index digest (`CACHE_FORMAT_VERSION` unchanged, the same dump
  saving the same bytes) and the strict listing's block count at 16.

## Negative results, and what the check does not see

- **The check is per keyword, not per use.** A reader that starts
  dispatching on a keyword already in `READS` — a `Clause` word taken as a new
  statement form — passes unchanged; only a new keyword constant is forced
  into a row. A `Clause` is the hand classification that permits this.
- **A reader matching by a character is not seen**: the map takes any line
  opening with `\` as a meta-command (`\restrict`), which no keyword names.
  Its writers, `RestoreArchive` and `pg_dumpall`'s `main`, are listed for
  their other statements.
- **The list holds more than `READS` names**, the spec binding one direction:
  `setup_connection` (I4's pins, no row) and `pg_dumpall`'s tablespace and
  database emitters, whose statements no keyword reads but the map must
  still tile (`KD61`'s `CREATE TABLESPACE`).
