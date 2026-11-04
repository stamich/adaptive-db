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
