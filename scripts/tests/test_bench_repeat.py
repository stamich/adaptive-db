"""Tests of scripts/bench_repeat.py on canned runs."""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from util import read_json, read_text, write_json  # noqa: E402
import bench_lib  # noqa: E402
import bench_repeat  # noqa: E402


def run(scan_ms, checksum=7, value=100):
    """A canned report: one timed case with a checksum and one untimed case."""
    return {"schema": 3, "cases": {
        "scan": {"checksum": checksum, "timing": {"median_of_medians_ms": scan_ms}},
        "rows": {"value": value},
    }}


class RepeatTest(unittest.TestCase):
    def test_median_and_spread(self):
        self.assertEqual(bench_repeat.median([3.0, 1.0, 2.0]), 2.0)
        self.assertAlmostEqual(bench_repeat.spread_percent([9.0, 10.0, 11.0]), 20.0)
        self.assertEqual(bench_repeat.spread_percent([0.0, 0.0]), 0.0)

    def test_steady_runs_pass_and_carry_a_repeat_object(self):
        doc, problems, status = bench_repeat.condense([run(10.0), run(10.2), run(9.9)])
        self.assertEqual((problems, status), ([], bench_lib.PASS))
        repeat = doc["cases"]["scan"]["repeat"]
        self.assertEqual(repeat["median_ms"], 10.0)
        self.assertEqual(repeat["runs"], 3)
        self.assertNotIn("repeat", doc["cases"]["rows"])
        self.assertTrue(doc["repeat"]["limit_is_provisional"])

    def test_a_wide_spread_is_unstable(self):
        _, _, status = bench_repeat.condense([run(10.0), run(14.0), run(10.0)])
        self.assertEqual(status, bench_lib.UNSTABLE)
        _, _, lax = bench_repeat.condense([run(10.0), run(14.0), run(10.0)], limit=50.0)
        self.assertEqual(lax, bench_lib.PASS)

    def test_a_deterministic_difference_fails_even_when_timings_agree(self):
        _, problems, status = bench_repeat.condense([run(10.0), run(10.0, checksum=8)])
        self.assertEqual(status, bench_lib.FAIL)
        self.assertIn("`scan`", problems[0])
        _, problems, status = bench_repeat.condense([run(10.0), run(10.0, value=101)])
        self.assertEqual(status, bench_lib.FAIL)

    def test_cases_outside_the_cases_object_are_found(self):
        """rust-local keeps its 2.5.4 layout: top-level and nested cases with p50_ms."""
        def layout(ms):
            return {"entity_scan": {"p50_ms": ms, "checksum": 3}, "relational": {"hash_join": {"p50_ms": ms * 2}},
                    "environment": {"p50_ms": 1.0}, "cases": {}}
        self.assertEqual(sorted(bench_lib.cases(layout(1.0))), ["entity_scan", "relational.hash_join"])
        doc, _, status = bench_repeat.condense([layout(10.0), layout(10.1)])
        self.assertEqual(status, bench_lib.PASS)
        self.assertAlmostEqual(doc["relational"]["hash_join"]["repeat"]["median_ms"], 20.1)
        self.assertIn("repeat", doc["entity_scan"])
        self.assertNotIn("repeat", doc["environment"])

    def test_a_changed_case_set_fails(self):
        other = run(10.0)
        del other["cases"]["rows"]
        _, problems, status = bench_repeat.condense([run(10.0), other])
        self.assertEqual(status, bench_lib.FAIL)
        self.assertTrue(problems)

    def test_command_line_with_inputs_and_with_a_command(self):
        with tempfile.TemporaryDirectory() as tmp:
            paths = []
            for index, ms in enumerate((10.0, 10.1, 10.0)):
                path = os.path.join(tmp, f"r{index}.json")
                write_json(path, run(ms))
                paths.append(path)
            out = os.path.join(tmp, "median.json")
            self.assertEqual(bench_repeat.main(["--out", out, "--inputs", *paths]), 0)
            self.assertEqual(read_json(out)["repeat"]["runs"], 3)
            writer = f"{sys.executable} -c \"import json,sys; open(sys.argv[1],'w').write(json.dumps({run(5.0)!r}))\" {{out}}"
            self.assertEqual(bench_repeat.main(["--runs", "2", "--out", out, "--cmd", writer]), 0)
            self.assertEqual(read_json(out)["repeat"]["runs"], 2)
            self.assertEqual(bench_repeat.main(["--runs", "2", "--out", out, "--cmd", "false"]), 2)
            self.assertEqual(bench_repeat.main(["--out", out, "--inputs", os.path.join(tmp, "nope.json")]), 2)


if __name__ == "__main__":
    unittest.main()
