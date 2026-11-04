#!/usr/bin/env python3
"""Shared pieces of the Milestone 2.5.4.1 benchmark tools: gate statuses, exit codes, and walking
the `timing` objects of a schema-3 report.

Gate statuses, from best to worst for a reader who wants to trust a number:

* ``pass``        - checked and good.
* ``skipped``     - not checked (a quick run, or nothing to check); never makes a run fail.
* ``unstable``    - the numbers moved more than the limit allows: measure again.
* ``diagnostic``  - the machine was not in a state that gives comparable numbers (CPU governor is not
  ``performance`` or unknown): the numbers are for looking at, never a pass.
* ``fail``        - something is wrong: a deterministic value differed, the storage was refused, a
  report broke the schema.

Exit codes of the tools: 0 pass (or skipped), 1 fail, 3 unstable, 4 diagnostic. When several apply the
order is fail (1), then diagnostic (4), then unstable (3): a failed check outranks everything, and a
machine that gives no comparable numbers makes questions about their stability moot.
"""
import json

PASS, SKIPPED, UNSTABLE, DIAGNOSTIC, FAIL = "pass", "skipped", "unstable", "diagnostic", "fail"

EXIT_CODES = {PASS: 0, SKIPPED: 0, FAIL: 1, UNSTABLE: 3, DIAGNOSTIC: 4}

# Worst status wins; this is the order in which statuses outrank each other.
_SEVERITY = {PASS: 0, SKIPPED: 0, UNSTABLE: 1, DIAGNOSTIC: 2, FAIL: 3}

# Families that write to disk with fsync: their numbers mean nothing on a RAM disk.
DURABLE_FAMILIES = {"rust-local", "workloads", "scaling"}


def worst(statuses) -> str:
    """The status that outranks the others (``pass`` for an empty list)."""
    result = PASS
    for status in statuses:
        if _SEVERITY[status] > _SEVERITY[result]:
            result = status
    return result


def exit_code(status: str) -> int:
    """The process exit code of a status."""
    return EXIT_CODES[status]


def load(path: str):
    """Reads a JSON report."""
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def timings(value, path="$"):
    """Yields ``(json_path, timing_object, parent_object)`` for every ``timing`` object in ``value``."""
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "timing" and isinstance(child, dict):
                yield f"{path}.{key}", child, value
            else:
                yield from timings(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from timings(child, f"{path}[{index}]")


def headline(case):
    """``(milliseconds, has_timing)`` of a case: the median of medians of a schema-3 ``timing`` object,
    else the legacy ``p50_ms`` (``has_timing`` False), else ``(None, False)`` for an untimed case."""
    if not isinstance(case, dict):
        return None, False
    timing = case.get("timing")
    if isinstance(timing, dict) and isinstance(timing.get("median_of_medians_ms"), (int, float)):
        return float(timing["median_of_medians_ms"]), True
    if isinstance(case.get("p50_ms"), (int, float)):
        return float(case["p50_ms"]), False
    return None, False


#: Top-level keys that describe the run, not a measurement.
_NOT_CASES = {"environment", "storage", "plan", "plan_durable", "runtime_before", "runtime_after",
              "fsync_probe", "host", "repeat", "setup"}


def cases(doc) -> dict:
    """Every case of a report, by name, as references into ``doc``.

    A case is an entry of the top-level ``cases`` object (timed or not) or, wherever else it sits, an
    object with a ``timing`` object or a legacy ``p50_ms``: the rust-local family keeps its 2.5.4
    layout (``entity_scan``, ``relational.hash_join``, ``commit_single_row.one_thread``, ...) next to
    the schema-3 ``timing``. Names of nested cases are dotted paths.
    """
    found = {}
    if isinstance(doc, dict) and isinstance(doc.get("cases"), dict):
        for name, case in doc["cases"].items():
            found[name] = case

    def walk(value, prefix):
        if not isinstance(value, dict):
            return
        for key, child in value.items():
            if not prefix and (key in _NOT_CASES or key == "cases"):
                continue
            name = f"{prefix}.{key}" if prefix else key
            if isinstance(child, dict) and headline(child)[0] is not None:
                found[name] = child
            else:
                walk(child, name)

    walk(doc, "")
    return found
