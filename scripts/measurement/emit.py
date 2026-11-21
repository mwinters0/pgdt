"""A sitting, emitted: the session stamp, each figure's section, and `--list`."""

from __future__ import annotations

import dataclasses
import json
import sys
import tempfile
import time
from datetime import date
from pathlib import Path
from typing import Mapping, Sequence
from typing import TYPE_CHECKING

import contained

from measurement import borrowing
from measurement import config
from measurement import consumers
from measurement import host
from measurement import inputs
from measurement import machine_state
from measurement import preflight
from measurement import replay
from measurement import staging
from measurement import timing
from measurement.builds import leg_binaries
from measurement.figures import register
from measurement.figures import runners
from measurement.staleness import doc_markers
from measurement.staleness import outside_sections
from measurement.staleness import sitting_checks
from measurement.staleness import stale

if TYPE_CHECKING:
    from measurement.config import Config
    from measurement.figures.declaration import Figure


def git_head() -> tuple[str, bool]:
    """The commit the figures were taken against, and whether the tree differs
    from it *in a way that could move a number*.

    An uncommitted doc or a new harness file cannot change a reading, and
    calling the run unpublishable for one would make the check noise. What
    counts is an uncommitted change under some figure's declared paths --
    exactly the predicate `--stale` uses."""
    head = host.run(["git", "rev-parse", "--short", "HEAD"], cwd=config.REPO, capture=True).strip()
    status = host.run(["git", "status", "--porcelain"], cwd=config.REPO, capture=True)
    changed = [line[3:].strip() for line in status.splitlines() if line.strip()]
    return head, bool(stale.figures_touched(changed))


def session_stamp(
    head: str,
    dirty: bool,
    allocator: str | None = None,
    outside: Sequence[tuple[str, str]] = (),
    glibc: str | None = None,
) -> str:
    """The line the doc carries, and the line `stamped_commit` reads back.

    The allocator is part of it because a figure here is a **CLI** figure,
    taken under whatever `pgdt` links against -- see `measurements.md`, "Which
    allocator a figure was taken under". It is read out of the binary rather
    than assumed, so the day the CLI's default changes the stamp changes with
    it; `None` where there is no binary to ask, which is `--dry-run` and the
    unit tests. **The glibc is the register image's**, asked of the image
    (`glibc_of`) for the same reason; a figure whose program ran anywhere else
    names its own in its marker (`marker_glibc`).

    **It is scoped to the register, not to everything printed below it.** The
    doc also carries sections this harness does not own, and the stamp's claim
    was false over them for as long as it said "every figure below"; the scope
    is what a section's `<!-- outside-register: <id> -->` marker declares it
    out of, and `--check` holds the two in step.

    **`outside` is the figures the doc carries from a sitting of their own**,
    and the sentence accounting for them is generated here rather than written
    by hand beside it — see `sitting_accounting`."""
    return (
        f"**Session stamp.** Every figure below — every section carrying a "
        f"`<!-- figure: … -->` marker, and no other — was taken by `scripts/measure.py` on "
        f"{date.today().isoformat()}, against commit `{head}`"
        f"{taken_against(dirty, allocator, glibc)}. "
        + sitting_checks.sitting_accounting(outside)
    )


def taken_against(dirty: bool, allocator: str | None, glibc: str | None = None) -> str:
    """What qualifies a commit in a stamp: the tree's state, the allocator and
    the register image's glibc.

    Shared with the header a *partial* sitting writes instead of a stamp, so
    that the two say the same thing about the same run."""
    suffix = " (with uncommitted changes under a measured path)" if dirty else ""
    under = [f"the `{allocator}` allocator"] if allocator else []
    under += [f"glibc {glibc}"] if glibc else []
    return suffix + (", under " + " and ".join(under) if under else "")


