#!/usr/bin/env bash
# Milestone 2.5.4.1 workload benchmark, JVM paths: the Rust binary prepares the dataset (the only
# definition of the data), then one JVM creates the indexes through SQL and measures
# planning_only, ffi_prepared_plan and scala_cbo_ffi_rust for workloads A-E, the selectivity sweep,
# join order, estimation quality, planning time for 2-12 relations and the cost calibration.
# It is the `workloads` JVM run of scripts/bench-jvm-family.sh; the report is gated and measured
# again while unstable.
#
# Environment:
#   ADB_BENCH_SCALE     scale factor (default 1)
#   ADB_BENCH_OUT       report (default examples/results/2.5.4.1-ffi[-sfN].json)
#   ADB_NATIVE_LIBRARY  native library (default: the release build of adb-ffi)
#   ADB_BENCH_RETRIES   re-runs while unstable (default 1)
#   ADB_BENCH_QUICK     1 selects the quick plans (a smoke run, not release grade)
# `ADB_BENCH_ITERATIONS`, `ADB_BENCH_WARMUP_MS` and `ADB_BENCH_DATA` are ignored: the measurement
# plan decides and every run gets a fresh data directory (docs/BENCHMARKS.md).
#
# Set the CPU governor to performance first (`sudo cpupower frequency-set -g performance`); the
# gate reports `diagnostic` (exit 4) otherwise.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Durable benchmarks must not run on a RAM disk (/tmp is one on many systems): the storage gate fails
# them. Default to a directory under the repository, i.e. on the disk the repository lives on.
export ADB_BENCH_DATA_DIR="${ADB_BENCH_DATA_DIR:-$ROOT/target/bench-data}"
mkdir -p "$ADB_BENCH_DATA_DIR"
SCALE="${ADB_BENCH_SCALE:-1}"
SUFFIX=""; [[ "$SCALE" == "1" ]] || SUFFIX="-sf$SCALE"
OUT="${ADB_BENCH_OUT:-$ROOT/examples/results/2.5.4.1-ffi$SUFFIX.json}"
cd "$ROOT"
cargo build --release -q -p adb-ffi -p adb-benchmark-rust
status=0
python3 scripts/bench_run.py --retries "${ADB_BENCH_RETRIES:-1}" --out "$OUT" \
  --cmd "$ROOT/scripts/bench-jvm-family.sh workloads {out} $SCALE" || status=$?
echo "JVM/FFI results: $OUT"
exit "$status"
