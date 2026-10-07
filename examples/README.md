# Examples — Milestone 2.0.3

## Rust benchmark

```bash
cargo run --release -p adb-benchmark-2-0-3-rust -- \
  --rows 5000 --iters 1000 --threads 8 \
  --output examples/results/2.0.3-rust-local.json
```

Sections: bulk load, point lookup, `EntityScan` vs full scan + filter, plan JSON parsing, change-feed
read throughput, single-row commit throughput with 1 and N threads (group commit), current-heap size
under update churn, and recovery time after a crash.

## Java FFM change-feed smoke test

```bash
examples/jvm-cdc-smoke/run.sh
```

Builds `libadb_ffi`, compiles the Java binding with plain `javac` (no Gradle/Maven) and checks the
change feed, consumer offsets, vacuum, `entity_scan` and error statuses through the full
Java → C ABI v3 → engine path.

## JVM planner benchmark and end-to-end benchmark

```bash
cd jvm
gradle :adb-benchmark:run --args='--iterations 10000'
cargo build --release -p adb-ffi
gradle :adb-benchmark:run --args="--iterations 10000 --gateway-iterations 1000 --data ../.bench-data --native-lib ../target/release/libadb_ffi.so"
```

## Demo

```bash
./demo/run-demo.sh
```
