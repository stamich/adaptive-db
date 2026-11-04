#!/usr/bin/env bash
# Milestone 2.5.4.1 workload benchmark (workloads A-H of 2.4.0 plus the consistency workloads I-O),
# rust_native path: loads the dataset at a scale factor, times
# ANALYZE of every table and the build of every index, executes the cost-based plans of workloads
# A-E directly in Rust, and measures an index read at an older snapshot and the write overhead of
# 0, 1 and 3 indexes.
#
# Environment:
#   ADB_BENCH_SCALE       scale factor (default 1; 10 and 100 grow the large tables 10x / 100x)
#   ADB_BENCH_ITERATIONS  timed iterations per workload (default 100)
#   ADB_BENCH_WARMUP_MS   minimum warm-up per workload (default 200)
#   ADB_BENCH_OUT         report (default examples/results/2.5.4.1-database[-sfN].json)
#
# Before you run it on Linux: set the CPU governor to performance
# (`sudo cpupower frequency-set -g performance`). With the default governor the write and commit
# rates measured on the reference machine were 2 to 8 times lower for identical code
# (examples/results/README.md). The script warns when the governor is not `performance`.
set -euo pipefail

# Warn (never fail) when the CPU governor would distort the write and commit rates.
GOV_FILE=/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor
if [ -r "$GOV_FILE" ] && [ "$(cat "$GOV_FILE")" != "performance" ]; then
  echo "WARNING: CPU governor is '$(cat "$GOV_FILE")', not 'performance'; write and commit rates will be" \
       "too low. Run: sudo cpupower frequency-set -g performance" >&2
fi
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCALE="${ADB_BENCH_SCALE:-1}"
ITERATIONS="${ADB_BENCH_ITERATIONS:-100}"
WARMUP_MS="${ADB_BENCH_WARMUP_MS:-200}"
SUFFIX=""; [[ "$SCALE" == "1" ]] || SUFFIX="-sf$SCALE"
OUT="${ADB_BENCH_OUT:-$ROOT/examples/results/2.5.4.1-database$SUFFIX.json}"

cd "$ROOT"
cargo run --release -p adb-benchmark-rust --bin workloads -- \
  --scale "$SCALE" --iters "$ITERATIONS" --warmup-ms "$WARMUP_MS" --output "$OUT"
echo "rust_native results: $OUT"