def partial_lead(
    taken: int,
    head: str,
    dirty: bool,
    allocator: str | None,
    unpublishable: str | None,
    glibc: str | None = None,
) -> str:
    """The header a sitting short of the sweep writes instead of a stamp.

    **The fold-in instruction is conditional on the run being publishable at
    all.** A `--alone` sitting emits its tables under the `NOT PUBLISHABLE`
    banner, and a header that told the reader to fold each table in would be
    directing them to do the thing the banner a few lines down refuses."""
    fold_in = (
        "Its tables do not enter the document at all — see the banner below."
        if unpublishable
        else "Leave its session stamp alone, and fold each table in with the `taken at` "
        "commit and glibc its own marker carries; `--check` refuses that marker on a figure that "
        "shares a reading or stands in a derivation, which is every figure a sweep is the "
        "only way to move."
    )
    return (
        f"**A sitting of its own, not a sweep.** This run took {taken} of the "
        f"{len(register.FIGURES)} figures a sweep takes, with `scripts/measure.py` on "
        f"{date.today().isoformat()}, against commit `{head}`"
        f"{taken_against(dirty, allocator, glibc)} — so this run does not stamp the document. "
        + fold_in
    )


def censored_note(kills: Mapping[str, int]) -> str:
    """The paragraph that goes above a table one of whose legs was OOM-killed.

    It sits in the figure's own section rather than only in the run's header,
    because a section is what gets pasted: a banner at the top of `tables.md`
    is not carried by the one table somebody copies out of it.
    """
    legs = ", ".join(f"`{key.partition('/')[2]}` ({n} rep(s))" for key, n in sorted(kills.items()))
    return (
        "> **A leg of this table was OOM-killed, so it must not be published.** "
        f"{legs}. Those cells are censored — the reading is a bound on a peak the process "
        "never reached, so it enters neither the fit nor the headroom column — and the rest "
        "of the table was measured beside a leg that did not complete.\n\n"
    )


def recorded_input_sizes(stager: staging.Stager, figures: Sequence[Figure]) -> dict[str, int]:
    """Every selected figure's inputs, by name, at the size they were on disk.

    Recorded so `--render` can rebuild a table's byte counts and rates without
    the inputs still existing: warm staging is evicted at the end of a sitting,
    and the SSD cache is explicitly safe to delete.

    **A derived input's source is sized too**, though no figure stages it: a
    renderer reading a compressed leg's plaintext length reads it there
    (`gzip_limit_ratio_problem`).
    """
    sizes: dict[str, int] = {}
    for fig in figures:
        for name in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs):
            while name is not None:
                path = stager.cfg.cache_dir / inputs.input_file(name)
                if path.exists():
                    sizes[name] = path.stat().st_size
                name = inputs.INPUTS[name].derives_from
    return sizes


