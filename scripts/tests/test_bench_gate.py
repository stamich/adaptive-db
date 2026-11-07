"""Tests of scripts/bench_gate.py and bench_lib.py."""
import copy
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from util import read_json, read_text, write_json  # noqa: E402
import bench_gate  # noqa: E402
import bench_lib  # noqa: E402

TIMING = {
    "median_of_medians_ms": 1.0, "batch_medians_ms": [1.0, 1.1, 0.9], "batch_mad_percent": 2.0,
    "outlier_count": 0, "outlier_indices": [], "stability": "stable", "batches": 3,
    "samples_per_batch": 7, "iterations_per_sample": 1, "quick": False,
}


def document(family="rust-local", governor="performance", filesystem="ext4", stability="stable", warm=None) -> dict:
    """A valid schema-3 document with one timed case."""
    timing = dict(TIMING, stability=stability, quick=stability == "skipped")
    case = {"p50_ms": 1.0, "timing": timing}
    if warm is not None:
        case["jit"] = {"warm": warm}
    return {
        "schema": 3, "milestone": "2.5.4.1", "family": family, "plan": {}, "runtime_before": {}, "runtime_after": {},
        "environment": {"governor": governor},
        "storage": {"filesystem": filesystem, "memory_backed": filesystem in ("tmpfs", "ramfs")},
        "cases": {"scan": case},
    }


class GateTest(unittest.TestCase):
    def test_a_good_run_passes(self):
        result = bench_gate.evaluate(document())
        self.assertEqual(result["status"], bench_lib.PASS)

    def test_a_governor_other_than_performance_is_diagnostic_never_pass(self):
        for governor in ("powersave", "schedutil", None, ""):
            result = bench_gate.evaluate(document(governor=governor))
            self.assertEqual(result["gates"]["governor"]["status"], bench_lib.DIAGNOSTIC, governor)
            self.assertEqual(result["status"], bench_lib.DIAGNOSTIC, governor)

    def test_ram_disks_are_refused_for_durable_families_only(self):
        self.assertEqual(bench_gate.evaluate(document(filesystem="tmpfs"))["status"], bench_lib.FAIL)
        self.assertEqual(bench_gate.evaluate(document(filesystem="ramfs"))["status"], bench_lib.FAIL)
        memory = bench_gate.evaluate(document(family="memory", filesystem="tmpfs"))
        self.assertEqual(memory["gates"]["storage"]["status"], bench_lib.SKIPPED)
        self.assertEqual(memory["status"], bench_lib.PASS)

    def test_unstable_cases_make_the_run_unstable(self):
        result = bench_gate.evaluate(document(stability="unstable"))
        self.assertEqual(result["gates"]["stability"]["status"], bench_lib.UNSTABLE)
        self.assertEqual(result["status"], bench_lib.UNSTABLE)

    def test_quick_runs_are_skipped_not_failed(self):
        result = bench_gate.evaluate(document(stability="skipped"))
        self.assertEqual(result["gates"]["stability"]["status"], bench_lib.SKIPPED)
        self.assertEqual(result["status"], bench_lib.PASS)

    def test_cold_jit_is_unstable(self):
        cold = bench_gate.evaluate(document(warm=False))
        self.assertEqual(cold["gates"]["jit"]["status"], bench_lib.UNSTABLE)
        self.assertEqual(bench_gate.evaluate(document(warm=True))["gates"]["jit"]["status"], bench_lib.PASS)

    def test_a_broken_schema_fails_and_a_legacy_report_is_diagnostic(self):
        broken = document()
        del broken["plan"]
        self.assertEqual(bench_gate.evaluate(broken)["status"], bench_lib.FAIL)
        legacy = bench_gate.evaluate({"milestone": "2.5.4", "host": {}})
        self.assertEqual(legacy["status"], bench_lib.DIAGNOSTIC)
        self.assertEqual(list(legacy["gates"]), ["schema"])

    def test_the_order_of_severity_and_the_exit_codes(self):
        self.assertEqual(bench_lib.worst([bench_lib.PASS, bench_lib.UNSTABLE]), bench_lib.UNSTABLE)
        self.assertEqual(bench_lib.worst([bench_lib.UNSTABLE, bench_lib.DIAGNOSTIC]), bench_lib.DIAGNOSTIC)
        self.assertEqual(bench_lib.worst([bench_lib.DIAGNOSTIC, bench_lib.FAIL, bench_lib.UNSTABLE]), bench_lib.FAIL)
        self.assertEqual(bench_lib.worst([]), bench_lib.PASS)
        self.assertEqual([bench_lib.exit_code(s) for s in (bench_lib.PASS, bench_lib.SKIPPED, bench_lib.FAIL, bench_lib.UNSTABLE, bench_lib.DIAGNOSTIC)],
                         [0, 0, 1, 3, 4])

    def test_command_line_exit_status_is_the_worst_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            good, slow = os.path.join(tmp, "good.json"), os.path.join(tmp, "slow.json")
            write_json(good, document())
            write_json(slow, document(governor="powersave"))
            self.assertEqual(bench_gate.main([good]), 0)
            self.assertEqual(bench_gate.main([good, slow]), 4)
            self.assertEqual(bench_gate.main([]), 2)
            self.assertEqual(bench_gate.main([os.path.join(tmp, "missing.json")]), 2)


if __name__ == "__main__":
    unittest.main()
