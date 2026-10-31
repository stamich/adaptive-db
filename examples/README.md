# Examples — Milestone 2.4.0

## Rust data-plane benchmark

```bash
cargo run --release -p adb-benchmark-rust -- \
  --rows 5000 --iters 1000 --threads 8 --groups 100 --nested-rows 1000 --relational-iters 5 \
  --output examples/results/2.4.0-rust-local.json
```

Storage sections: bulk load, point lookup, `EntityScan` vs. full scan + filter, plan JSON parsing,
change-feed read throughput, single-row commit throughput with 1 and N threads (group commit),
current-heap size under update churn, and recovery time after a crash. Relational sections (2.1):
hash join, nested-loop join, GROUP BY with SUM, sort, TopK, plan wire encode/decode.

## Workload benchmark (2.3.0 workloads, 2.4.0 schema evolution)

```bash
./scripts/benchmark-db.sh    # rust_native, index builds, AS OF, write overhead, schema evolution
                             #                    -> examples/results/2.4.0-database.json
./scripts/benchmark-ffi.sh   # ffi_prepared_plan, scala_cbo_ffi_rust, selectivity sweep, join order,
                             # estimation, calibration  -> examples/results/2.4.0-ffi.json
python3 scripts/check-benchmarks.py examples/results/2.4.0-database.json examples/results/2.4.0-ffi.json
```

The Rust binary defines and loads the dataset (11 tables, `ADB_BENCH_SCALE`); the JVM opens the
prepared database, creates the matching tables (in the engine catalog since 2.4.0) and indexes
through SQL, and runs workloads A
(hash join + TopK), B (+ aggregate), C (covering index equality), D (index range + row fetch +
LIMIT, no sort) and E (sequential control). The `checksum` of a workload is identical on every
path. Settings: `ADB_BENCH_SCALE`, `ADB_BENCH_ITERATIONS`, `ADB_BENCH_WARMUP_MS`,
`ADB_BENCH_OUT`, `ADB_BENCH_DATA`, `ADB_NATIVE_LIBRARY`. Sections and recorded results:
[results/README.md](results/README.md).

## Rust relational demo

```bash
cargo run --release -p adb-rust-demo
```

## Java FFM smoke test

```bash
examples/jvm-cdc-smoke/run.sh
```

Builds `libadb_ffi`, compiles the Java binding with plain `javac` (no Gradle/Maven) and checks the
change feed, consumer offsets, vacuum, `entity_scan`, `ANALYZE`, statistics documents,
modification counts, index creation, listing and dropping, negative primary keys (data format 2),
schemas (create, alter, validation, stale-plan refusal, schema events in the change feed, drop)
and error statuses through the full Java → C ABI v7 → engine path.

## JVM planner benchmark and end-to-end benchmark

```bash
cargo build --release -p adb-ffi
cd jvm
./gradlew :adb-benchmark:run --args="--iterations 10000 --gateway-iterations 1000 --data ../.bench-data --native-lib ../target/release/libadb_ffi.so"
```

## Demo

```bash
./demo/run-demo.sh
```
