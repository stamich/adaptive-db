# Milestone 2.0.2 demo and benchmark

Milestone 2.0.2 keeps the hardened 2.0.1 SQL/JVM feature scope. The existing chronological demo is
under `../demo/`. `rust-benchmark` provides a data-plane baseline; `jvm/adb-benchmark` separates JVM
planning cost and can optionally measure the complete SQL/FFM/Rust path.

## Demo

```bash
./demo/run-demo.sh
```

## Rust benchmark

```bash
cargo run --release -p adb-benchmark-2-0-2-rust -- \
  --rows 5000 --iters 1000 \
  --output examples/results/2.0.2-rust-local.json
```

## JVM planner-only benchmark

No native library is required for this mode:

```bash
cd jvm
gradle :adb-benchmark:run --args='--iterations 10000'
```

## JVM end-to-end benchmark

Build the native library first and pass both paths:

```bash
cargo build --release -p adb-ffi
cd jvm
gradle :adb-benchmark:run --args="--iterations 10000 --gateway-iterations 1000 --data ../.bench-data --native-lib ../target/release/libadb_ffi.so"
```
