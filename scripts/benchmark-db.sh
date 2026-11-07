#!/usr/bin/env bash
# Milestone 2.5.4.1 workload benchmark, rust_native path (workloads A-H of 2.4.0 plus the
# consistency workloads I-O): one family of scripts/benchmark.sh, kept as a shortcut because the
# milestone scripts and docs refer to it. The report is gated and measured again while unstable.
#
# Environment:
#   ADB_BENCH_SCALE   scale factor (default 1; 10 and 100 grow the large tables 10x / 100x)
#   ADB_BENCH_OUT     report (default examples/results/2.5.4.1-database[-sfN].json)
#   ADB_BENCH_RETRIES re-runs while unstable (default 1: a run takes 10-20 minutes)
#   ADB_BENCH_QUICK   1 selects the quick plans (a smoke run, not release grade)
# The iteration and warm-up counts are no longer settings: the measurement plan decides
# (docs/BENCHMARKS.md). `ADB_BENCH_ITERATIONS` and `ADB_BENCH_WARMUP_MS` are ignored.
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
OUT="${ADB_BENCH_OUT:-$ROOT/examples/results/2.5.4.1-database$SUFFIX.json}"
cd "$ROOT"
cargo build --release -q -p adb-benchmark-rust
status=0
python3 scripts/bench_run.py --retries "${ADB_BENCH_RETRIES:-1}" --out "$OUT" \
  --cmd "$ROOT/target/release/workloads --output {out} --scale $SCALE" || status=$?
echo "rust_native results: $OUT"
exit "$status"
