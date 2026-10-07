# Adaptive DB 2.1.3 Demo

This demo is a chronological feature tour of the engine lineage from **1.0.1** through **2.1.3**.
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

The demo intentionally performs a real close/reopen between the 1.0.1 and 1.5.1 phases to make persistence visible rather than merely describing it.

See `FEATURE-MAP.md` for the architecture mapping and `sql/2.0.1-tour.sql` plus `sql/2.1.3-relational.sql` for the human-readable SQL sequence.

A Rust-only relational demo (plan wire round trip, HashJoin, Aggregate, TopK, runtime profile) runs with
`cargo run --release -p adb-rust-demo`.
