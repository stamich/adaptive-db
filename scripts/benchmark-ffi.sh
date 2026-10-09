#!/usr/bin/env bash
# Milestone 2.2.3 workload benchmark, JVM paths: the Rust binary prepares the dataset (the only
# definition of the data), then the JVM measures planning_only, ffi_prepared_plan and
# scala_cbo_ffi_rust for workloads A and B, join order (chain and star) in cost and rule mode,
# estimation quality (skew, correlation, stale statistics), planning time for 2-12 relations
# and the cost calibration.
#
# Environment:
#   ADB_BENCH_SCALE       scale factor (default 1)
#   ADB_BENCH_ITERATIONS  timed iterations per path (default 100)
#   ADB_BENCH_WARMUP_MS   minimum warm-up per timed path, for the JIT (default 2000)
#   ADB_BENCH_OUT         report (default examples/results/2.2.3-ffi[-sfN].json)
#   ADB_BENCH_DATA        working directory, must not exist (default: a fresh temporary directory)
#   ADB_NATIVE_LIBRARY    native library (default: the release build of adb-ffi)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCALE="${ADB_BENCH_SCALE:-1}"
ITERATIONS="${ADB_BENCH_ITERATIONS:-100}"
WARMUP_MS="${ADB_BENCH_WARMUP_MS:-2000}"
SUFFIX=""; [[ "$SCALE" == "1" ]] || SUFFIX="-sf$SCALE"
OUT="${ADB_BENCH_OUT:-$ROOT/examples/results/2.2.3-ffi$SUFFIX.json}"
DATA="${ADB_BENCH_DATA:-$(mktemp -d)/adb-bench}"
case "$(uname -s)" in
  Linux)  DEFAULT_LIB="$ROOT/target/release/libadb_ffi.so" ;;
  Darwin) DEFAULT_LIB="$ROOT/target/release/libadb_ffi.dylib" ;;
  *) echo "ERROR: unsupported OS" >&2; exit 1 ;;
esac
LIB="${ADB_NATIVE_LIBRARY:-$DEFAULT_LIB}"

if [[ -x "$ROOT/jvm/gradlew" ]]; then GRADLE=(./gradlew); elif command -v gradle >/dev/null; then GRADLE=(gradle); else
  echo "ERROR: jvm/gradlew or gradle is required" >&2; exit 1
fi

(cd "$ROOT" && cargo build --release -p adb-ffi)
(cd "$ROOT" && cargo run --release -p adb-benchmark-rust --bin workloads -- --prepare "$DATA/rust" --scale "$SCALE")
(cd "$ROOT/jvm" && "${GRADLE[@]}" -q :adb-benchmark:run \
  --args="--workloads --scale $SCALE --iterations $ITERATIONS --warmup-ms $WARMUP_MS --data $DATA --native-lib $LIB --out $OUT")
echo "JVM/FFI results: $OUT"