def render(cfg: Config, run_dir: Path) -> int:
    """Rebuild one sitting's `tables.md` from its `raw.json`, measuring nothing.

    The header is the sitting's own, verbatim: it describes when and at what
    commit the readings were taken, which re-rendering does not change. Only
    the per-figure sections are rebuilt, because those are what a renderer
    change moves.
    """
    raw_path = run_dir / "raw.json"
    if not raw_path.exists():
        print(f"no raw.json in {run_dir}", file=sys.stderr)
        return 2
    raw = json.loads(raw_path.read_text())
    missing = [k for k in ("header", "input_sizes", "readings") if k not in raw]
    if missing:
        print(
            f"{raw_path} predates --render and is missing {', '.join(missing)}; "
            "re-take the figure to render it.",
            file=sys.stderr,
        )
        return 2

    # `SELECTABLE_BY_ID`, not `FIGURES_BY_ID`: a sitting is re-renderable
    # exactly when `--figure` could have taken it, and an **untaken**
    # instrument is the case that needs re-rendering most — it is taken
    # diagnostically, repeatedly, while its renderer is still being written.
    by_id = register.SELECTABLE_BY_ID
    unknown = [fid for fid in raw["figures"] if fid not in by_id]
    if unknown:
        print(f"unknown figure(s) in {raw_path}: {', '.join(unknown)}", file=sys.stderr)
        return 2
    # A figure the sitting failed took no readings, so replaying it would die
    # on a missing key rather than rebuild a table -- and the run's own record
    # already says which those were.
    failed = {fid for fid, _ in raw.get("failures", [])}
    figures = [by_id[fid] for fid in raw["figures"] if fid not in failed]
    head, whole_sweep = raw["commit"], raw.get("whole_sweep", False)
    # An older sitting recorded the *selection* here, so re-deriving it is what
    # keeps a re-render from restoring a stamp the run was not entitled to.
    whole_sweep = stamps_the_document(whole_sweep, sorted(failed))

    with tempfile.TemporaryDirectory(prefix="pgdt-render-") as tmp:
        session = replay.ReplaySession(cfg, raw, Path(tmp), lambda msg: None)
        parts: list[str] = []
        sections_seen: set[str] = set()
        for fig in figures:
            session.figure_id = fig.id
            body = runners.RUNS[fig.id](session)
            apparatus = machine_state.apparatus_note([r for r in session.records if r.get("figure") == fig.id])
            quoting = consumers.consumers(fig)
            note = (
                "\n**The fold-in must also re-read**, because these repeat this figure's "
                "numbers or the claim it licenses: "
                + ", ".join(f"`{q}`" for q in quoting)
                + ".\n"
                if quoting
                else ""
            )
            heading = "" if fig.section in sections_seen else f"## {fig.section}\n\n"
            sections_seen.add(fig.section)
            label = f"**{fig.table_label}**\n\n" if fig.table_label else ""
            marker = doc_markers.figure_marker(
                fig.id,
                None if whole_sweep else head,
                glibc=doc_markers.marker_glibc(
                    whole_sweep, raw.get("glibc"), raw.get("figure_glibc", {}).get(fig.id)
                ),
            )
            kills = session.figure_kills(fig.id)
            bar = censored_note(kills) if kills else ""
            parts.append(f"{heading}{marker}\n\n{label}{bar}{body}\n{apparatus}{note}")

    out = run_dir / "tables.md"
    out.write_text("\n".join(raw["header"]) + "\n" + "\n".join(parts))
    print(f"re-rendered {out} from {raw_path} — {len(figures)} figure(s), nothing measured")
    return 0


def stamps_the_document(selected_sweep: bool, failures: Sequence[object]) -> bool:
    """Whether a sitting may re-stamp `measurements.md` with a session stamp.

    **Selecting the whole sweep is not taking it, and the stamp speaks for
    tables this sitting did not produce.** A run that selected every figure and
    lost one leaves those tables in the document from whatever sitting put them
    there, so a stamp saying every figure below came from this one is false of
    exactly the tables nobody re-took. That is not a smaller version of a
    partial sitting -- it is the same thing, and it declares itself the same
    way: each table it *did* take carries the commit inside its own marker.

    It is a function so that the predicate is testable and named. Computed
    inline it was `{f.id for f in FIGURES} <= {f.id for f in figures}` evaluated
    before the first reading, which cannot see a failure at all."""
    return selected_sweep and not failures


