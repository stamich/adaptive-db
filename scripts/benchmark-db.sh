#!/usr/bin/env bash
# Milestone 2.2.3 workload benchmark, rust_native path: loads the dataset at a scale factor, times
# ANALYZE of every table and executes the cost-based plans of workloads A and B directly in Rust.
#
# Environment:
#   ADB_BENCH_SCALE       scale factor (default 1; 10 and 100 grow the large tables 10x / 100x)
#   ADB_BENCH_ITERATIONS  timed iterations per workload (default 100)
#   ADB_BENCH_WARMUP_MS   minimum warm-up per workload (default 200)
#   ADB_BENCH_OUT         report (default examples/results/2.2.3-database[-sfN].json)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCALE="${ADB_BENCH_SCALE:-1}"
ITERATIONS="${ADB_BENCH_ITERATIONS:-100}"
WARMUP_MS="${ADB_BENCH_WARMUP_MS:-200}"
SUFFIX=""; [[ "$SCALE" == "1" ]] || SUFFIX="-sf$SCALE"
OUT="${ADB_BENCH_OUT:-$ROOT/examples/results/2.2.3-database$SUFFIX.json}"

cd "$ROOT"
cargo run --release -p adb-benchmark-rust --bin workloads -- \
  --scale "$SCALE" --iters "$ITERATIONS" --warmup-ms "$WARMUP_MS" --output "$OUT"
echo "rust_native results: $OUT"
