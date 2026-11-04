#!/usr/bin/env python3
"""Evaluates the gates of benchmark reports and reports a status per gate.

Gates:

* **governor** - ``pass`` when the CPU governor of the machine was ``performance``; ``diagnostic`` when
  it was anything else or could not be read. On the reference machine the same code wrote 5.1k or
  42.0k rows/s depending on the governor, so a run under another governor is never a pass.
* **storage** - for durable families (``rust-local``, ``workloads``, ``scaling``): ``fail`` when the
  data directory is on a RAM-backed filesystem (``tmpfs``, ``ramfs``), where ``fsync`` measures
  nothing; ``skipped`` when the report has no storage record or the family is not durable.
* **stability** - ``unstable`` when a case's batch MAD is beyond its limit, ``skipped`` for quick plans
  (they do not judge), ``pass`` otherwise. The limit is the one written in the report, which is the
  provisional ``DEFAULT_STABLE_BATCH_MAD_PERCENT`` until it is calibrated on the reference machine.
* **jit** - for JVM reports: ``unstable`` when a case was still being compiled while it was measured
  (``jit.warm`` is false); ``skipped`` for quick plans.
* **schema** - ``fail`` when ``bench_validate`` finds a problem.

Usage:
    bench_gate.py [--json] FILE...

Prints one line per gate and file. Exit status: 0 pass/skipped, 1 fail, 3 unstable, 4 diagnostic (see
``bench_lib`` for the order when several apply).
"""
import json
import sys

import bench_lib
import bench_validate

MEMORY_BACKED = {"tmpfs", "ramfs", "devtmpfs"}


def governor_gate(doc: dict) -> dict:
    """The governor gate of a report."""
    governor = (doc.get("environment") or {}).get("governor")
    if governor == "performance":
        return {"status": bench_lib.PASS, "reason": "governor is performance"}
    if not governor:
        return {"status": bench_lib.DIAGNOSTIC, "reason": "governor unknown (a VM, a container or a non-Linux system)"}
    return {"status": bench_lib.DIAGNOSTIC, "reason": f"governor is {governor}, not performance"}


def storage_gate(doc: dict) -> dict:
    """The storage gate of a report."""
    if doc.get("family") not in bench_lib.DURABLE_FAMILIES:
        return {"status": bench_lib.SKIPPED, "reason": "family is not durable"}
    storage = doc.get("storage")
    if not isinstance(storage, dict):
        return {"status": bench_lib.SKIPPED, "reason": "no storage record"}
    if storage.get("memory_backed") or storage.get("filesystem") in MEMORY_BACKED:
        return {"status": bench_lib.FAIL,
                "reason": f"data directory is on {storage.get('filesystem') or 'a RAM-backed filesystem'}: fsync measures nothing"}
    return {"status": bench_lib.PASS, "reason": f"data directory is on {storage.get('filesystem') or 'unknown'}"}


def stability_gate(doc: dict) -> dict:
    """The stability gate: the worst verdict of any case."""
    verdicts = [timing.get("stability") for _, timing, _ in bench_lib.timings(doc)]
    if not verdicts:
        return {"status": bench_lib.SKIPPED, "reason": "no timed case"}
    unstable = [path for path, timing, _ in bench_lib.timings(doc) if timing.get("stability") == "unstable"]
    if unstable:
        return {"status": bench_lib.UNSTABLE, "reason": f"{len(unstable)} unstable case(s), first {unstable[0]}"}
    if all(v == "skipped" for v in verdicts):
        return {"status": bench_lib.SKIPPED, "reason": "quick plan: stability is not judged"}
    return {"status": bench_lib.PASS, "reason": f"{len(verdicts)} case(s) stable"}


def jit_gate(doc: dict) -> dict:
    """The JIT gate (JVM reports): cases measured while the compiler was still working."""
    records = []
    for path, timing, parent in bench_lib.timings(doc):
        jit = parent.get("jit")
        if isinstance(jit, dict):
            records.append((path, jit, timing))
    if not records:
        return {"status": bench_lib.SKIPPED, "reason": "no JIT record"}
    if all(timing.get("quick") for _, _, timing in records):
        return {"status": bench_lib.SKIPPED, "reason": "quick plan: warm-up is not judged"}
    cold = [path for path, jit, timing in records if not timing.get("quick") and not jit.get("warm")]
    if cold:
        return {"status": bench_lib.UNSTABLE, "reason": f"{len(cold)} case(s) measured while compiling, first {cold[0]}"}
    return {"status": bench_lib.PASS, "reason": f"{len(records)} case(s) warm"}


def schema_gate(doc) -> dict:
    """The schema gate: ``bench_validate`` found nothing."""
    if bench_validate.classify(doc) != "schema3":
        return {"status": bench_lib.DIAGNOSTIC, "reason": "legacy report: no environment record, comparisons are diagnostic"}
    problems = bench_validate.validate(doc)
    if problems:
        return {"status": bench_lib.FAIL, "reason": f"{len(problems)} schema problem(s), first: {problems[0]}"}
    return {"status": bench_lib.PASS, "reason": "valid schema 3"}


def evaluate(doc) -> dict:
    """All gates of one report and the overall status."""
    gates = {"schema": schema_gate(doc)}
    if isinstance(doc, dict) and bench_validate.classify(doc) == "schema3":
        gates["governor"] = governor_gate(doc)
        gates["storage"] = storage_gate(doc)
        gates["stability"] = stability_gate(doc)
        gates["jit"] = jit_gate(doc)
    overall = bench_lib.worst(g["status"] for g in gates.values())
    return {"status": overall, "gates": gates}


def main(argv) -> int:
    """Command-line entry point; returns the exit status."""
    as_json = "--json" in argv
    paths = [a for a in argv if not a.startswith("--")]
    if not paths or any(a.startswith("--") and a != "--json" for a in argv):
        print(__doc__)
        return 2
    results = {}
    for path in paths:
        try:
            results[path] = evaluate(bench_lib.load(path))
        except (OSError, ValueError) as error:
            print(f"{path}: cannot read: {error}")
            return 2
    if as_json:
        print(json.dumps(results, indent=2))
    else:
        for path, result in results.items():
            print(f"{path}: {result['status']}")
            for name, gate in result["gates"].items():
                print(f"  {name:10} {gate['status']:10} {gate['reason']}")
    return bench_lib.exit_code(bench_lib.worst(r["status"] for r in results.values()))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