def emit(cfg: Config, figures: Sequence[Figure]) -> int:
    out_root = cfg.out_dir / f"measure-{time.strftime('%Y%m%dT%H%M%S')}"
    out_root.mkdir(parents=True, exist_ok=True)
    log_path = out_root / "log.txt"
    log_file = log_path.open("w")

    def log(msg: str) -> None:
        print(msg, flush=True)
        log_file.write(msg + "\n")
        log_file.flush()

    head, dirty = git_head()
    allocator = None if cfg.dry_run else leg_binaries.binary_allocator(cfg.bin_pgdt)
    # The register image's glibc, which the stamp names: asked before anything
    # is staged, so an image that is no glibc costs a second rather than a
    # sitting. A figure run anywhere else asks its own place (`Session.ran_in`).
    glibc = None if cfg.dry_run else leg_binaries.glibc_of(cfg, cfg.image)
    # A sitting short of the whole sweep does not re-stamp the document, so
    # each table it emits carries the commit it was taken at inside its own
    # marker and every reader of the stamp argues from that
    # (`measurements.md`, "A figure may be published outside the sweep").
    #
    # **Selecting the whole sweep is not taking it.** This is the *selection*;
    # a figure that fails takes no table, so a run that selected all of them
    # and lost one leaves the document's other tables from an older sitting
    # while a stamp would claim they came from this one. The predicate that
    # decides the stamp is computed after the loop, once `failures` is known.
    selected_sweep = {f.id for f in register.FIGURES} <= {f.id for f in figures}
    log(
        f"measure.py — {len(figures)} figure(s), commit {head}{' (dirty)' if dirty else ''}"
        + (f", allocator {allocator}" if allocator else "")
        + (f", glibc {glibc}" if glibc else "")
        + ("" if selected_sweep else ", a sitting of its own (each table declares this commit)")
    )
    log(f"output: {out_root}")
    if cfg.arms != (machine_state.Arm(),):
        log(
            "arms: " + ", ".join(a.name for a in cfg.arms)
            + (" — each leg taken under each in turn; tables render the first" if len(cfg.arms) > 1 else "")
        )
    unpublishable = cfg.unpublishable_reason
    if unpublishable:
        log(
            "!! not publishable, and its tables must not be folded into measurements.md: "
            + unpublishable
        )

    # Named before the first reading, not discovered at fold-in: a selection
    # that takes half of a shared reading is publishable only with the note
    # that says so, and the closure is what the note has to name.
    gaps = borrowing.closure_gaps(figures)
    if gaps:
        log("!! this sitting is short of the set it shares readings with:")
        for gap in gaps:
            log("!!   " + gap)
    # The reverse edge, which the closure deliberately does not carry: a table
    # this sitting is not taking is a difference over reps it *is* re-taking.
    # Named, not dragged in -- `derived_consumers` says why.
    derivations = borrowing.derivation_gaps(figures)
    if derivations:
        log("!! this sitting re-takes reps a figure it does not take derives from:")
        for line in derivations:
            log("!!   " + line)

    stager = staging.Stager(cfg, log)
    stager.plan(figures)
    problems = stager.preflight(figures)
    problems += preflight.apparatus_preflight(cfg, log)
    problems += preflight.memory_preflight(cfg, figures, contained.machine_bytes())
    problems += preflight.mount_preflight(
        cfg,
        [cfg.cache_dir, cfg.warm_dir, *([cfg.nvme_dir] if any(fig.nvme_inputs for fig in figures) else [])],
    )
    if any(fig.counts_instructions for fig in figures):
        problems += preflight.counter_preflight(cfg, log)
    log(
        f"tmpfs budget: {stager.budget() / config.GIB:.2f} GiB "
        f"({staging.WARM_FULL_INPUTS} full-size inputs + {staging.WARM_SLACK // config.MIB} MiB; largest warm set "
        f"{max(stager.group_need.values(), default=0) / config.GIB:.2f} GiB)"
        + ("" if stager.budget() == staging.warm_bound(cfg) else ", capped by PGDT_MEASURE_TMPFS_BUDGET_GIB")
    )
    if problems:
        for problem in problems:
            log(f"!! {problem}")
        log("nothing was run: every one of these is knowable before the first measurement.")
        log_file.close()
        return 2
    session = timing.Session(cfg, stager, log, out_root)
    if glibc is not None:
        session.glibcs[cfg.image] = glibc
    # The apparatus, in the order it has to be established: pin the governor
    # (a machine-wide change, restored on the way out), then start sampling.
    # `--dry-run` touches neither: it runs nothing worth witnessing and must
    # not need root.
    governor = None
    if not cfg.dry_run:
        if cfg.pin_governor:
            governor = machine_state.GovernorPin(sudo=cfg.sudo, log=log)
            governor.__enter__()
        session.sampler.start()

    #: One entry per figure that produced a table: its id, the section heading
    #: it opens (empty where it shares one), and everything below the marker.
    #: Assembled into `parts` after the loop, when the marker is decidable.
    rendered: list[tuple[str, str, str]] = []
    sections_seen: set[str] = set()
    failures: list[tuple[str, str]] = []
    # Figures that lost a leg to the OOM killer. A kill inside `KILL_TOLERANT`
    # is a reading rather than an apparatus failure, so the sitting goes on —
    # but the figure it belongs to may not be published from it, because a
    # censored cell is a bound rather than a number and the table would read as
    # though the arrangement had been measured.
    censored: list[tuple[str, int]] = []
    for i, fig in enumerate(figures):
        blocked = [r for r, _ in failures if r in fig.requires]
        if blocked:
            # A figure that borrows a reading from one that failed cannot be
            # measured on its own terms; say so rather than dying on the
            # missing key.
            log(f"\n=== {fig.id} skipped — borrows from {', '.join(blocked)}, which failed")
            failures.append((fig.id, f"skipped: borrows from {', '.join(blocked)}"))
            continue
        log(f"\n=== {fig.id} ({fig.stage}) — {fig.section}")
        session.figure_id = fig.id
        # Per figure and reset each time, so a figure that needs more than the
        # recorded 512 MB cannot leave the next one running under its ceiling.
        session.memory = fig.memory
        if fig.memory:
            log(f"    container memory {fig.memory}, against the recorded {cfg.memory}")
        first_record = len(session.records)
        started = time.time()
        try:
            for name in fig.cold_inputs:
                stager.ensure_generated(name)
            for name in fig.nvme_inputs:
                stager.nvme_path(name)
            session.stage(fig.id, 0)
            body = runners.RUNS[fig.id](session)
        except staging.StagingError as exc:
            # Every later figure stages through the same area, so the sweep
            # ends here rather than failing each of them in turn.
            log(f"!! {fig.id} failed staging, and the sweep is aborted: {exc}")
            failures.append((fig.id, str(exc)))
            for rest in figures[i + 1 :]:
                failures.append((rest.id, "skipped: the sweep was aborted by a staging failure"))
            break
        except Exception as exc:  # one figure failing must not lose the others
            log(f"!! {fig.id} failed: {exc}")
            failures.append((fig.id, str(exc)))
            continue
        took = time.time() - started
        kills = session.figure_kills(fig.id)
        if kills:
            censored.append((fig.id, sum(kills.values())))
            log(
                f"!! {fig.id}: {sum(kills.values())} rep(s) OOM-killed across "
                f"{len(kills)} leg(s) — recorded as censored readings; this figure "
                "must not be published from this sitting"
            )
            for key, n in sorted(kills.items()):
                log(f"!!   {key}: {n}")
        apparatus = machine_state.apparatus_note(session.records[first_record:])
        if apparatus:
            log("    " + apparatus.strip())
        log(f"--- {fig.id} done in {took:.0f} s")
        quoting = consumers.consumers(fig)
        note = (
            "\n**The fold-in must also re-read**, because these repeat this figure's numbers "
            "or the claim it licenses: " + ", ".join(f"`{q}`" for q in quoting) + ".\n"
            if quoting
            else ""
        )
        heading = "" if fig.section in sections_seen else f"## {fig.section}\n\n"
        sections_seen.add(fig.section)
        label = f"**{fig.table_label}**\n\n" if fig.table_label else ""
        bar = censored_note(kills) if kills else ""
        # The marker is filled in below, not here: whether this run re-stamps
        # the document is a fact about the *whole* sitting, and a figure taken
        # third cannot know that the nineteenth will fail.
        rendered.append((fig.id, heading, f"{label}{bar}{body}\n{apparatus}{note}"))

    # What the sitting actually took. A selection short of the sweep never
    # stamped; a selection of the whole sweep that lost a figure must not
    # either, because the tables it did not take stay in the document from
    # whatever sitting put them there and the stamp speaks for those too.
    whole_sweep = stamps_the_document(selected_sweep, failures)
    if selected_sweep and failures:
        log(
            f"\n!! the whole sweep was selected and {len(rendered)} of {len(figures)} figures "
            "produced a table, so this sitting does not re-stamp the document: each table it "
            "did take declares this commit inside its own marker instead"
        )
    # The glibc each figure's programs ran under, from the places the sitting
    # recorded; its marker names it wherever the stamp does not speak for it.
    ran_under = {
        fid: leg_binaries.glibc_named(session.places.get(fid, ()), session.glibcs) for fid, _, _ in rendered
    }
    markers = {
        fid: doc_markers.figure_marker(
            fid,
            None if whole_sweep else head,
            glibc=doc_markers.marker_glibc(whole_sweep, glibc, ran_under[fid]),
        )
        for fid, _, _ in rendered
    }
    parts = [f"{heading}{markers[fid]}\n\n{tail}" for fid, heading, tail in rendered]

    stager.cleanup()
    if session.groups:
        session.place_harness(session._harness_home)
    if not cfg.dry_run:
        session.sampler.stop()
    if governor is not None:
        governor.__exit__()

    # What the doc still carries from somewhere other than this sitting: the
    # markers already in it, minus whatever this run just replaced.
    doc = config.REPO / "docs/design/measurements.md"
    taken = {f.id for f in figures}
    outside = sorted(
        (fid, sha)
        for fid, sha in (doc_markers.figure_sittings(doc.read_text()) if doc.exists() else {}).items()
        if fid not in taken
    )
    # A whole sweep stamps the document; a sitting of its own does not, and
    # says what it is instead of writing a stamp nobody may paste.
    if whole_sweep:
        lead = session_stamp(head, dirty, allocator, outside, glibc)
    else:
        # `len(rendered)`, not `len(figures)`: the lead counts the tables below
        # it, and a selected figure that failed produced none.
        lead = partial_lead(len(rendered), head, dirty, allocator, unpublishable, glibc)
    header = [
        "# measure.py output",
        "",
        lead,
        "",
        "Each section below is one figure, ready to paste under its heading in "
        "`docs/design/measurements.md`.",
        "",
    ]
    if unpublishable:
        header += ["> **NOT PUBLISHABLE.** " + unpublishable, ""]
    elif governor is not None and not governor.ok:
        # The governor is part of the recorded apparatus, so failing to pin it
        # departs from that apparatus exactly as a resized input does.
        header += [
            f"> **NOT PUBLISHABLE.** The `{machine_state.SWEEP_GOVERNOR}` governor could not be pinned "
            f"({governor.error}), so these readings were taken under whatever scaling "
            "policy the machine was left in. Fix that and re-run.",
            "",
        ]
    if gaps:
        header += [
            "> **Short of the set it shares readings with.** Each of these publishes a reading "
            "a figure this sitting did not take also publishes, so their absolutes may not be "
            "set beside those tables' until all of them are re-taken together: "
            + "; ".join(gaps)
            + ".",
            "",
        ]
    if derivations:
        header += [
            "> **A table this sitting did not take is derived from these reps.** Its published "
            "row is computed over readings replaced below, so folding one of these in without "
            "re-taking that table leaves a difference over reps the doc no longer holds: "
            + "; ".join(derivations)
            + ".",
            "",
        ]
    if censored:
        header += [
            "> **NOT PUBLISHABLE — a leg was OOM-killed.** "
            + "; ".join(f"`{fid}` lost {n} rep(s)" for fid, n in censored)
            + ". The kill is a reading and is recorded as one, but a censored cell is a "
            "bound on a peak the process never reached rather than the peak the "
            "arrangement holds, so the table above it describes an arrangement that was "
            "not measured. Fix what kills it, or state the finding from the kill itself, "
            "and re-take the figure.",
            "",
        ]
    if failures:
        header += ["> **Figures that failed:** " + ", ".join(f"`{f}` ({m})" for f, m in failures), ""]

    (out_root / "tables.md").write_text("\n".join(header) + "\n" + "\n".join(parts))
    (out_root / "raw.json").write_text(
        json.dumps(
            {
                "commit": head,
                "dirty": dirty,
                "date": date.today().isoformat(),
                "config": {k: str(v) for k, v in dataclasses.asdict(cfg).items()},
                "figures": [f.id for f in figures],
                "failures": failures,
                # Everything below this line exists so `--render` can rebuild
                # `tables.md` from this file alone, measuring nothing. A
                # presentation-only change to a renderer is folded in by
                # re-rendering the sitting, never by hand-editing the pasted
                # table -- which is how a table came to disagree with the
                # harness that would have produced it.
                "allocator": allocator,
                "glibc": glibc,
                "figure_glibc": ran_under,
                "places": {fid: sorted(where) for fid, where in session.places.items()},
                "glibcs": session.glibcs,
                "whole_sweep": whole_sweep,
                "header": header,
                "input_sizes": recorded_input_sizes(stager, figures),
                "readings": session.readings,
                "rss": session.rss,
                "instructions": session.instructions,
                "killed": session.killed,
                "reported": session.reported,
                "instrument": session.instrument,
                "telemetry": session.telemetry,
                # Every arm's readings, where a leg was taken under more than
                # one; `readings` above is the first arm's (`Arm`).
                **(
                    {
                        "arms": {
                            "primary": cfg.arms[0].name,
                            "names": [a.name for a in cfg.arms],
                            "readings": session.arm_readings,
                        }
                    }
                    if len(cfg.arms) > 1
                    else {}
                ),
                "governor": {
                    "requested": machine_state.SWEEP_GOVERNOR if cfg.pin_governor else None,
                    "pinned": bool(governor and governor.ok),
                    "error": governor.error if governor else None,
                },
                "runs": session.records,
            },
            indent=1,
        )
        + "\n"
    )
    log(f"\nwrote {out_root / 'tables.md'} and {out_root / 'raw.json'}")
    log_file.close()
    # A censored sitting exits non-zero like a failed one. It *completed*, and
    # that is the point: "the sitting finished" is no longer the same claim as
    # "the figure may be published", so the exit code has to carry the second.
    # A detached `--figure reserve --alone` whose gate is "no leg was killed"
    # is then answerable without reading the log.
    return 1 if (failures or censored) else 0


