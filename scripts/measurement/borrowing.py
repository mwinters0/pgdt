"""The borrow graph: what a figure takes from another, what a re-take drags with it, and the
provenance paragraph a table carries."""

from __future__ import annotations

from typing import Sequence
from typing import TYPE_CHECKING

from measurement.figures import register

if TYPE_CHECKING:
    from measurement.figures.declaration import Figure
    from measurement.runspec import Shared
    from measurement.timing import Session


def sharing_edges() -> dict[str, set[str]]:
    """The republication graph, undirected.

    Undirected because the defect is symmetric: `allocator` borrowing
    `scan-throughput-warm`'s `COPY` reading puts the same number in two
    tables, and re-taking *either* of them alone leaves the doc carrying two
    numbers for one measurement. Direction only says which figure measures
    it."""
    edges: dict[str, set[str]] = {}
    for fig in register.EVERY_FIGURE:
        for shared in fig.shares:
            if not shared.republished:
                continue
            edges.setdefault(fig.id, set()).add(shared.source)
            edges.setdefault(shared.source, set()).add(fig.id)
    return edges


def sharing_closure(fid: str) -> list[str]:
    """Every other figure that publishes a reading this one would move.

    Transitive, which is the whole point of declaring the graph: the note a
    run function used to write by hand named its *direct* sources, and a
    source that is itself borrowed by a third table drags that table too.
    Returned in register order, so the closure reads as a run order."""
    edges = sharing_edges()
    seen, queue = {fid}, [fid]
    while queue:
        for nxt in edges.get(queue.pop(), ()):
            if nxt not in seen:
                seen.add(nxt)
                queue.append(nxt)
    return [f.id for f in register.EVERY_FIGURE if f.id in seen and f.id != fid]


def share_readings(session: Session, figure: str) -> str:
    """Borrow every reading this figure declares, and say what happened.

    The provenance paragraph is generated from the declaration rather than
    written at the call site. That is what makes it name the closure: a call
    site knows the source it just asked for, and nothing else."""
    fig = register.EVERY_BY_ID[figure]
    taken: list[Shared] = []
    missing: list[Shared] = []
    for shared in fig.shares:
        if not shared.republished:
            continue
        got = session.borrow(figure, shared)
        (taken if len(got) == len(shared.republished) else missing).append(shared)
    lines = []
    if taken:
        lines.append(
            "\nShared, not measured again — the same binary, command and input:\n"
            + "".join(f"- {s.what}, from `{s.source}`.\n" for s in taken)
        )
    if missing:
        closure = ", ".join(f"`{f}`" for f in sharing_closure(figure))
        lines.append(
            "\n**Partial sweep**: this session did not emit every figure this table shares a "
            "reading with, so these were measured here instead:\n"
            + "".join(f"- {s.what}, which is `{s.source}`'s.\n" for s in missing)
            + "\nThose are the same measurements and must agree, so the doc now carries two "
            f"numbers for one reading. The whole set that shares readings with this table is "
            f"{closure} — re-take it together before folding any of them in.\n"
        )
    return "".join(lines)


def publication_refusals(figures: Sequence[Figure]) -> list[str]:
    """Why this sitting's tables could not enter the doc, one line per figure.

    A sitting short of the whole sweep publishes outside the session stamp, and
    a figure may only do that when it stands in no edge of the borrow graph in
    either direction — a republished share puts one measurement's number in two
    tables, a derivation makes one table's row a difference over another's reps,
    and either one crossing two sittings is the doc asserting a differencing it
    no longer has. It is the same condition `--check` fails a sitting marker on,
    asked before the measurement is spent rather than after.

    Refusing rather than warning is deliberate: there is no partial version of
    it, and the harness already refuses this way where a selection cannot be
    honestly taken (`resolve_selection` under `--alone`).

    Asked only of a run whose tables could enter the doc at all. `--alone` is
    not exempted from it -- such a run is unpublishable, so the refusal has no
    publication to refuse and stops firing by construction."""
    return [
        f"{fig.id} is read beside {', '.join(entangled)}, so only a sweep can move its table"
        for fig in figures
        if (entangled := entangled_with(fig.id))
    ]


def closure_gaps(figures: Sequence[Figure]) -> list[str]:
    """What a selection shares a reading with and does not take, one line each.

    Said before the first reading rather than discovered at fold-in time. A
    figure here is not necessarily *wrong* — the doc publishes a partial
    sitting so long as it says so — but it is a table whose absolutes may not
    be set beside the ones it shares a reading with, and this is the last
    moment adding them to the selection is free."""
    selected = {f.id for f in figures}
    out = []
    for fig in figures:
        absent = [f for f in sharing_closure(fig.id) if f not in selected]
        if absent:
            out.append(f"{fig.id} — shares a reading with {', '.join(absent)}")
    return out


def derivation_edges() -> list[tuple[str, str, str]]:
    """Every non-republishing borrow, as `(consumer, source, what)`.

    The direction the closure deliberately does not carry: a derived quantity
    is not the source's number a second time, so the two tables are free to be
    published from different sittings — right up until the source is re-taken,
    at which point the consumer's row is a difference over reps the doc no
    longer holds anywhere.

    Naming is the whole remedy. Widening the closure would drag the consumer
    into every sitting that touches its source (`Shared`, above), where what
    the hazard actually needs is that nobody folds the source in without being
    told which table is now derived from readings that are gone. In register
    order, so a report of it reads as a run order."""
    return [
        (fig.id, shared.source, shared.what)
        for fig in register.EVERY_FIGURE
        for shared in fig.shares
        if not shared.republished
    ]


def derived_consumers(fid: str) -> list[tuple[str, str]]:
    """Figures whose own table is computed over `fid`'s reps, and what of."""
    return [(c, what) for c, source, what in derivation_edges() if source == fid]


def derivation_gaps(figures: Sequence[Figure]) -> list[str]:
    """Consumers of a selected figure's reps that this sitting does not take.

    The counterpart of `closure_gaps` for the derived direction, and said at the
    same moment for the same reason: `--figure nested-end-to-end` is a perfectly
    good sitting, but folding it in strands `cross-file-floor`'s first row, and
    that is knowable before the first reading rather than at fold-in time."""
    selected = {f.id for f in figures}
    return [
        f"{fig.id} — {consumer} derives {what} from its reps"
        for fig in figures
        for consumer, what in derived_consumers(fig.id)
        if consumer not in selected
    ]


def entangled_with(fid: str) -> list[str]:
    """Every figure whose readings this one's table cannot be separated from.

    Both edges of the borrow graph in both directions: a republished share
    puts one measurement's number in two tables, and a derivation makes one
    table's row a difference over another's reps. A figure with neither may be
    published from a sitting of its own, because what one sitting buys is the
    ability to *difference* these tables against each other, and a figure that
    stands in no edge endangers none of it."""
    fig = register.EVERY_BY_ID.get(fid)
    consumed = [s.source for s in fig.shares if not s.republished] if fig else []
    return sorted({*sharing_closure(fid), *(c for c, _ in derived_consumers(fid)), *consumed})
