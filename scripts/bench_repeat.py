#!/usr/bin/env python3
"""Repeats a benchmark family in fresh processes and condenses the runs into one document.

One run can be unlucky (a noisy neighbour, a cold cache, a frequency dip). ``bench_repeat.py`` runs a
family N times as separate processes, then writes a *median document*: the first run's report, whose
timed cases carry a ``repeat`` object next to ``timing``::

    "repeat": {"runs": 3, "values_ms": [..], "median_ms": .., "spread_percent": .., "stability": ".."}

and whose top level carries ``repeat`` with the run count, the limit and the overall verdict.

Two gates apply across the runs:

* **deterministic values** (``checksum``, ``rows_out``, ``rows``, ``entries``, ``table_rows``,
  ``expected_changes``, ``changed_rows``, and ``value`` of untimed cases) must be *identical* in every
  run: a difference is a correctness bug, never noise -> status ``fail`` (exit 1);
* the **spread** of a case's per-run medians, ``(max - min) / median`` in percent, must stay within
  ``--limit`` (provisional, 10 % until calibrated on the reference machine) -> otherwise ``unstable``
  (exit 3).

Usage::

    bench_repeat.py --runs 3 --out median.json --cmd "target/release/rust-benchmark --out {out}"
    bench_repeat.py --out median.json --inputs run1.json run2.json run3.json
"""
import argparse
import json
import os
import shlex
import statistics
import subprocess
import sys
import tempfile

import bench_lib

#: Provisional limit of the run-to-run spread in percent; calibrated in task 24 on the reference machine.
DEFAULT_LIMIT_PERCENT = 10.0

#: Keys whose value is a pure function of the data: any difference between runs is a bug.
DETERMINISTIC_KEYS = (
    "checksum", "rows_out", "rows", "entries", "table_rows", "expected_changes", "changed_rows",
)


def median(values):
    """The median of a non-empty list."""
    return statistics.median(values)


def spread_percent(values) -> float:
    """``(max - min) / median`` in percent; 0 when the median is 0."""
    m = median(values)
    return 0.0 if m == 0 else (max(values) - min(values)) / abs(m) * 100.0


def case_value(case):
    """The headline time of a case in milliseconds, or None for an untimed case."""
    timing = case.get("timing") if isinstance(case, dict) else None
    if isinstance(timing, dict) and isinstance(timing.get("median_of_medians_ms"), (int, float)):
        return float(timing["median_of_medians_ms"])
    return None


def deterministic_of(case) -> dict:
    """The deterministic values of a case: the named keys, plus ``value`` when untimed."""
    if not isinstance(case, dict):
        return {}
    found = {k: case[k] for k in DETERMINISTIC_KEYS if k in case}
    if "value" in case and case_value(case) is None:
        found["value"] = case["value"]
    return found


def condense(documents, limit=DEFAULT_LIMIT_PERCENT):
    """Builds the median document from ``documents`` (the parsed reports of the runs).

    Returns ``(document, problems, status)``; ``problems`` lists the failed deterministic checks.
    """
    if not documents:
        raise ValueError("no runs to condense")
    result = json.loads(json.dumps(documents[0]))
    problems, statuses = [], []
    names = list(result.get("cases", {}))
    for doc in documents[1:]:
        if list(doc.get("cases", {})) != names:
            problems.append("the runs disagree on the set of cases")
    for name in names:
        runs = [d.get("cases", {}).get(name) for d in documents]
        reference = deterministic_of(runs[0])
        for index, run in enumerate(runs[1:], start=2):
            other = deterministic_of(run)
            if other != reference:
                problems.append(f"case `{name}`: run {index} {other} differs from run 1 {reference}")
        values = [case_value(r) for r in runs]
        if any(v is None for v in values):
            continue
        spread = spread_percent(values)
        stability = "stable" if spread <= limit else "unstable"
        statuses.append(bench_lib.PASS if stability == "stable" else bench_lib.UNSTABLE)
        result["cases"][name]["repeat"] = {
            "runs": len(values), "values_ms": values, "median_ms": median(values),
            "spread_percent": spread, "stability": stability,
        }
    if problems:
        statuses.append(bench_lib.FAIL)
    status = bench_lib.worst(statuses)
    result["repeat"] = {
        "runs": len(documents), "limit_percent": limit, "limit_is_provisional": True,
        "status": status, "problems": problems,
    }
    return result, problems, status


def run_command(template, runs, workdir):
    """Runs ``template`` (with ``{out}``) ``runs`` times; returns the report paths."""
    paths = []
    for index in range(1, runs + 1):
        out = os.path.join(workdir, f"run{index}.json")
        argv = [part.replace("{out}", out) for part in shlex.split(template)]
        completed = subprocess.run(argv, check=False)
        if completed.returncode != 0 or not os.path.exists(out):
            raise RuntimeError(f"run {index} failed (exit {completed.returncode}): {' '.join(argv)}")
        paths.append(out)
    return paths


def main(argv=None) -> int:
    """Command line; returns the exit code (0, 1, 2 usage, 3, 4 never: the machine gate is bench_gate)."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--limit", type=float, default=DEFAULT_LIMIT_PERCENT)
    parser.add_argument("--out", required=True)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--cmd")
    source.add_argument("--inputs", nargs="+")
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return 2 if exit_.code else 0
    if args.runs < 1 or args.limit < 0:
        print("--runs must be at least 1 and --limit non-negative", file=sys.stderr)
        return 2
    try:
        with tempfile.TemporaryDirectory() as tmp:
            paths = args.inputs or run_command(args.cmd, args.runs, tmp)
            documents = [bench_lib.load(p) for p in paths]
    except (OSError, ValueError, RuntimeError) as error:
        print(f"bench_repeat: {error}", file=sys.stderr)
        return 2
    document, problems, status = condense(documents, args.limit)
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(document, handle, indent=2)
        handle.write("\n")
    for problem in problems:
        print(f"FAIL {problem}")
    print(f"{len(documents)} runs -> {args.out}: {status}")
    return bench_lib.exit_code(status)


if __name__ == "__main__":
    sys.exit(main())
