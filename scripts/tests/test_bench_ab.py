"""Tests of scripts/bench_ab.py: pure parts and the worktree overlay on a throwaway repository."""
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import bench_ab  # noqa: E402
import bench_lib  # noqa: E402


def run(ms, checksum=1):
    """A canned report with one timed case."""
    return {"cases": {"scan": {"checksum": checksum, "timing": {"median_of_medians_ms": ms}}}}


def runs(*values, checksum=1):
    """Several canned reports."""
    return [run(v, checksum) for v in values]


class CompareTest(unittest.TestCase):
    def test_identical_builds_are_the_same(self):
        doc, status = bench_ab.compare(runs(10, 10), runs(10.1, 10.1), runs(10.05, 10.05))
        self.assertEqual(doc["cases"]["scan"]["verdict"], "same")
        self.assertEqual(status, bench_lib.PASS)

    def test_a_real_change_is_reported_in_both_directions(self):
        slower, _ = bench_ab.compare(runs(10), runs(12), runs(10.1))
        faster, _ = bench_ab.compare(runs(10), runs(8), runs(10.1))
        self.assertEqual(slower["cases"]["scan"]["verdict"], "slower")
        self.assertEqual(faster["cases"]["scan"]["verdict"], "faster")
        self.assertAlmostEqual(slower["cases"]["scan"]["change_percent"], 20.0)

    def test_a_noisy_control_makes_the_comparison_unstable(self):
        doc, status = bench_ab.compare(runs(10), runs(12), runs(11.5))
        self.assertEqual(doc["cases"]["scan"]["verdict"], "unstable")
        self.assertEqual(status, bench_lib.UNSTABLE)

    def test_different_deterministic_values_fail(self):
        doc, status = bench_ab.compare(runs(10), runs(10, checksum=2), runs(10))
        self.assertEqual(status, bench_lib.FAIL)
        self.assertTrue(doc["problems"])

    def test_the_order_rotates(self):
        self.assertEqual(bench_ab.order(0), ["A", "B", "A2"])
        self.assertEqual(bench_ab.order(1), ["B", "A2", "A"])
        self.assertEqual(bench_ab.order(3), bench_ab.order(0))
        self.assertEqual(len({bench_ab.order(i)[0] for i in range(3)}), 3)


class WorkspaceTest(unittest.TestCase):
    OLD = '[workspace]\nmembers = [\n    "crates/adb-core",\n    "examples/rust-benchmark"\n]\nresolver = "2"\n'

    def test_missing_members_are_added_once(self):
        patched = bench_ab.patch_workspace(self.OLD)
        self.assertIn('"crates/adb-bench"', patched)
        self.assertEqual(patched.count('"examples/rust-benchmark"'), 1)
        self.assertEqual(bench_ab.patch_workspace(patched), patched)
        self.assertIn('resolver = "2"', patched)

    def test_a_manifest_without_members_is_refused(self):
        with self.assertRaises(ValueError):
            bench_ab.patch_workspace("[package]\nname = \"x\"\n")

    def test_the_overlay_replaces_the_harness_in_a_worktree(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = os.path.join(tmp, "repo")
            os.makedirs(os.path.join(repo, "examples/rust-benchmark"))
            with open(os.path.join(repo, "Cargo.toml"), "w") as f:
                f.write(self.OLD)
            with open(os.path.join(repo, "examples/rust-benchmark/old.rs"), "w") as f:
                f.write("old")
            env = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t",
                   "GIT_COMMITTER_EMAIL": "t@t", "PATH": os.environ["PATH"], "HOME": tmp}
            for cmd in (["init", "-q"], ["add", "-A"], ["commit", "-qm", "base"], ["tag", "base"]):
                subprocess.run(["git", "-C", repo, *cmd], check=True, env=env)
            os.makedirs(os.path.join(repo, "crates/adb-bench"))
            with open(os.path.join(repo, "crates/adb-bench/lib.rs"), "w") as f:
                f.write("new")
            with open(os.path.join(repo, "examples/rust-benchmark/old.rs"), "w") as f:
                f.write("new harness")
            work = os.path.join(tmp, "work")
            os.makedirs(work)
            tree = bench_ab.build_baseline(repo, "base", work, run=lambda a, **k: subprocess.run(a, env=env, **k))
            self.assertEqual(open(os.path.join(tree, "examples/rust-benchmark/old.rs")).read(), "new harness")
            self.assertTrue(os.path.exists(os.path.join(tree, "crates/adb-bench/lib.rs")))
            self.assertIn('"crates/adb-bench"', open(os.path.join(tree, "Cargo.toml")).read())


if __name__ == "__main__":
    unittest.main()
