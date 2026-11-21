"""Tests of `measurement.check`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement import test_support
from measurement import check
from measurement import config


class Scaffolding(unittest.TestCase):
    """`tables.md` carries lines addressed to the session folding a table in.

    Pasting a whole section drags them into `measurements.md`, where they read
    as part of the table's own commentary. `--check` refuses that.
    """

    def test_a_clean_document_has_none(self):
        self.assertEqual(check.scaffolding_in("# doc\n\nsome prose\n"), [])

    def test_a_pasted_fold_in_note_is_caught(self):
        found = check.scaffolding_in(
            "# doc\n\n**The fold-in must also re-read**, because these repeat it: `x`.\n"
        )
        self.assertEqual(len(found), 1)
        self.assertIn("line 3", found[0])

    def test_the_document_carries_none(self):
        doc = (config.REPO / "docs/design/measurements.md").read_text()
        self.assertEqual(check.scaffolding_in(doc), [])

    def test_every_scaffolding_marker_is_something_emit_writes(self):
        # A marker that no longer matches what the harness emits would police
        # nothing, silently.
        source = test_support.harness_source()
        for marker in check.SCAFFOLDING:
            self.assertIn(marker, source)


if __name__ == "__main__":
    unittest.main()
