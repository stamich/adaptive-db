#!/usr/bin/env bash
# Milestone 2.2.3 workload benchmark, rust_native path: loads the shared benchmark tables, runs
# ANALYZE and executes the cost-based plans of workloads A and B directly in Rust.
#
# Environment: ADB_BENCH_ITERATIONS (default 100), ADB_BENCH_OUT (default
# examples/results/2.2.3-database.json).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ITERATIONS="${ADB_BENCH_ITERATIONS:-100}"
OUT="${ADB_BENCH_OUT:-$ROOT/examples/results/2.2.3-database.json}"

cd "$ROOT"
cargo run --release -p adb-benchmark-rust --bin workloads -- --iters "$ITERATIONS" --output "$OUT"
echo "rust_native results: $OUT"
