"""Tests of scripts/bench_report.py and scripts/bench_compare.py."""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from util import read_json, read_text, write_json  # noqa: E402
import bench_compare  # noqa: E402
import bench_lib  # noqa: E402
import bench_report  # noqa: E402
from test_bench_gate import document  # noqa: E402


def with_ms(ms, **kwargs):
    """A good schema-3 report whose `scan` case takes `ms`."""
    doc = document(**kwargs)
    doc["cases"]["scan"]["timing"]["median_of_medians_ms"] = ms
    doc["cases"]["scan"]["checksum"] = 5
    return doc


class CompareTest(unittest.TestCase):
    def test_comparable_reports_pass_and_classify_changes(self):
        doc, status = bench_compare.compare(with_ms(10), with_ms(10.2))
        self.assertEqual((status, doc["cases"]["scan"]["verdict"]), (bench_lib.PASS, "same"))
        self.assertEqual(bench_compare.compare(with_ms(10), with_ms(13))[0]["cases"]["scan"]["verdict"], "slower")
        self.assertEqual(bench_compare.compare(with_ms(10), with_ms(7))[0]["cases"]["scan"]["verdict"], "faster")

    def test_legacy_reports_are_diagnostic(self):
        legacy = {"milestone": "2.5.4", "cases": {"scan": {"p50_ms": 10.0, "checksum": 5}}}
        doc, status = bench_compare.compare(legacy, with_ms(10))
        self.assertEqual(status, bench_lib.DIAGNOSTIC)
        self.assertIn("scan", doc["cases"])

    def test_a_non_performance_governor_is_diagnostic(self):
        self.assertEqual(bench_compare.compare(with_ms(10), with_ms(10, governor="powersave"))[1], bench_lib.DIAGNOSTIC)

    def test_unstable_cases_and_deterministic_differences(self):
        self.assertEqual(bench_compare.compare(with_ms(10), with_ms(10, stability="unstable"))[1], bench_lib.UNSTABLE)
        changed = with_ms(10)
        changed["cases"]["scan"]["checksum"] = 6
        self.assertEqual(bench_compare.compare(with_ms(10), changed)[1], bench_lib.FAIL)

    def test_command_line_exit_codes(self):
        with tempfile.TemporaryDirectory() as tmp:
            paths = []
            for name, doc in (("a", with_ms(10)), ("b", with_ms(10)), ("c", with_ms(10, governor="powersave"))):
                path = os.path.join(tmp, name + ".json")
                write_json(path, doc)
                paths.append(path)
            self.assertEqual(bench_compare.main([paths[0], paths[1]]), 0)
            self.assertEqual(bench_compare.main([paths[0], paths[2]]), 4)
            self.assertEqual(bench_compare.main([paths[0], os.path.join(tmp, "x.json")]), 2)


class ReportTest(unittest.TestCase):
    def test_rendering_a_report_and_a_legacy_report(self):
        text = bench_report.render(with_ms(1.5), "run")
        self.assertIn("| scan | 1.5000 |", text)
        self.assertIn("governor performance", text)
        self.assertIn("gate status: **pass**", text)
        legacy = bench_report.render({"cases": {"scan": {"p50_ms": 2.0}}}, "old")
        self.assertIn("(legacy report)", legacy)
        self.assertIn("| scan | 2.0000 |", legacy)
        self.assertIn("no timed cases", bench_report.render({"cases": {}}))

    def test_command_line_writes_a_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            src, out = os.path.join(tmp, "r.json"), os.path.join(tmp, "r.md")
            write_json(src, with_ms(1.0))
            self.assertEqual(bench_report.main([src, "--out", out]), 0)
            self.assertIn("## ", read_text(out))
            self.assertEqual(bench_report.main([os.path.join(tmp, "none.json")]), 2)


if __name__ == "__main__":
    unittest.main()
