#!/usr/bin/env bash
# Milestone 2.2.3 workload benchmark, JVM paths: ffi_prepared_plan (prepared plan through Java
# FFM) and scala_cbo_ffi_rust (full SQL with the cost-based optimizer), plus the join-order
# workload in cost and rule mode and the cost calibration.
#
# Environment: ADB_BENCH_ITERATIONS (default 100), ADB_BENCH_OUT (default
# examples/results/2.2.3-ffi.json), ADB_BENCH_DATA (default: a fresh temporary directory),
# ADB_NATIVE_LIBRARY (default: the release build of adb-ffi).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ITERATIONS="${ADB_BENCH_ITERATIONS:-100}"
OUT="${ADB_BENCH_OUT:-$ROOT/examples/results/2.2.3-ffi.json}"
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
(cd "$ROOT/jvm" && "${GRADLE[@]}" -q :adb-benchmark:run \
  --args="--workloads --iterations $ITERATIONS --data $DATA --native-lib $LIB --out $OUT")
echo "JVM/FFI results: $OUT"
