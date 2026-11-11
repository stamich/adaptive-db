#!/usr/bin/env python3
"""Compares two benchmark reports of the same family case by case.

``bench_compare.py old.json new.json [--limit PERCENT] [--out FILE]``. A case is ``slower`` or
``faster`` when its headline time moved by more than the limit (5 % until calibrated, provisional),
else ``same``. The *status* of the comparison is conservative:

* ``diagnostic`` when either report is a legacy (pre-2.5.4.1) report, or either was taken without the
  ``performance`` governor: the two numbers were not measured under comparable conditions, so the
  ratios are for looking at, never a verdict. Old reports always land here;
* ``unstable`` when either report has an unstable case that is being compared;
* ``fail`` when a deterministic value (checksum, rows, ...) differs;
* ``pass`` otherwise.

Comparing two reports taken at different times still carries machine drift; for a verdict use
``bench_ab.py``, which interleaves the runs. Exit codes follow ``bench_lib``.
"""
import argparse
import json
import sys

import bench_gate
import bench_lib
import bench_repeat

DEFAULT_LIMIT_PERCENT = 5.0


def compare(old: dict, new: dict, limit: float = DEFAULT_LIMIT_PERCENT):
    """Returns ``(document, status)`` comparing the cases of ``new`` with ``old``."""
    statuses, problems, cases = [], [], {}
    for label, doc in (("old", old), ("new", new)):
        verdict = bench_gate.evaluate(doc)
        if "schema" not in doc or verdict["gates"].get("governor", {}).get("status") == bench_lib.DIAGNOSTIC:
            statuses.append(bench_lib.DIAGNOSTIC)
            problems.append(f"{label} report is legacy or was measured without the performance governor")
    new_cases = bench_lib.cases(new)
    for name, old_case in bench_lib.cases(old).items():
        new_case = new_cases.get(name)
        if new_case is None:
            continue
        reference, other = bench_repeat.deterministic_of(old_case), bench_repeat.deterministic_of(new_case)
        if reference != other:
            statuses.append(bench_lib.FAIL)
            problems.append(f"case `{name}`: {other} differs from {reference}")
        a, _ = bench_lib.headline(old_case)
        b, _ = bench_lib.headline(new_case)
        if a is None or b is None or a == 0:
            continue
        change = (b / a - 1.0) * 100.0
        unstable = any((c.get("timing") or {}).get("stability") == "unstable" for c in (old_case, new_case))
        if unstable:
            statuses.append(bench_lib.UNSTABLE)
        verdict = "unstable" if unstable else "same" if abs(change) <= limit else "slower" if change > 0 else "faster"
        cases[name] = {"old_ms": a, "new_ms": b, "change_percent": change, "verdict": verdict}
    status = bench_lib.worst(statuses)
    return {"kind": "compare", "limit_percent": limit, "limit_is_provisional": True, "status": status,
            "problems": problems, "cases": cases}, status


def main(argv=None) -> int:
    """Command line."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("old")
    parser.add_argument("new")
    parser.add_argument("--limit", type=float, default=DEFAULT_LIMIT_PERCENT)
    parser.add_argument("--out")
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return 2 if exit_.code else 0
    try:
        document, status = compare(bench_lib.load(args.old), bench_lib.load(args.new), args.limit)
    except (OSError, ValueError) as error:
        print(f"bench_compare: {error}", file=sys.stderr)
        return 2
    if args.out:
        with open(args.out, "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
            handle.write("\n")
    for problem in document["problems"]:
        print(f"note: {problem}")
    for name, row in document["cases"].items():
        print(f"{name:40} {row['old_ms']:10.4f} -> {row['new_ms']:10.4f} ms {row['change_percent']:+7.1f}%  {row['verdict']}")
    print(f"comparison: {status}")
    return bench_lib.exit_code(status)


if __name__ == "__main__":
    sys.exit(main())
