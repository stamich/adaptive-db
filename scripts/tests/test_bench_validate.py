"""Tests of scripts/bench_validate.py (run: python3 -m unittest discover -s scripts/tests)."""
import copy
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import bench_validate  # noqa: E402

TIMING = {
    "median_of_medians_ms": 1.0, "batch_medians_ms": [1.0, 1.1], "batch_mad_percent": 2.0,
    "outlier_count": 0, "outlier_indices": [], "stability": "stable", "batches": 2,
    "samples_per_batch": 5, "iterations_per_sample": 1, "quick": False,
}


def good_document() -> dict:
    """A minimal valid schema-3 document."""
    return {
        "schema": 3, "milestone": "2.5.4.1", "family": "t", "environment": {}, "runtime_before": {},
        "runtime_after": {}, "plan": {}, "cases": {"scan": {"p50_ms": 1.0, "timing": copy.deepcopy(TIMING)}},
    }


class ValidateTest(unittest.TestCase):
    def test_a_good_document_is_valid(self):
        self.assertEqual(bench_validate.validate(good_document()), [])

    def test_missing_and_unknown_keys_are_reported(self):
        doc = good_document()
        del doc["plan"]
        del doc["cases"]["scan"]["timing"]["quick"]
        doc["cases"]["scan"]["timing"]["extra"] = 1
        problems = bench_validate.validate(doc)
        self.assertTrue(any("missing top-level key `plan`" in p for p in problems))
        self.assertTrue(any("timing lacks `quick`" in p for p in problems))
        self.assertTrue(any("unknown timing key `extra`" in p for p in problems))

    def test_inconsistent_timing_is_reported(self):
        doc = good_document()
        timing = doc["cases"]["scan"]["timing"]
        timing.update(stability="great", outlier_count=2, batches=7, median_of_medians_ms=-1.0)
        problems = " | ".join(bench_validate.validate(doc))
        for needle in ("stability `great`", "outlier_count 2", "batches 7", "must be a finite number"):
            self.assertIn(needle, problems)

    def test_timing_objects_are_found_in_a_legacy_layout(self):
        doc = good_document()
        doc["workloads"] = {"A": [{"timing": {"stability": "stable"}}]}
        self.assertTrue(any("$.workloads.A[0].timing" in p for p in bench_validate.validate(doc)))

    def test_classification(self):
        self.assertEqual(bench_validate.classify(good_document()), "schema3")
        self.assertEqual(bench_validate.classify({"milestone": "2.5.4"}), "legacy")
        self.assertEqual(bench_validate.classify({"host": {}}), "legacy")
        self.assertEqual(bench_validate.classify({"x": 1}), "invalid")
        self.assertEqual(bench_validate.classify([1]), "invalid")

    def test_command_line_exit_codes(self):
        with tempfile.TemporaryDirectory() as tmp:
            good, legacy, bad = (os.path.join(tmp, n) for n in ("good.json", "legacy.json", "bad.json"))
            json.dump(good_document(), open(good, "w"))
            json.dump({"milestone": "2.5.4"}, open(legacy, "w"))
            broken = good_document()
            del broken["plan"]
            json.dump(broken, open(bad, "w"))
            self.assertEqual(bench_validate.main([good, legacy]), 0)
            self.assertEqual(bench_validate.main([good, legacy, "--require-schema"]), 1)
            self.assertEqual(bench_validate.main([bad]), 1)
            self.assertEqual(bench_validate.main([os.path.join(tmp, "missing.json")]), 2)
            self.assertEqual(bench_validate.main([]), 2)


if __name__ == "__main__":
    unittest.main()
