#!/usr/bin/env python3
"""Checks that the 2.3.0 workload benchmark reports agree: every path of workloads A-E returns
the same checksum, cost and rule mode return the same rows, fresh statistics estimate the plain
workloads well, stale statistics are flagged and repaired by ANALYZE, every index holds one
entry per row, an index read at an older snapshot returns the older rows, and the index and
sequential plans of the selectivity sweep return the same rows, with cost mode choosing the
index for the most selective range.

Usage: check-benchmarks.py DATABASE_JSON FFI_JSON
Exits with status 1 and one line per failure if anything disagrees. Timings are never checked:
they depend on the machine.
"""
import json
import sys


def main(database_path: str, ffi_path: str) -> int:
    """Runs every check; returns the process exit status."""
    with open(database_path) as f:
        native = json.load(f)
    with open(ffi_path) as f:
        ffi = json.load(f)
    failures = []

    def check(ok: bool, message: str) -> None:
        """Records `message` unless `ok`."""
        if not ok:
            failures.append(message)

    scale = native["setup"]["scale"]
    check(scale == ffi["setup"]["scale"], f"scale differs: {scale} vs {ffi['setup']['scale']}")
    for name, rust in native["workloads"].items():
        jvm = ffi["workloads"][name]
        expected = rust["checksum"]
        for path in ("ffi_prepared_plan", "scala_cbo_ffi_rust"):
            check(jvm[path]["checksum"] == expected,
                  f"{name}: {path} checksum {jvm[path]['checksum']} != rust_native {expected}")
        check((jvm["max_q_error"] or 0) <= 1.5, f"{name}: max q-error {jvm['max_q_error']} with fresh statistics")
    if scale == 1:
        expected_checksums = {
            "A_hash_join_topk": 9955,
            "B_hash_join_aggregate_topk": 54550,
            "C_covering_index_eq": 5742223708,
            "D_index_range_order_limit": 1052395880,
            "E_seq_scan_region_eq": 1000,
        }
        for name, expected in expected_checksums.items():
            check(native["workloads"][name]["checksum"] == expected, f"{name}: scale-1 checksum changed")
    for name, build in native["index_build"].items():
        check(build["entries"] == build["table_rows"], f"index {name}: {build['entries']} entries for {build['table_rows']} rows")
    as_of = native["as_of_index_read"]
    check(as_of["as_of_before_change"]["checksum"] == native["workloads"]["D_index_range_order_limit"]["checksum"],
          "AS OF index read does not return the rows before the change")
    check(as_of["latest"]["checksum"] != as_of["as_of_before_change"]["checksum"],
          "the change is not visible to the latest index read")
    for name in ("C_covering_index_eq", "D_index_range_order_limit"):
        check(ffi["workloads"][name]["access_path"].startswith("index_scan"),
              f"{name}: cost mode did not use the index ({ffi['workloads'][name]['access_path']})")
    sweep = {k: v for k, v in ffi["selectivity_sweep"].items() if k != "table_rows"}
    for name, point in sweep.items():
        check(point["index_plan"]["checksum"] == point["seq_plan"]["checksum"],
              f"selectivity {name}: index and sequential plans return different rows")
    if sweep:
        most_selective = min(sweep.values(), key=lambda p: p["fraction"])
        check(most_selective["cost_mode_choice"].startswith("index_scan"),
              f"cost mode chose {most_selective['cost_mode_choice']} for {most_selective['fraction']} of the rows")
    for name, modes in ffi["join_order"].items():
        cost, rule = modes["cost"], modes["rule"]
        check("error" not in cost, f"join order {name}: cost mode failed: {cost.get('error')}")
        if "error" not in cost and "error" not in rule:
            check(cost["ffi_prepared_plan"]["checksum"] == rule["ffi_prepared_plan"]["checksum"],
                  f"join order {name}: cost and rule mode return different rows")
            check(cost["estimated_root_cost"] <= rule["estimated_root_cost"],
                  f"join order {name}: cost mode chose a plan it estimates as more expensive")
    estimation = ffi["estimation"]
    check(estimation["stale_statistics"].get("stale_warning") is True, "stale statistics were not reported")
    check((estimation["after_analyze"].get("q_error") or 99) <= 1.5, "ANALYZE did not repair the stale estimate")
    check((estimation["zipf_most_common"].get("q_error") or 99) <= 1.5, "most common value badly estimated")

    for failure in failures:
        print(f"FAIL {failure}")
    if not failures:
        print(f"benchmark results agree (scale {scale})")
    return 1 if failures else 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    sys.exit(main(sys.argv[1], sys.argv[2]))
