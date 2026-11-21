"""The gate's legs: the build every gate reading is read on beside the shipped one (`gate_twin`)."""

from __future__ import annotations

import dataclasses
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from measurement.runspec import RunSpec


#: The build every **gate** reading is read on beside the shipped one.
#:
#: The gate is the pass/fail readings the harness takes against a memory
#: limit, each failing on a kill alone: `reserve`'s flagless legs, a kill
#: being a censored reading (`KILL_TOLERANT`), the margin each leaves
#: reported beside the verdict (`REPORTED_MARGIN_PERCENT`);
#: `parallel-peak-rss`'s legs in `PARALLEL_MEMORY` and `gzip-resident`'s in
#: `GZPASS_MEMORY`, each the library's own `budget + PARALLEL_HEADROOM`; and a
#: kill anywhere else, which raises
#: (`Session.time_run`). Each is read on this
#: build too, at the same commit in the same sitting, so the two legs differ by
#: the allocator alone, as the `allocator` figure's legs do for time — and the
#: build *is* that figure's `system` leg (`ensure_allocator_binary`), never a
#: second build of it.
#:
#: **What the pair decides is whose failure it is.** A reading failing on both
#: legs is not the allocator's and is filed, against the reserve (`KD34`) or as
#: a deficiency of its own; one failing on the shipped build alone is the
#: allocator's, attributed on the instrument build before a remedy is chosen,
#: and reopens D13 only where that entry's Reopens line says. A reading is
#: never judged against an earlier sitting's platform-allocator figure instead:
#: that sitting differs by every commit since, so a failure judged against it
#: could not be told from theirs (`.claude/skills/evidence/SKILL.md`, rule 7).
GATE_LEG = "system"

#: `GATE_LEG` as a `RunSpec.binary` (`Session.binary_path`).
GATE_BINARY = f"alloc:{GATE_LEG}"

#: The `RunSpec.binary` values that run the shipped build: `pgdt`, and
#: `pgdt sql` under the provider legs' name (`SQL_SHELL`). Only these have a
#: gate twin — an instrument build or another allocator leg is not what ships.
SHIPPED_BINARIES: tuple[str, ...] = ("pgdt", "dfcli")


def gate_twin(spec: RunSpec) -> RunSpec:
    """`spec` on `GATE_BINARY`: the same command, input, regime and limit.

    It differs by `RunSpec.binary` alone, which `key` carries, so neither leg
    can be read as a rep of the other; and since the command is the same, a
    shape `KILL_TOLERANT` licenses is licensed on both legs alike."""
    if spec.binary not in SHIPPED_BINARIES:
        raise ValueError(
            f"{spec.label}: only a leg of the shipped build has a gate twin, not {spec.binary!r}"
        )
    return dataclasses.replace(spec, binary=GATE_BINARY, label=f"{spec.label}, {GATE_LEG}")
