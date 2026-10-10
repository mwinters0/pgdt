# P35.1 notes — the limits and the skill

What the next slices inherit from the check. The mechanism is
`scripts/file_limits.py`, whose docstring says what it counts and how, with
the rule in [`../process.md`](../process.md), "Seams" and the procedure in
`.claude/skills/seams/SKILL.md`; `check.py` runs it as a meter beside
repoint's, in every `--affected` run too.

## What 35.2 inherits

- **The red files, and why each is red.** `stream.rs` and `predicate.rs` are
  past the hard code-line limit, `predicate.rs` past the total-length one as
  well; `decode.rs`, `gather.rs` and `io.rs` are past the hard function limit
  with their code lines only past the soft one — the grab-bag signal the
  function limit exists for. Past a soft limit only: `main.rs`, `pgtype.rs`,
  `preamble.rs`, `batch.rs`, `nested.rs`, `map.rs`, `cache.rs`,
  `statistics.rs`. `datetime_in.rs` is the one exemption. The counts are the
  script's to print, not this file's to keep.
- **Where the length is.** In the red files the inline test module is the
  larger half of the file more often than not, so the skill's first cut —
  tests to `<module>/tests.rs` — is what 35.2 does across every file past a
  limit. Three files hold more than one test module (`decode.rs` and
  `batch.rs` carry `tests`, `prior_shape` and `differential`); each becomes
  its own file beside the module, declared under `#[cfg(test)]`.
- **A test file is known by its declaration**, `#[cfg(test)] mod name;` in
  its parent, or by living under `tests/`; nothing else marks one, so a moved
  module keeps counting as tests without a naming convention the check would
  have to know.
- **What names a test by its file.** `scripts/oracle_register.py` locates
  three test functions by path in `pgtype.rs` and `predicate.rs`
  (`Evidence(...)` rows), and `STATUS.md` cites `predicate.rs`'s unit test by
  file; both move with the tests. `tests/layering.rs` reads only `src/*.rs`
  and cuts each at its first `#[cfg(test)] mod`, so a declaration line
  satisfies it unchanged.

## What 35.4 inherits

- **A `touch` rebuild is a floor, not a cost.** `runs/rebuild-baseline-20261009.txt`
  holds one sample each of the library, the binary, one test binary and
  `cargo check` rebuilt after touching one file: seconds, not minutes. It is
  a note, not a figure, and no slice re-takes it (the spec, "Why now").
- **Members are read from `[workspace].members` with globs**, so `crates/*`
  needs no edit here.

## Negative results

- **Counting every `fn` was wrong for Rust**: with methods and test functions
  included, fourteen of thirty library files were past forty; with tests
  excluded and the hard limit at a hundred, the five files past it are the
  ones a reader would have named. The first-pass numbers are the script's
  constants, pinned by `test_file_limits`; the maintainer tightens them.
