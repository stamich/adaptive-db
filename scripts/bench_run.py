#!/usr/bin/env python3
"""Runs one benchmark family, gates the report and measures again while it is unstable.

``bench_run.py --out report.json --cmd "<command with {out}>"`` runs the command, evaluates the report
with ``bench_gate`` and, when the verdict is ``unstable`` (the numbers moved more than the limit
allows, a noisy moment rather than a machine fault), runs the family again, up to ``--retries``
times. The last report is kept; its gate result is written next to it as ``<out>.gate`` (JSON, but not named *.json, so globs over reports skip it) with the
list of attempts. Only ``unstable`` is retried: ``diagnostic`` (a machine that gives no comparable
numbers) and ``fail`` (a wrong value, a refused storage) do not improve by measuring again.

Exit codes: 0 pass or skipped, 1 fail, 2 usage or the command failed, 3 still unstable after the
retries, 4 diagnostic.
"""
import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile

import bench_gate
import bench_lib


def attempt(template: str, out: str, run=subprocess.run) -> dict:
    """Runs the command once, writing the report to ``out``; returns the gate verdict."""
    argv = [part.replace("{out}", out) for part in shlex.split(template)]
    completed = run(argv, check=False)
    if completed.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f"command failed (exit {completed.returncode}): {' '.join(argv)}")
    return bench_gate.evaluate(bench_lib.load(out))


def run_gated(template: str, out: str, retries: int, run=subprocess.run):
    """Runs and re-runs while unstable. Returns ``(final_verdict, statuses_of_all_attempts)``."""
    statuses, verdict = [], None
    with tempfile.TemporaryDirectory() as tmp:
        for index in range(retries + 1):
            candidate = os.path.join(tmp, f"attempt{index}.json")
            verdict = attempt(template, candidate, run)
            statuses.append(verdict["status"])
            shutil.copyfile(candidate, out)
            if verdict["status"] != bench_lib.UNSTABLE:
                break
    return verdict, statuses


def main(argv=None, run=subprocess.run) -> int:
    """Command line."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--out", required=True)
    parser.add_argument("--cmd", required=True)
    parser.add_argument("--retries", type=int, default=2)
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return 2 if exit_.code else 0
    if args.retries < 0:
        print("--retries must not be negative", file=sys.stderr)
        return 2
    try:
        verdict, statuses = run_gated(args.cmd, args.out, args.retries, run)
    except (OSError, ValueError, RuntimeError) as error:
        print(f"bench_run: {error}", file=sys.stderr)
        return 2
    verdict = dict(verdict, attempts=statuses)
    with open(args.out + ".gate", "w", encoding="utf-8") as handle:
        json.dump(verdict, handle, indent=2)
        handle.write("\n")
    if verdict["status"] not in (bench_lib.PASS, bench_lib.SKIPPED):
        for name, gate in verdict["gates"].items():
            if gate["status"] not in (bench_lib.PASS, bench_lib.SKIPPED):
                print(f"  {name}: {gate['status']}: {gate.get('reason', '')}")
    print(f"{args.out}: {verdict['status']} after {len(statuses)} attempt(s) {statuses}")
    return bench_lib.exit_code(verdict["status"])


if __name__ == "__main__":
    sys.exit(main())
