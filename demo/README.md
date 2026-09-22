# Adaptive DB 2.0.1 Demo

This demo is a chronological feature tour of the hardened engine lineage from **1.0.1** through **2.0.1**.
It runs one current 2.0.1 binary and labels each phase with the milestone where the demonstrated capability first became part of the project.

## Run

Requirements:

- Rust/Cargo,
- JDK 22+,
- Gradle 9.x,
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

The demo intentionally performs a real close/reopen between the 1.0.1 and 1.5.1 phases to make persistence visible rather than merely describing it.

See `FEATURE-MAP.md` for the architecture mapping and `sql/2.0.1-tour.sql` for the human-readable SQL sequence.
