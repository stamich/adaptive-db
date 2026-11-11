#!/usr/bin/env bash
# Runs ONE JVM benchmark family in a JVM of its own and writes its schema-3 report.
#
#   bench-jvm-family.sh FAMILY OUT [SCALE]
#
# FAMILY is sql-path, telemetry, actuator, ffi or workloads. One invocation is one JVM, so the JIT
# profile of one family never colours another; ADB_BENCH_FORK=1 tells the report so (`fresh_fork`).
# Every run gets a fresh data directory. The `workloads` family needs the dataset the Rust binary
# prepares (the only definition of the data), so it is prepared first.
#
# Environment:
#   ADB_NATIVE_LIBRARY  native library (default: the release build of adb-ffi)
#   ADB_BENCH_JVM_RUN   command that replaces `gradle :adb-benchmark:run --args=...` and receives the
#                       benchmark arguments instead (for a machine without Gradle)
#   ADB_BENCH_QUICK     1 selects the quick measurement plan
set -euo pipefail

FAMILY="${1:?usage: bench-jvm-family.sh FAMILY OUT [SCALE]}"
OUT="${2:?usage: bench-jvm-family.sh FAMILY OUT [SCALE]}"
SCALE="${3:-1}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "$(uname -s)" in
  Linux)  DEFAULT_LIB="$ROOT/target/release/libadb_ffi.so" ;;
  Darwin) DEFAULT_LIB="$ROOT/target/release/libadb_ffi.dylib" ;;
  *) echo "ERROR: unsupported OS" >&2; exit 1 ;;
esac
LIB="${ADB_NATIVE_LIBRARY:-$DEFAULT_LIB}"
BASE="${ADB_BENCH_DATA_DIR:-$ROOT/target/bench-data}"; mkdir -p "$BASE"
DATA="$(mktemp -d -p "$BASE")/adb-bench"
trap 'rm -rf "$(dirname "$DATA")"' EXIT

if [[ "$FAMILY" == "workloads" ]]; then
  "$ROOT/target/release/workloads" --prepare "$DATA/rust" --scale "$SCALE"
fi
ARGS="--family $FAMILY --scale $SCALE --data $DATA --native-lib $LIB --out $OUT"
export ADB_BENCH_FORK=1
if [[ -n "${ADB_BENCH_JVM_RUN:-}" ]]; then
  # shellcheck disable=SC2086
  $ADB_BENCH_JVM_RUN $ARGS
else
  if [[ -x "$ROOT/jvm/gradlew" ]]; then GRADLE=(./gradlew); elif command -v gradle >/dev/null; then GRADLE=(gradle); else
    echo "ERROR: jvm/gradlew or gradle is required (or set ADB_BENCH_JVM_RUN)" >&2; exit 1
  fi
  (cd "$ROOT/jvm" && "${GRADLE[@]}" -q :adb-benchmark:run --args="$ARGS")
fi
