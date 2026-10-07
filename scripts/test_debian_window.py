#!/usr/bin/env python3
"""Unit tests for debian_window.py, run with `uv run python -m unittest test_debian_window`.

The window's three refusals, its one reported-not-refused band and its edges are
held here against a table of Debian's release days; nothing reads the network.
"""

from __future__ import annotations

import contextlib
import datetime
import io
import shutil
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import debian_window as dw
import release
from release import ReleaseError

D = datetime.date
DIGEST = "sha256:" + "a" * 64

CSV = """version,codename,series,created,release,eol,eol-lts,eol-elts
12,Bookworm,bookworm,2021-08-14,2023-06-10,2026-07-11,2028-06-30,2033-06-30
13,Trixie,trixie,2023-06-10,2025-08-09,2028-08-09,2030-06-30,2035-06-30
14,Forky,forky,2025-08-09
15,Duke,duke,2027-08-01
,Sid,sid,1993-08-16
"""


def verdict(pin: str, today: D, text: str = CSV) -> dw.Verdict:
    return dw.assess(dw.parse_csv(text), pin, today)


class AddMonths(unittest.TestCase):
    def test_whole_months(self):
        self.assertEqual(dw.add_months(D(2025, 8, 9), 6), D(2026, 2, 9))
        self.assertEqual(dw.add_months(D(2025, 8, 9), 12), D(2026, 8, 9))

    def test_a_year_boundary(self):
        self.assertEqual(dw.add_months(D(2025, 11, 20), 3), D(2026, 2, 20))

    def test_clamped_to_the_last_day_of_a_short_month(self):
        self.assertEqual(dw.add_months(D(2025, 8, 31), 6), D(2026, 2, 28))
        self.assertEqual(dw.add_months(D(2023, 8, 31), 6), D(2024, 2, 29))


class Parse(unittest.TestCase):
    def test_an_unreleased_series_has_no_release_day(self):
        by = {s.codename: s.released for s in dw.parse_csv(CSV)}
        self.assertEqual(by["trixie"], D(2025, 8, 9))
        self.assertIsNone(by["forky"])
        self.assertIsNone(by["sid"])

    def test_a_table_without_the_columns_is_refused(self):
        with self.assertRaises(ReleaseError):
            dw.parse_csv("<html>not a csv</html>\n")

    def test_a_release_day_that_is_not_a_date_is_refused(self):
        with self.assertRaises(ReleaseError):
            dw.parse_csv("series,release\ntrixie,soon\n")


class Window(unittest.TestCase):
    def test_stable_with_no_successor_holds(self):
        v = verdict("trixie", D(2026, 10, 7))
        self.assertTrue(v.ok, v.message)

    def test_an_unreleased_codename_is_refused(self):
        for pin in ("forky", "sid", "nonesuch"):
            self.assertFalse(verdict(pin, D(2026, 10, 7)).ok, pin)

    def test_a_codename_is_unreleased_before_its_day(self):
        self.assertFalse(verdict("trixie", D(2025, 8, 8)).ok)

    def test_a_codename_under_six_months_old_is_refused(self):
        v = verdict("trixie", D(2026, 2, 8))
        self.assertFalse(v.ok)
        self.assertIn("2026-02-09", v.message)

    def test_a_codename_exactly_six_months_old_is_allowed(self):
        self.assertTrue(verdict("trixie", D(2026, 2, 9)).ok)

    def test_the_predecessor_holds_through_the_grace_period(self):
        table = CSV.replace("14,Forky,forky,2025-08-09", "14,Forky,forky,2025-08-09,2027-08-01")
        v = verdict("trixie", D(2027, 12, 31), table)
        self.assertTrue(v.ok, v.message)
        self.assertIn("grace period runs to 2028-02-01", v.message)

    def test_between_six_and_twelve_months_the_day_is_the_maintainers(self):
        table = CSV.replace("14,Forky,forky,2025-08-09", "14,Forky,forky,2025-08-09,2027-08-01")
        v = verdict("trixie", D(2028, 2, 1), table)
        self.assertTrue(v.ok, v.message)
        self.assertIn("may move now and must by 2028-08-01", v.message)

    def test_a_predecessor_kept_twelve_months_is_refused(self):
        table = CSV.replace("14,Forky,forky,2025-08-09", "14,Forky,forky,2025-08-09,2027-08-01")
        self.assertTrue(verdict("trixie", D(2028, 7, 31), table).ok)
        v = verdict("trixie", D(2028, 8, 1), table)
        self.assertFalse(v.ok)
        self.assertIn("forky", v.message)

    def test_a_pin_two_releases_behind_is_refused_by_the_first_successor(self):
        v = verdict("bookworm", D(2026, 10, 7))
        self.assertFalse(v.ok)
        self.assertIn("trixie", v.message)

    def test_a_successor_not_yet_released_does_not_start_the_clock(self):
        # `duke` carries a creation day in the future and no release day.
        self.assertTrue(verdict("trixie", D(2030, 1, 1)).ok)


class Pin(unittest.TestCase):
    def test_the_codename_is_read_from_the_digest_pinned_base(self):
        self.assertEqual(dw.pinned_codename(f"FROM debian:trixie@{DIGEST}\nRUN true\n"), "trixie")

    def test_an_unpinned_base_is_refused(self):
        with self.assertRaises(ReleaseError):
            dw.pinned_codename("FROM debian:trixie\n")

    def test_another_distribution_is_refused(self):
        with self.assertRaises(ReleaseError):
            dw.pinned_codename(f"FROM ubuntu:noble@{DIGEST}\n")

    def test_the_committed_dockerfile_names_a_debian_codename(self):
        self.assertRegex(dw.pinned_codename(release.DOCKERFILE.read_text()), r"^[a-z]+$")


class Main(unittest.TestCase):
    def run_main(self, *argv: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = dw.main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def csv_file(self, text: str = CSV) -> Path:
        d = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, d)
        path = d / "debian.csv"
        path.write_text(text)
        return path

    def test_a_held_pin_exits_zero(self):
        with unittest.mock.patch.object(release, "DOCKERFILE", self.dockerfile("trixie")):
            code, out, _ = self.run_main("--csv", str(self.csv_file()), "--today", "2026-10-07")
        self.assertEqual((code, out.startswith("ok: ")), (0, True))

    def test_a_refused_pin_exits_one(self):
        with unittest.mock.patch.object(release, "DOCKERFILE", self.dockerfile("forky")):
            code, out, _ = self.run_main("--csv", str(self.csv_file()), "--today", "2026-10-07")
        self.assertEqual((code, out.startswith("refused: ")), (1, True))

    def test_a_source_that_cannot_be_read_is_never_a_pass(self):
        with unittest.mock.patch.object(dw, "fetch", side_effect=ReleaseError("offline")):
            code, out, err = self.run_main("--today", "2026-10-07")
        self.assertEqual((code, out), (2, ""))
        self.assertIn("offline", err)

    def test_fetch_names_the_url_it_could_not_read(self):
        with unittest.mock.patch("urllib.request.urlopen", side_effect=OSError("no route")):
            with self.assertRaises(ReleaseError) as e:
                dw.fetch("https://example.invalid/debian.csv")
        self.assertIn("https://example.invalid/debian.csv", str(e.exception))

    def dockerfile(self, codename: str) -> Path:
        d = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, d)
        path = d / "Dockerfile"
        path.write_text(f"FROM debian:{codename}@{DIGEST}\n")
        return path


if __name__ == "__main__":
    unittest.main()
