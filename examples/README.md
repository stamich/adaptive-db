# Examples — Milestone 2.2.3

## Rust data-plane benchmark

```bash
cargo run --release -p adb-benchmark-rust -- \
  --rows 5000 --iters 1000 --threads 8 --groups 100 --nested-rows 1000 --relational-iters 5 \
  --output examples/results/2.2.3-rust-local.json
```

Storage sections: bulk load, point lookup, `EntityScan` vs. full scan + filter, plan JSON parsing,
change-feed read throughput, single-row commit throughput with 1 and N threads (group commit),
current-heap size under update churn, and recovery time after a crash. Relational sections (2.1):
hash join, nested-loop join, GROUP BY with SUM, sort, TopK, plan wire encode/decode.

## 2.2.3 workload benchmark

```bash
./scripts/benchmark-db.sh    # rust_native        -> examples/results/2.2.3-database.json
./scripts/benchmark-ffi.sh   # ffi_prepared_plan, scala_cbo_ffi_rust, join order, calibration
                             #                    -> examples/results/2.2.3-ffi.json
```

Both scripts load the same tables with the same formulas (`bench_customer`: 100 rows,
`bench_orders`: 1,000 rows) and run workload A (hash join + TopK) and workload B (hash join +
aggregate + TopK) 100 times. The `checksum` of a workload is identical on every path. Settings:
`ADB_BENCH_ITERATIONS`, `ADB_BENCH_OUT`, `ADB_BENCH_DATA`, `ADB_NATIVE_LIBRARY`.

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
modification counts and error statuses through the full Java → C ABI v5 → engine path.

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
