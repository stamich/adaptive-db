"""Tests of scripts/bench_run.py with a fake command that replays canned reports."""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from util import read_json, read_text, write_json  # noqa: E402
import bench_lib  # noqa: E402
import bench_run  # noqa: E402
from test_bench_gate import document  # noqa: E402


class Replay:
    """A stand-in for subprocess.run that writes the next canned report to the `{out}` path."""

    def __init__(self, documents, returncode=0):
        self.documents, self.calls, self.returncode = list(documents), 0, returncode

    def __call__(self, argv, check=False):
        out = argv[-1]
        if self.returncode == 0:
            write_json(out, self.documents[min(self.calls, len(self.documents) - 1)])
        self.calls += 1
        return type("Done", (), {"returncode": self.returncode})()


class RunTest(unittest.TestCase):
    def go(self, replay, retries=2):
        with tempfile.TemporaryDirectory() as tmp:
            out = os.path.join(tmp, "r.json")
            code = bench_run.main(["--out", out, "--cmd", "fake {out}", "--retries", str(retries)], run=replay)
            gate = read_json(out + ".gate") if os.path.exists(out + ".gate") else None
            return code, gate

    def test_the_gate_file_is_not_matched_by_a_json_glob(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = os.path.join(tmp, "r.json")
            bench_run.main(["--out", out, "--cmd", "fake {out}"], run=Replay([document()]))
            self.assertEqual(sorted(os.listdir(tmp)), ["r.json", "r.json.gate"])

    def test_a_good_run_is_not_repeated(self):
        replay = Replay([document()])
        code, gate = self.go(replay)
        self.assertEqual((code, replay.calls, gate["attempts"]), (0, 1, ["pass"]))

    def test_unstable_is_remeasured_until_it_settles(self):
        replay = Replay([document(stability="unstable"), document(stability="unstable"), document()])
        code, gate = self.go(replay)
        self.assertEqual((code, replay.calls), (0, 3))
        self.assertEqual(gate["attempts"], ["unstable", "unstable", "pass"])

    def test_still_unstable_after_the_retries_exits_3(self):
        replay = Replay([document(stability="unstable")])
        code, _ = self.go(replay, retries=1)
        self.assertEqual((code, replay.calls), (3, 2))

    def test_diagnostic_and_fail_are_not_retried(self):
        replay = Replay([document(governor="powersave")])
        self.assertEqual(self.go(replay)[0], 4)
        self.assertEqual(replay.calls, 1)
        replay = Replay([document(filesystem="tmpfs")])
        self.assertEqual(self.go(replay)[0], 1)
        self.assertEqual(replay.calls, 1)

    def test_the_reason_of_a_failed_gate_is_printed(self):
        import contextlib
        import io
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            self.go(Replay([document(filesystem="tmpfs")]))
        self.assertIn("storage: fail:", buffer.getvalue())

    def test_a_failing_command_is_a_usage_level_error(self):
        self.assertEqual(self.go(Replay([document()], returncode=3))[0], 2)


if __name__ == "__main__":
    unittest.main()
