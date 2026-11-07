#!/usr/bin/env python3
"""Renders benchmark reports as Markdown: environment, gate verdict and one row per case.

``bench_report.py report.json [more.json ...] [--out FILE]``. Schema-3 reports show the stability
verdict of every case; reports from before 2.5.4.1 are shown as *legacy* (their numbers are
diagnostic, see ``bench_gate``). The exit status is 0, or 2 on a usage or file error: rendering
never judges.
"""
import argparse
import sys

import bench_gate
import bench_lib


def environment_line(doc: dict) -> str:
    """One line about the machine: CPU, governor, storage."""
    env = doc.get("environment") or {}
    storage = doc.get("storage") or {}
    parts = [f"cpu {env.get('cpu_model') or 'unknown'}", f"governor {env.get('governor') or 'unknown'}"]
    if storage:
        parts.append(f"storage {storage.get('filesystem') or 'unknown'}"
                     + (" (memory backed)" if storage.get("memory_backed") else ""))
    return ", ".join(parts)


def render(doc: dict, name: str = "report") -> str:
    """The Markdown of one report."""
    verdict = bench_gate.evaluate(doc)
    legacy = "schema" not in doc
    lines = [f"## {name}: {doc.get('family', 'unknown family')} {doc.get('milestone', '')}".rstrip(),
             "", f"- gate status: **{verdict['status']}**" + (" (legacy report)" if legacy else ""),
             f"- {environment_line(doc)}", ""]
    rows = []
    for case_name, case in bench_lib.cases(doc).items():
        ms, has_timing = bench_lib.headline(case)
        if ms is None:
            continue
        timing = case.get("timing") or {}
        rows.append((case_name, ms, timing.get("batch_mad_percent"), timing.get("stability", "legacy"),
                     timing.get("outlier_count", "")))
    if rows:
        lines += ["| case | median ms | MAD % | stability | outliers |", "|---|---:|---:|---|---:|"]
        for case_name, ms, mad, stability, outliers in rows:
            mad_text = "" if mad is None else f"{mad:.2f}"
            lines.append(f"| {case_name} | {ms:.4f} | {mad_text} | {stability} | {outliers} |")
    else:
        lines.append("_no timed cases_")
    return "\n".join(lines) + "\n"


def main(argv=None) -> int:
    """Command line."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("reports", nargs="+")
    parser.add_argument("--out")
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return 2 if exit_.code else 0
    try:
        text = "\n".join(render(bench_lib.load(p), p) for p in args.reports)
    except (OSError, ValueError) as error:
        print(f"bench_report: {error}", file=sys.stderr)
        return 2
    if args.out:
        with open(args.out, "w", encoding="utf-8") as handle:
            handle.write(text)
    else:
        print(text, end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
