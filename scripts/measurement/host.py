"""Running one command on the host, its output captured or not (`run`)."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Sequence


def run(
    argv: Sequence[str], cwd: Path | None = None, capture: bool = False, quiet: bool = False
) -> str:
    stdout = subprocess.PIPE if capture else (subprocess.DEVNULL if quiet else None)
    proc = subprocess.run(
        list(argv), cwd=str(cwd) if cwd else None, text=True, stdout=stdout, check=True
    )
    return proc.stdout or ""
