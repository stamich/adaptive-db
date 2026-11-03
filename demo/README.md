# Adaptive DB 2.5.3 Demo

This demo is a chronological feature tour of the engine lineage from **1.0.1** through **2.5.3**.
It runs one current binary and labels each phase with the milestone where the demonstrated capability first became part of the project.

## Run

Requirements:

- Rust/Cargo,
- JDK 22+,
- the Gradle wrapper in `jvm/` (or Gradle 9.x on PATH),
- Linux or macOS for the convenience script.

From the repository root:

```bash
./demo/run-demo.sh
```

The script builds `adb-ffi`, resets only the repository-local `.demo-data` directory, and starts the JVM CLI in `--demo` mode.

## Tour

| Phase | Capability shown |
|---|---|
| 1.0.1 | transaction commits, WAL durability, reopen/recovery |
| 1.5.1 | persistent Current Store, primary B+Tree, point lookup |
| 1.6.1 | persistent Version Store, historical `AS OF VERSION` reads |
| 1.7.1 | physical execution, Java FFM bridge, RecordBatch results, EXPLAIN ANALYZE |
| 2.0.1 | SQL parser, persistent catalog, binder, optimizer, physical planner, DDL/DML gateway |
| 2.1.3 | JOIN / LEFT JOIN / self-join, GROUP BY with aggregates, ORDER BY, TopK, NestedLoopJoin fallback, EXPLAIN ANALYZE with planner decisions and the native runtime profile |
| 2.2.3 | `REFERENCES ... NOT ENFORCED` hints, EXPLAIN without and with statistics, `ANALYZE`, join reordering, estimates per node, EXPLAIN ANALYZE q-error, cost vs rule mode, stale-statistics warning |
| 2.3.0 | `CREATE INDEX` / `SHOW INDEXES` / `DROP INDEX`, covering index scans, row fetches, ORDER BY ... LIMIT read from a (backward) index without TopK, `AS OF VERSION` through an index, cost-based choice between index and sequential scans with rejected alternatives in EXPLAIN |
| 2.4.0 | `ALTER TABLE ... ADD / RENAME / DROP COLUMN` without rewriting rows, virtual defaults, lazy migration on write, `DROP COLUMN` refused while an index uses the column (RESTRICT), write validation, `SHOW SCHEMA`, `DROP TABLE`, the EXPLAIN Schema section, and schemas surviving a second close/reopen |
| 2.5.0 | the global commit order (`commitSeq` of an `INSERT`), the session token and level (`\session`), the EXPLAIN Read context section, a second session with its own level, `SET SESSION CONSISTENCY STRONG / READ_YOUR_WRITES / BOUNDED / EVENTUAL`, the projection listing and the change log |
| 2.5.1 | the plan cache: EXPLAIN's `Plan cache:` line, the first/second/third run of one SELECT (remember, store, hit), invalidation by `ANALYZE`, `SET plan_cache = off / on` |
| 2.5.2 | the telemetry hot path: off (no counters move), full (shapes with rows read and returned), sampled (most queries skipped), a tiny queue that drops events without delaying a query |
| 2.5.2.1 | the same telemetry phase, now reading the rows a captured query read from the engine's own counters (C ABI 9) instead of the JSON profile; no new SQL |
| 2.5.3 | the workload advisor: a wasteful scan is recommended an index only after three evaluations in a row, nothing is created by the advisor, the advice leaves once the index exists, and the audit log keeps the history |

The demo intentionally performs a real close/reopen between the 1.0.1 and 1.5.1 phases, and again after 2.4.0 (the 2.5.0 phase runs in the reopened database, so its session starts at token 0), to make persistence visible rather than merely describing it. Since 2.4.0 the schemas live in the engine's log: the demo directory has no `catalog.properties`.

See `FEATURE-MAP.md` for the architecture mapping and `sql/2.0.1-tour.sql`, `sql/2.1.3-relational.sql`, `sql/2.2.3-statistics.sql`, `sql/2.3.0-indexes.sql`, `sql/2.4.0-schema.sql`, `sql/2.5.0-consistency.sql`, `sql/2.5.1-plan-cache.sql`, `sql/2.5.2-telemetry.sql` and `sql/2.5.3-advisor.sql` for the human-readable SQL sequence.

A Rust-only relational demo (plan wire round trip, HashJoin, Aggregate, TopK, runtime profile) runs with
`cargo run --release -p adb-rust-demo`.
