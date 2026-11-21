"""What the harness's tests share.

Stdlib `unittest`, no dependency added to a repo that has none. What is tested
is where a silent error would be worst: the median, the spread, the table
formatting and the parsing of another tool's output. A wrong median produces a
*confidently wrong* table, which is the failure the harness exists to prevent —
whereas a broken container invocation fails loudly, on the first run, and needs
root and a container runtime to exercise at all.

The structural tests are the other half: they hold the register itself to the
rules that make selection safe — every figure declares what invalidates it, and
a figure that borrows a reading runs after the figure that takes it.

Run them all with `cd scripts && uv run python -m unittest discover -s
measurement -t .`, or one module's with `uv run python -m unittest
measurement.test_<module>`.
"""

from __future__ import annotations

import contained
import importlib
import pkgutil
import re
import release
import unittest
import unittest.mock
from pathlib import Path
from types import ModuleType

import measurement
from measurement import config
from measurement import runspec
from measurement.builds import image
from measurement.figures import declaration
from measurement.figures import register


def harness_modules() -> list[ModuleType]:
    """Every production module of the harness: the package's, its tests left
    out, and the command line beside it."""
    found = [importlib.import_module("measure")]
    for info in pkgutil.walk_packages(measurement.__path__, "measurement."):
        if not info.name.rpartition(".")[2].startswith("test_"):
            found.append(importlib.import_module(info.name))
    return found


def harness_has(name: str) -> bool:
    """Whether any module of the harness defines `name`."""
    return any(hasattr(module, name) for module in harness_modules())


def harness_source() -> str:
    """The harness's production source, every module's in turn."""
    return "\n".join(Path(module.__file__).read_text() for module in harness_modules())


def read_path_source() -> str:
    """The read path's production source, every file `declaration.READ` declares:
    what a check reading the library's own constants and arithmetic off the
    source reads, whichever of the module's files holds the line."""
    return "\n".join((config.REPO / path).read_text() for path in declaration.READ)


def without_the_release_image(test: unittest.TestCase) -> None:
    """Patch the release image away for one test, so a build in it is the
    container run the test's `fake_run` sees and nothing is built or made."""
    test.enterContext(
        unittest.mock.patch.object(release, "ensure_image", return_value="pgdt-release:t")
    )
    test.enterContext(unittest.mock.patch.object(release, "prepare_state"))


def leg_binary(cfg: "config.Config", leg: str) -> Path:
    """Where the image's build of one allocator leg lands on the host."""
    build = release.Build(features=(leg,), default_features=False)
    return build.binary(image.image_target_dir(cfg, leg))


def profile_leg_binary(cfg: "config.Config", leg: str) -> Path:
    """Where the image's build of one `release-profile` leg lands on the host."""
    return release.Build(profile=leg).binary(image.image_target_dir(cfg, leg))


def _peak_rss_specs() -> list:
    """`peak-rss`'s specs, without running the figure."""
    return [
        runspec.RunSpec("pgdt", name, "parse-rss", "warm", name) for name in register._RSS_ROWS
    ]


def recipe_uncontained(recipe: str) -> list[str]:
    """Each command of a printed recipe, continuations joined and comments
    dropped, that runs a reading outside a container, by the hook's own
    classifier (`contained.uncontained`)."""
    joined = re.sub(r"\\\n", " ", recipe)
    found = []
    for line in joined.splitlines():
        if line.strip() and not line.lstrip().startswith("#"):
            refused = contained.uncontained(line)
            if refused is not None:
                found.append(refused)
    return found
