#!/usr/bin/env python3
"""Validates benchmark reports against the schema-3 contract of Milestone 2.5.4.1.

The contract is written in `crates/adb-bench/src/report.rs` (`validate`); this script implements
the same checks so reports can be checked without building anything, and a Rust test runs both on
the same document.

A report is one of:

* **schema 3** - has `"schema": 3`. It must have the required top-level keys; every `timing` object
  anywhere in it must have exactly the known keys, a valid `stability` and be internally
  consistent.
* **legacy** - no `schema` key (every report of 2.5.4 and earlier). It must be a JSON object with a
  `milestone` or `host` key. Legacy reports are accepted, but marked `legacy`: they carry no
  environment record, so comparisons against them are diagnostic only.

Usage:
    bench_validate.py [--require-schema] FILE...

Exit status: 0 all valid, 1 some invalid (or legacy with --require-schema), 2 usage or unreadable.
"""
import json
import sys

SCHEMA_VERSION = 3
REQUIRED_TOP_LEVEL = ["schema", "milestone", "family", "environment", "runtime_before", "runtime_after", "plan"]
REQUIRED_TIMING = [
    "median_of_medians_ms", "batch_medians_ms", "batch_mad_percent", "outlier_count", "outlier_indices",
    "stability", "batches", "samples_per_batch", "iterations_per_sample", "quick",
]
OPTIONAL_TIMING = ["stability_limit_percent"]
STABILITY_VALUES = ["stable", "unstable", "skipped"]


def check_timing(timing: dict, path: str, problems: list) -> None:
    """Appends to `problems` every violation of the `timing` contract found at `path`."""
    for key in REQUIRED_TIMING:
        if key not in timing:
            problems.append(f"{path}: timing lacks `{key}`")
    for key in timing:
        if key not in REQUIRED_TIMING and key not in OPTIONAL_TIMING:
            problems.append(f"{path}: unknown timing key `{key}`")
    stability = timing.get("stability")
    if isinstance(stability, str) and stability not in STABILITY_VALUES:
        problems.append(f"{path}: stability `{stability}` is not one of {STABILITY_VALUES}")
    count, indices = timing.get("outlier_count"), timing.get("outlier_indices")
    if isinstance(count, int) and isinstance(indices, list) and count != len(indices):
        problems.append(f"{path}: outlier_count {count} does not match {len(indices)} indices")
    batches, medians = timing.get("batches"), timing.get("batch_medians_ms")
    if isinstance(batches, int) and isinstance(medians, list) and batches != len(medians):
        problems.append(f"{path}: batches {batches} does not match {len(medians)} batch medians")
    for key in ("median_of_medians_ms", "batch_mad_percent"):
        value = timing.get(key)
        if isinstance(value, (int, float)) and not isinstance(value, bool) and not value >= 0:
            problems.append(f"{path}: `{key}` must be a finite number >= 0")


def walk_timings(value, path: str, problems: list) -> None:
    """Validates every `timing` object in `value`, however deeply nested."""
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "timing" and isinstance(child, dict):
                check_timing(child, f"{path}.{key}", problems)
            else:
                walk_timings(child, f"{path}.{key}", problems)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            walk_timings(child, f"{path}[{index}]", problems)


def validate(doc) -> list:
    """Returns every problem of a schema-3 document (an empty list means valid)."""
    if not isinstance(doc, dict):
        return ["the document is not a JSON object"]
    problems = []
    for key in REQUIRED_TOP_LEVEL:
        if key not in doc:
            problems.append(f"missing top-level key `{key}`")
    if doc.get("schema") != SCHEMA_VERSION:
        problems.append(f"`schema` must be {SCHEMA_VERSION}")
    cases = doc.get("cases")
    if cases is not None:
        if not isinstance(cases, dict):
            problems.append("`cases` is not an object")
        else:
            for name, case in cases.items():
                if not isinstance(case, dict):
                    problems.append(f"case `{name}` is not an object")
                elif not any(k in case for k in ("timing", "skipped_reason", "value")):
                    problems.append(f"case `{name}` has neither `timing`, `value` nor `skipped_reason`")
    walk_timings(doc, "$", problems)
    return problems


def classify(doc) -> str:
    """`schema3`, `legacy` or `invalid` for a parsed document."""
    if not isinstance(doc, dict):
        return "invalid"
    if "schema" in doc:
        return "schema3"
    return "legacy" if ("milestone" in doc or "host" in doc) else "invalid"


def main(argv) -> int:
    """Command-line entry point; returns the exit status."""
    args = [a for a in argv if not a.startswith("--")]
    require = "--require-schema" in argv
    if not args or any(a.startswith("--") and a != "--require-schema" for a in argv):
        print(__doc__)
        return 2
    status = 0
    for path in args:
        try:
            with open(path, encoding="utf-8") as handle:
                doc = json.load(handle)
        except (OSError, ValueError) as error:
            print(f"{path}: cannot read: {error}")
            return 2
        kind = classify(doc)
        if kind == "schema3":
            problems = validate(doc)
        elif kind == "legacy":
            problems = ["legacy report (no schema key), not accepted with --require-schema"] if require else []
        else:
            problems = ["not a benchmark report (no `schema`, `milestone` or `host`)"]
        label = "ok" if not problems else "INVALID"
        print(f"{path}: {label} ({kind})")
        for problem in problems:
            print(f"  {problem}")
        if problems:
            status = 1
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
