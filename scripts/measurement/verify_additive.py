"""`--verify-additive`: every figure's inputs regenerated at two revisions and compared byte for
byte, settling whether a generator's edit moved them."""

from __future__ import annotations

import dataclasses
import re
import sys
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING

from measurement import config
from measurement import git
from measurement import host
from measurement import inputs
from measurement.figures import register
from measurement.staleness import sitting_checks

if TYPE_CHECKING:
    from measurement.inputs import InputSpec


#: The size `--verify-additive` generates at. Small enough that verifying every
#: input is seconds rather than the sweep's tens of gigabytes, and large enough
#: that every value shape the generators draw from appears many times over: the
#: same seed draws the same row sequence at any size, so a prefix that matches
#: byte for byte is the row logic matching, not a coincidence of length.
VERIFY_SIZE_GIB = 0.03


def cmd_verify_additive(since: str | None) -> int:
    """Regenerate every figure's inputs at two revisions and compare bytes.

    The one class of staleness that can be settled mechanically. A figure's
    `depends` names its generator, so *any* edit to that script marks it stale
    — including one that only adds a flag. Rather than trusting a reading of
    the diff, generate both ways and compare: identical bytes mean the figure
    would have been taken on the same input, which is the whole of what its
    generator dependency claims.

    What this cannot settle is a change to library code or to the harness's own
    timing path, where there is no cheap oracle and the honest answer is a
    sweep. Those stay stale, and `--stale` keeps saying so.

    **A figure's inputs are regenerated at that figure's own base**, which is
    its sitting where its marker declares one and the session stamp otherwise.
    Reading the stamp for all of them rested on "the figure was taken at the
    stamp, so its inputs existed then" — a sentence a figure published outside
    the sweep falsifies, and one whose failure is silent: the older revision
    generates *something*, and the comparison then answers a question nobody
    asked.
    """
    doc = config.REPO / "docs/design/measurements.md"
    bases = sitting_checks.figure_bases(doc.read_text(), since)
    if any(bases[fig.id] is None for fig in register.ALL_FIGURES):
        print(
            "no --since given and measurements.md carries no session stamp naming a commit; "
            "pass --since <rev>",
            file=sys.stderr,
        )
        return 2

    changed_since = {rev: set(git.changed_paths(rev)) for rev in sorted({*bases.values()})}
    # Only the inputs a *published* figure is taken on. An input that exists
    # solely for an untaken instrument has no bytes in the doc to be wrong
    # about, and -- as `composite_text` proved on this mechanism's first run --
    # it may not be generatable at the old revision at all, because the commit
    # under test is what added its flag. A published figure's inputs cannot be
    # new that way: the figure was taken at its own base, so they existed then.
    wanted: dict[str, dict[str, InputSpec]] = {}
    covered: dict[str, set[str]] = {}
    generators: dict[str, set[str]] = {}
    for fig in register.ALL_FIGURES:
        rev = bases[fig.id]
        for name in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs):
            spec = inputs.INPUTS[name]
            if f"scripts/{spec.generator}" in changed_since[rev]:
                wanted.setdefault(rev, {})[name] = spec
                covered.setdefault(rev, set()).add(fig.id)
                generators.setdefault(rev, set()).add(spec.generator)
    if not wanted:
        print("no generator any published figure depends on changed since that figure's base.")
        return 0

    cfg = dataclasses.replace(config.Config(), size_gib=VERIFY_SIZE_GIB)
    cfg.out_dir.mkdir(parents=True, exist_ok=True)
    differing: list[str] = []
    for rev, specs in wanted.items():
        who = sorted(covered[rev])
        print(
            f"{len(generators[rev])} generator(s) changed since {rev} "
            f"({', '.join(sorted(generators[rev]))}), which {len(who)} figure(s) are taken "
            f"against: {', '.join(who)}"
        )
        print(
            f"regenerating {len(specs)} input(s) at {VERIFY_SIZE_GIB} GiB under both revisions\n"
        )
        # Under `runs/`: gitignored, so a crashed run leaves no untracked tree
        # inside the repo being measured.
        safe = re.sub(r"[^A-Za-z0-9._-]", "_", rev)
        work = cfg.out_dir / f"worktree-verify-{safe}"
        host.run(["git", "worktree", "add", "--detach", str(work), rev], cwd=config.REPO, quiet=True)
        try:
            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = Path(tmp)
                for name, spec in sorted(specs.items()):
                    old_out = tmp_path / f"{name}.old"
                    new_out = tmp_path / f"{name}.new"
                    host.run(spec.argv(cfg, old_out), cwd=work / "scripts", quiet=True)
                    host.run(spec.argv(cfg, new_out), cwd=config.SCRIPTS, quiet=True)
                    old, new = old_out.read_bytes(), new_out.read_bytes()
                    if old == new:
                        print(f"  {name:<16} identical ({len(new)} bytes)")
                    else:
                        differing.append(f"{name} (since {rev})")
                        at = next(
                            (i for i, (a, b) in enumerate(zip(old, new)) if a != b),
                            min(len(old), len(new)),
                        )
                        print(
                            f"  {name:<16} DIFFERS at byte {at} ({len(old)} vs {len(new)} bytes)"
                        )
                    old_out.unlink()
                    new_out.unlink()
        finally:
            host.run(["git", "worktree", "remove", "--force", str(work)], cwd=config.REPO, quiet=True)
        print()

    if differing:
        print(
            f"{len(differing)} input(s) changed: {', '.join(differing)}. Those figures are "
            "genuinely stale and need a sweep; do not acknowledge them."
        )
        return 1

    accounted = sorted({fid for ids in covered.values() for fid in ids})
    print(
        f"Every input is byte-identical, so no figure was taken on different bytes. "
        f"The {len(accounted)} figure(s) this accounts for:\n"
    )
    for fid in accounted:
        print(f"  {fid}")
    print(
        "\nThat is the evidence an `Acknowledged` entry carries — record the commit that changed "
        "the generator, these figure ids, and this command as its `verified`."
    )
    return 0
