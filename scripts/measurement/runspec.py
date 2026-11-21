"""What a figure runs and reads: one timed command (`RunSpec`), a reading borrowed from another
figure (`Shared`), and one read across two inputs (`Subtraction`)."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class RunSpec:
    """One timed command: a binary, an input, a command shape, a regime."""

    # "pgdt" | "dfcli" (`pgdt sql`, under the name a sitting's `raw.json` from
    # before the composition carries, so it still renders) | … | "none" (dd);
    # `Session.binary_path`
    binary: str
    input: str
    command: str
    regime: str  # "cold" | "cold-nvme" | "warm"
    label: str
    #: The container memory limit this one run is given, overriding the figure's
    #: own. `None` — every spec but the reserve figure's flagless legs — takes
    #: the figure's `memory`, or the recorded 512 MB.
    #:
    #: **A per-spec limit exists because one figure's axis *is* the limit.** A
    #: flagless run resolves its whole arrangement from the allocation it was
    #: given, so two legs differing only in `-m` are two different readings, not
    #: two reps of one; every other figure states its flags and wants one
    #: container for the table.
    memory: str | None = None
    #: Whether this leg runs an **instrument build** and must therefore produce
    #: a report (`INSTRUMENT_OUT_VAR`).
    #:
    #: **It is a declaration, not a consequence of the binary.** A missing
    #: report otherwise reads as `{}`, which is right for every default build
    #: and indistinguishable from three apparatus bugs: a leg built without the
    #: feature, a leg pointed at the default binary, and a report that never
    #: reached the file. Declared here, the absence is an error at the rep that
    #: produced it rather than an empty column an hour later. A leg the kernel
    #: OOM-killed is the legitimate absence and stays censored, as `KILL_TOLERANT`
    #: leaves it.
    #:
    #: Not part of `key`: it says what the harness must find, not what
    #: arrangement was measured, and two legs cannot differ by it alone — the
    #: build does not change the argv.
    instrument: bool = False
    #: Which of a split figure's `warm_groups` this run is swept in, where its
    #: input is staged in more than one: `None` is the first group staging it,
    #: so only a later group's run states one (`in_sweep`). **Part of `key`**:
    #: the same input in two sweeps is two readings, each paired only with its
    #: own sweep's (`nested-end-to-end`'s control).
    sweep: int | None = None
    #: The `--memory` this leg states where its shape leaves it to the sitting
    #: (`ALLOWANCE_SLOT`, `with_allowance`): `long-line-resident`'s, sized from
    #: the factors its instrument legs read in the same sitting. **Part of
    #: `key`**, appended where set: one shape under two allowances is two
    #: arrangements, so a moved factor re-takes every leg it sizes.
    allowance: int | None = None
    #: One `NAME=VALUE` this leg's container is given (`-e`), where the
    #: environment is its axis: `purge-delay`'s legs differ by
    #: `PURGE_DELAY_VAR` alone. **Part of `key`**, appended where set: one
    #: shape under two environments is two readings. A string rather than a
    #: mapping, so a record's `spec` reads back from `raw.json` equal to it
    #: (`Session.censored_bounds`).
    env: str | None = None

    def key(self, figure: str) -> str:
        """This run's identity, which is what a reading is filed under.

        **The limit is part of it whenever there is one**, since two flagless
        legs differ by nothing else and would otherwise share a key — the
        failure `_attribution_specs` names, where two legs differing only in the
        words a table prints silently become one reading. It is appended rather
        than always present so that a past sitting's `raw.json` still renders:
        every spec that states no limit keys exactly as it did before. A later
        sweep's run is appended the same way."""
        base = f"{figure}/{self.binary}/{self.input}/{self.command}/{self.regime}"
        if self.memory is not None:
            base += f"/m={self.memory}"
        if self.allowance is not None:
            base += f"/allowance={self.allowance}"
        if self.env is not None:
            base += f"/env={self.env}"
        return base if self.sweep is None else f"{base}/sweep={self.sweep}"


@dataclass(frozen=True)
class Shared:
    """A reading this figure takes from another figure rather than measuring.

    Declared here rather than left in a `session.borrow` call site, because a
    call site is not a graph: the harness could not compute what re-taking a
    figure drags, so `--figure` could not name it and every run function
    assembled its provenance paragraph by hand -- naming its own direct
    sources, which is short of the honest set whenever a source is itself
    borrowed from.

    `republished` are the runs whose *number* appears in this figure's table as
    well as in the source's. That is the relation the closure is computed over,
    because it is the one that puts two numbers in the doc for one measurement
    when only half the pair is re-taken. It is empty where the readings are
    consumed without being republished -- `cross-file-floor` differences
    `nested-end-to-end`'s reps into a per-row cost, which is a derived quantity
    and not that figure's number a second time -- and such an entry still
    orders the run and still pulls its source into a selection.

    *Rejected: making derivation an edge too.* It would close the one hazard
    this line leaves -- re-taking `nested-end-to-end` alone leaves
    `cross-file-floor`'s row 1 a difference over reps that no longer exist
    anywhere -- but at the cost of dragging a fifth figure into every
    `allocator` sitting and falsifying the four-figure closure
    `measurements.md` publishes. The hazard is closed by *naming* the reverse
    direction rather than taking it, which is the distinction `--figure`
    already draws: `derivation_gaps` says it before the first reading and in
    the emitted header, and `--check` reports it beside the partial sittings.
    Selection is directional and reads every share, so the forward direction --
    `--figure cross-file-floor` pulling its source in -- was never at risk.
    """

    source: str
    #: What this figure's table calls the borrowed reading, in its own terms.
    #: The generated note is only as legible as this phrase, since it is what a
    #: reader deciding whether to fold the table in actually reads.
    what: str
    republished: tuple[RunSpec, ...] = ()


@dataclass(frozen=True)
class Subtraction:
    """A reading this figure takes across two inputs of `source`'s reps.

    Declared because which inputs share a sweep is decided by it: a difference
    or a ratio between two inputs' readings, rep by rep or median against
    median, holds only where both were taken in one sweep, a session's own
    level shift being as large as what such a difference resolves
    (`measurements.md`, "Re-take a comparison table whole"). Where it is read
    is no matter -- a table row, a sentence or heading of the figure's
    section, or another figure consuming `source`'s reps -- and the reader
    declares it. `test_staging.py` holds each to one of `source`'s warm
    groups, and `_per_row_diffs` refuses a pair nothing declares. A reading
    normalised within its own sweep -- a shape against its own floor, a leg
    against its own one-worker row -- is set beside another sweep's, never
    subtracted from it, and is not one."""

    source: str
    #: The pair, unordered: which side is subtracted is the reader's.
    inputs: tuple[str, str]
    #: What reads it, in the section's own terms.
    what: str