def cmd_list() -> None:
    print("Figures (run order; one figure is one table):\n")
    for fig in register.ALL_FIGURES:
        print(f"  {fig.id:<24} [{fig.stage}]  {fig.section}")
        if fig.requires:
            print(f"  {'':<24}  borrows readings from: {', '.join(fig.requires)}")
        closure = borrowing.sharing_closure(fig.id)
        if closure:
            print(f"  {'':<24}  re-take it with: {', '.join(closure)}")
        print(f"  {'':<24}  invalidated by: {', '.join(fig.depends)}")
        print(f"  {'':<24}  quoted by: {', '.join(consumers.consumers(fig)) or '(nothing else)'}")
        reproduce = (
            "--drift <sweep> <sweep>" if fig.stage == "derived" else f"--figure {fig.id}"
        )
        print(f"  {'':<24}  reproduce: cd scripts && uv run measure.py {reproduce}")
        print()
    if register.UNTAKEN:
        print("Built, not taken — no table in the doc until someone runs it:\n")
        for fig in register.UNTAKEN:
            print(f"  {fig.id:<24} [{fig.stage}]  {fig.section}")
            print(f"  {'':<24}  invalidated by: {', '.join(fig.depends)}")
            print(f"  {'':<24}  take it: cd scripts && uv run measure.py --figure {fig.id}")
            print()
    print("Not emitted here, deliberately (each declares itself in the doc "
          "with `<!-- outside-register: <id> -->`):\n")
    for outside in outside_sections.NOT_OURS.values():
        print(f"  {outside.id:<24} {outside.section}\n  {'':<24}  {outside.why}")
        edge = ", ".join(outside.depends) or "(nothing — it publishes no reading)"
        print(f"  {'':<24}  invalidated by: {edge}\n")
