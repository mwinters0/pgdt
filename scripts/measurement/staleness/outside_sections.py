"""Sections the register does not hold (`Outside`), and what `--figure` may select."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable
from typing import TYPE_CHECKING

from measurement.figures import declaration
from measurement.figures import register

if TYPE_CHECKING:
    from measurement.figures.declaration import Figure


@dataclass(frozen=True)
class Outside:
    """A `measurements.md` section the figure register does not hold.

    Every *figure* announces itself twice — a marker and an apparatus line —
    and a section outside the register used to announce itself with nothing,
    while the doc's session stamp asserted "every figure below was taken by
    `scripts/measure.py`" over it. So a section outside the register carries an
    `<!-- outside-register: <id> -->` marker of its own, and `--check`
    reconciles those markers against this register both ways and holds each
    declared section to carrying no figure marker. A section cannot be both.

    **What makes staying outside safe rather than merely silent is `depends`.**
    Being outside the register says the harness cannot re-take the readings; it
    was also saying, by omission, that nothing would ever be told when they went
    wrong — `--stale` read `ALL_BY_ID` and could not name a declared section
    however far its inputs moved. koji went four campaigns that way, and the
    claim `peak-rss` was registered to rescue had sat in exactly that blind spot
    for a whole slice. So a declared section carries the same invalidation edge
    a figure does, and `--stale` reports it beside them. What it cannot carry is
    the other half of a figure's contract — the harness will not re-take it — so
    a red here is discharged by a run somebody makes by hand, or by an
    acknowledgement, exactly as a figure's is.

    **The commit the readings were taken at lives in the doc's marker, not
    here.** It is the argument the figure sittings already settled
    (`measurements.md`, "A figure may be published outside the sweep"), one
    mechanism along: `session-drift` declares the harness's source, so recording
    a koji run's commit in this file would mark a figure stale for recording
    where another reading came from, every time koji is re-run. The marker is
    already the doc's convention for provenance, and `outside_sittings` reads it
    back out."""

    id: str
    #: The doc heading it sits under, for `--list` and for the check's report.
    section: str
    #: Why the harness does not own it. Printed by `--list`.
    why: str
    #: Repo-relative paths whose change invalidates the readings this section
    #: publishes — a figure's `depends`, for a section that is not one. Empty
    #: is right only where the section publishes no number the design quotes,
    #: which is the very thing that puts `benches` outside; `--check` holds a
    #: section that declares paths to declaring the commit it was taken at, and
    #: one that declares none to declaring no commit.
    depends: tuple[str, ...] = ()

#: Sections measurements.md carries that this harness deliberately does not
#: own — the other half of the boundary `--check` reconciles.
NOT_OURS = {
    o.id: o
    for o in [
        Outside(
            "koji",
            "koji full scan",
            "784 GB on the HDD, ~54 minutes, a different medium, and a byte-for-byte regression "
            "check rather than a throughput figure. The harness owns the *invocation* — "
            "`--koji-recipe`, which prints it — and never runs it.",
            # What a koji run publishes is what a scan *concludes*: the block
            # list, the per-block offsets, the row and byte totals, and the
            # `info --detail` report they are compared as text against. So the
            # read path, the scanner, the map and the cache, which decide the
            # first four, and the preamble prepass, the type resolution and the
            # CLI, which decide the fifth — the last of those is not
            # hypothetical, the 2026-09-05 comparison differing from the
            # 2026-08-27 one by exactly one line of user-defined-type detail the
            # report had gained in between. `resolve.rs` and the register's
            # files (`PGTYPE`) are declared here rather than by every figure
            # that resolves a type because this is the only section in the
            # document whose readings a change to them can falsify: every
            # figure that resolves a type publishes a duration, and a duration
            # is not what the report's text identity is.
            depends=(
                *declaration.READ,
                *declaration.SCAN,
                *declaration.MAP,
                *declaration.CACHE,
                *declaration.PREAMBLE,
                "crates/pgdt-codec/src/resolve.rs",
                *declaration.PGTYPE,
                *declaration.QUERY_CLI,
            ),
        ),
        Outside(
            "benches",
            "benches/decoders.rs per-type pairs, benches/whole_file.rs",
            "Tripwires, not figures: they quote no number in the doc, so there is no table to "
            "emit. That is a decision, not an oversight -- `cargo bench -p pgdt-codec` and "
            "`-p pgdt-query` run them.",
            # No edge, and the empty tuple is the claim rather than an omission:
            # this section publishes no reading, so there is nothing a diff
            # could invalidate. It is the same sentence that puts it outside the
            # register, said in the field that would otherwise have to be
            # guessed at.
            depends=(),
        ),
    ]
}


def resolve_selection(ids: Iterable[str], alone: bool = False) -> list[Figure]:
    """Selected figures plus whatever they borrow from, in run order.

    `alone` takes exactly what was named, which is how a **diagnostic** sitting
    is asked for: the borrowed rows are then measured by the figure itself, its
    table carries the harness's partial-sweep note, and the whole run is marked
    unpublishable (`Config.unpublishable_reason`). It is refused where a figure
    *consumes* another's readings without republishing them, since there is no
    local measurement for that figure to fall back on -- the reading it wants
    is the other figure's reps, not a run it could take."""
    wanted: set[str] = set()

    def add(fid: str) -> None:
        if fid in wanted:
            return
        if fid not in register.SELECTABLE_BY_ID:
            raise SystemExit(f"unknown figure {fid!r}; `--list` names them all")
        wanted.add(fid)
        if alone:
            return
        for dep in register.SELECTABLE_BY_ID[fid].requires:
            add(dep)

    for fid in ids:
        add(fid)
    if alone:
        for fid in sorted(wanted):
            for shared in register.SELECTABLE_BY_ID[fid].shares:
                if not shared.republished and shared.source not in wanted:
                    raise SystemExit(
                        f"--alone refuses {fid}: it reads {shared.source}'s own reps "
                        f"({shared.what}) and cannot measure them for itself"
                    )
    return [f for f in register.SELECTABLE if f.id in wanted]
