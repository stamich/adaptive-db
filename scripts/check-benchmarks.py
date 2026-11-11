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
Since 2.5.4 the same section holds a write statement to an allocation budget too: the per-entity
write counters add (almost) nothing to it.
The advisor section (2.5.3) must show the hysteresis at work (no advice after two evaluations,
an index on the filtered column after three), an unchanged number of indexes and a non-empty
audit log; these are counts, so they are checked exactly.
The actuator section (2.5.4) must do nothing in off and advisory mode, keep the index that cuts
the rows read and roll back the one that does not, refuse the rolled-back target during its
cooldown, never fail an action, and leave the user's own indexes untouched; these are counts and
exact engine counters, so they are checked exactly.

Usage: check-benchmarks.py DATABASE_JSON FFI_JSON
Exits with status 1 and one line per failure if anything disagrees. Timings are never checked:
they depend on the machine.
"""
import json
import sys


import bench_gate  # noqa: E402
import bench_lib  # noqa: E402


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
    # Write counters (2.5.4): counting a write is one map lookup and three adder increments, so a
    # counted write allocates no more than an uncounted one beyond a small constant.
    check(section["write_alloc_extra_counted"] <= 64,
          f"telemetry: a counted write allocates {section['write_alloc_extra_counted']:.0f} extra bytes (budget 64)")
    check(section["write_statements_counted"] > 0, "telemetry: no write was counted while telemetry was on")


def check_advisor(ffi: dict, check) -> None:
    """Checks the 2.5.3 advisor section of the JVM report with `check(ok, message)`.

    Everything checked is a count, which does not depend on the load of the machine: the advice
    for the wasteful scan of workload E must be held back by the hysteresis for two evaluations and
    shown on the third, it must name an index on the filtered column, and the advisor must not
    have created any index itself.
    """
    section = ffi.get("advisor")
    check(section is not None, "the JVM report has no advisor section")
    if section is None:
        return
    check(section["recommended_after_two"] == 0, "advisor: advice was shown before the hysteresis allowed it")
    check(section["recommended_indexes"] >= 1, "advisor: no index was recommended for the unindexed scan of workload E")
    check("ix_account" in section["index_sql"] and "region" in section["index_sql"],
          f"advisor: the advice does not target ix_account(region): {section['index_sql']!r}")
    check(section["indexes_before"] == section["indexes_after"], "advisor: the advisor changed the number of indexes (it must only recommend)")
    check(section["audit_entries"] >= 1, "advisor: the audit log is empty although advice was confirmed")


def check_actuator(ffi: dict, check) -> None:
    """Checks the 2.5.4 actuator section of the JVM report with `check(ok, message)`.

    Counts and the engine's rows-read counter only; the mean tick time is informational.
    """
    section = ffi.get("actuator")
    check(section is not None, "the JVM report has no actuator section")
    if section is None:
        return
    check(section["idle_actions"] == 0, f"actuator: {section['idle_actions']} actions or log entries while the mode was off or advisory")
    check(section["actions_kept"] == 1, f"actuator: {section['actions_kept']} indexes kept (expected the one that helps)")
    check(section["actions_rolled_back"] == 1, f"actuator: {section['actions_rolled_back']} indexes rolled back (expected the one that does not help)")
    check(section["actions_rejected"] >= 1, "actuator: the rolled-back target was not refused during its cooldown")
    check(section["actions_failed"] == 0, f"actuator: {section['actions_failed']} actions failed")
    check(section["rows_read_per_execution_after"] * 4 <= section["rows_read_per_execution_before"],
          f"actuator: rows read per execution {section['rows_read_per_execution_before']:.0f} -> "
          f"{section['rows_read_per_execution_after']:.0f}, not at least 4x fewer")
    check(section["user_indexes_before"] == section["user_indexes_after"], "actuator: the user's own indexes changed")
    check(section["advisor_indexes_after"] == 1, f"actuator: {section['advisor_indexes_after']} advisor indexes left (expected 1)")


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
    check_advisor(ffi, check)
    check_actuator(ffi, check)
    estimation = ffi["estimation"]
    check(estimation["stale_statistics"].get("stale_warning") is True, "stale statistics were not reported")
    check((estimation["after_analyze"].get("q_error") or 99) <= 1.5, "ANALYZE did not repair the stale estimate")
    check((estimation["zipf_most_common"].get("q_error") or 99) <= 1.5, "most common value badly estimated")

    # Since 2.5.4.1 the reports also pass the machine and measurement gates. Only a `fail` gate (a broken
    # schema, a RAM disk under a durable family) fails the check; `unstable` and `diagnostic` are
    # reported but never fail it, because timings do not gate CI.
    for label, document in (("database", native), ("ffi", ffi)):
        verdict = bench_gate.evaluate(document)
        check(verdict["status"] != bench_lib.FAIL, f"{label} report gate failed: {verdict['gates']}")
        if verdict["status"] in (bench_lib.UNSTABLE, bench_lib.DIAGNOSTIC):
            print(f"note: {label} report is {verdict['status']} (informational)")

    for failure in failures:
        print(f"FAIL {failure}")
    if not failures:
        print(f"benchmark results agree (scale {scale})")
    return 1 if failures else 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    sys.exit(main(sys.argv[1], sys.argv[2]))
