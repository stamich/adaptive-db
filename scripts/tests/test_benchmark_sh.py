"""Tests of the argument handling of scripts/benchmark.sh and of the committed reports."""
import glob
import os
import subprocess
import sys
import unittest

ROOT = os.path.join(os.path.dirname(__file__), "..", "..")
sys.path.insert(0, os.path.join(ROOT, "scripts"))
import bench_validate  # noqa: E402
import bench_lib  # noqa: E402

SCRIPT = os.path.join(ROOT, "scripts", "benchmark.sh")


def launch(*args):
    """Runs benchmark.sh and returns the completed process."""
    return subprocess.run(["bash", SCRIPT, *args], capture_output=True, text=True, check=False)


class LauncherTest(unittest.TestCase):
    def test_no_arguments_print_the_usage(self):
        done = launch()
        self.assertEqual(done.returncode, 2)
        self.assertIn("scripts/benchmark.sh FAMILY|all", done.stdout)

    def test_an_unknown_family_or_option_is_refused(self):
        self.assertEqual(launch("bogus").returncode, 2)
        self.assertEqual(launch("memory", "--nope").returncode, 2)

    def test_repeat_must_be_a_positive_number(self):
        for bad in ("0", "x", "-1"):
            self.assertEqual(launch("memory", "--repeat", bad).returncode, 2, bad)


class CommittedReportsTest(unittest.TestCase):
    def test_every_committed_report_validates(self):
        paths = glob.glob(os.path.join(ROOT, "examples", "results", "**", "*.json"), recursive=True)
        self.assertTrue(paths)
        for path in paths:
            document = bench_lib.load(path)
            kind = bench_validate.classify(document)
            if kind == "schema3":
                self.assertEqual(bench_validate.validate(document), [], path)
            else:
                self.assertEqual(kind, "legacy", path)


if __name__ == "__main__":
    unittest.main()
