#!/usr/bin/env python3
"""Checks that the 2.4.0 workload benchmark reports agree: every path of workloads A-E returns
the same checksum, cost and rule mode return the same rows, fresh statistics estimate the plain
workloads well, stale statistics are flagged and repaired by ANALYZE, every index holds one
entry per row, an index read at an older snapshot returns the older rows, and the index and
sequential plans of the selectivity sweep return the same rows, with cost mode choosing the
index for the most selective range. The schema-evolution section (2.4.0) must return the same
rows whatever share of rows is migrated, fill in defaults on read, refuse every stale plan and
accept every current one, and write an ALTER record whose size does not depend on the table.
The consistency section (2.5.0) must stream every change exactly once, leave a caught-up
projection with no lag and no timed-out wait, rebuild the same change-log head after the log is
deleted, and prune consumed segments without overtaking the consumer while a byte limit against
a slow consumer overtakes it and marks it as needing a rebuild.
The telemetry section (2.5.2.1) holds the allocation of a skipped and of a captured query to a
budget (allocation, unlike time, does not depend on the load of the machine) and requires a
captured query to report the rows read.

Usage: check-benchmarks.py DATABASE_JSON FFI_JSON
Exits with status 1 and one line per failure if anything disagrees. Timings are never checked:
they depend on the machine.
"""
import json
import sys


def check_consistency(native: dict, check) -> None:
    """Checks the 2.5.0 consistency section of the native report with `check(ok, message)`."""
    section = native.get("consistency")
    check(section is not None, "the native report has no consistency section")
    if section is None:
        return
    commits = section["setup"]["commits"]
    work = section["workloads"]
    for name in ("one_row_per_commit", "batched"):
        check(work["I_commit_with_change_log"][name]["commits_per_s"] > 0, f"I {name}: no commit throughput")
    stream = work["J_change_log_stream"]
    check(stream["commits"] == commits, f"J: streamed {stream['commits']} of {commits} commits")
    check(stream["checksum"] == stream["expected_changes"], "J: the stream saw a different number of changes than were written")
    starts = work["K_cursor_start"]
    check(starts["start"]["first_commit_seq"] == 1, "K: a cursor at the start does not begin at commit 1")
    check(starts["end"]["first_commit_seq"] == commits, "K: a cursor at the end does not deliver the last commit")
    check(starts["segments"] > 1, "K: the log did not roll over, so the seek was not measured across segments")
    wait = work["L_projection_wait_latency"]
    check(wait["timed_out"] == 0, f"L: {wait['timed_out']} waits timed out")
    catch_up = work["M_projection_catch_up"]
    check(catch_up["applied"] == catch_up["backlog_commits"], "M: the consumer did not apply the whole backlog")
    check(catch_up["lag_after"] == 0, f"M: lag {catch_up['lag_after']} after catching up")
    recovery = work["N_recovery_tail_rebuild"]
    check(recovery["intact_log_head"] == recovery["head"], "N: the intact change log head differs from the database head")
    check(recovery["rebuilt_log_head"] == recovery["head"], "N: the rebuilt change log head differs from the database head")
    caught_up = work["O_retention_prune"]["consumer_caught_up"]
    check(not caught_up["overtook_frontier"], "O: pruning overtook a consumer that had read everything")
    check(caught_up["segments_after"] < caught_up["segments_before"], "O: consumed segments were not removed")
    slow = work["O_retention_prune"]["slow_consumer_byte_limit"]
    check(slow["overtook_frontier"], "O: the byte limit did not overtake the slow consumer")
    check(slow["consumer_state"] == "NeedsRebuild", f"O: the overtaken consumer is {slow['consumer_state']}")


def check_telemetry(ffi: dict, check) -> None:
    """Checks the 2.5.2.1 telemetry section of the JVM report with `check(ok, message)`.

    Only allocation is held to a budget, because it does not depend on the load of the machine:
    a query the sampler skips allocates no more than one that telemetry ignores, and a captured
    query adds a few hundred bytes (its event), not the kilobyte of a decoded profile document.
    The rows read a capture reports must be those the engine counted.
    """
    section = ffi.get("telemetry")
    check(section is not None, "the JVM report has no telemetry section")
    if section is None:
        return
    check(section["alloc_extra_skip"] <= 64, f"telemetry: a skipped query allocates {section['alloc_extra_skip']:.0f} extra bytes (budget 64)")
    check(section["alloc_extra_capture"] <= 512,
          f"telemetry: a captured query allocates {section['alloc_extra_capture']:.0f} extra bytes (budget 512)")
    check(section["captured_rows_read_per_execution"] > 0, "telemetry: a captured query reports no rows read")


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
    schema = native.get("schema_evolution")
    check(schema is not None, "the native report has no schema_evolution section")
    if schema is not None:
        scans = schema["workloads"]["G_schema_adapted_scan"]
        checksums = {name: scan["checksum"] for name, scan in scans.items()}
        check(len(set(checksums.values())) == 1, f"adapted scans disagree: {checksums}")
        point = schema["workloads"]["F_virtual_default_point_read"]
        check(point["old_row_currency"] == "EUR", f"old row reads {point['old_row_currency']}, not the default")
        check(point["old_row"]["checksum"] == point["migrated_row"]["checksum"],
              "old and migrated rows read different values")
        stale = schema["workloads"]["H_stale_schema_epoch_reject"]
        check(stale["rejected"]["refused"] == stale["rejected"]["iterations"], "a stale plan was executed")
        check(stale["accepted_same_plan"]["refused"] == 0, "a current plan was refused")
        alter = schema["alter_latency"]
        check(alter["large"]["log_bytes"] <= alter["small"]["log_bytes"] + 64,
              f"ALTER of the large table logged {alter['large']['log_bytes']} bytes, the small one {alter['small']['log_bytes']}")
        check(all(v == 2 for v in schema["checks"]["schema_versions"]), "the sc_account tables are not at version 2")
    check_consistency(native, check)
    check_telemetry(ffi, check)
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
