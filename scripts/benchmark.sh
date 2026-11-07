#!/usr/bin/env bash
# The Milestone 2.5.4.1 benchmark launcher.
#
#   scripts/benchmark.sh FAMILY|all [--quick] [--repeat N] [--ab REF] [--out DIR] [--scale N]
#
# Families: rust-local workloads memory scaling (Rust) and sql-path telemetry actuator ffi (JVM, one
# JVM per family). `all` runs every family in that order.
#
#   --quick     the quick measurement plans (a smoke run, not release grade; ADB_BENCH_QUICK=1)
#   --repeat N  run each family N times as fresh processes and keep the median document
#               (scripts/bench_repeat.py); without it a family is run once and measured again,
#               up to twice, while it is unstable (scripts/bench_run.py)
#   --ab REF    instead of a plain run, compare the working tree with git ref REF with interleaved
#               A/B/A2 runs (scripts/bench_ab.py); Rust families only
#   --out DIR   where the reports go (default examples/results/2.5.4.1)
#   --scale N   scale factor of the workloads (default 1)
#
# Every report is gated (scripts/bench_gate.py). The exit status is the worst of all families:
# 0 pass or skipped, 1 fail, 3 unstable, 4 diagnostic (for instance a CPU governor that is not
# `performance`; set it with `sudo cpupower frequency-set -g performance`). Timings never fail CI.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Durable benchmarks must not run on a RAM disk (/tmp is one on many systems): the storage gate fails
# them. Default to a directory under the repository, i.e. on the disk the repository lives on.
export ADB_BENCH_DATA_DIR="${ADB_BENCH_DATA_DIR:-$ROOT/target/bench-data}"
mkdir -p "$ADB_BENCH_DATA_DIR"
RUST_FAMILIES=(rust-local workloads memory scaling)
JVM_FAMILIES=(sql-path telemetry actuator ffi)
TARGET="all"; REPEAT=1; AB=""; OUT_DIR="$ROOT/examples/results/2.5.4.1"; SCALE=1

usage() { sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 2; }
[[ $# -ge 1 ]] || usage
TARGET="$1"; shift
while [[ $# -gt 0 ]]; do
  case "$1" in
    --quick) export ADB_BENCH_QUICK=1 ;;
    --repeat) REPEAT="${2:?--repeat needs a number}"; shift ;;
    --ab) AB="${2:?--ab needs a git ref}"; shift ;;
    --out) OUT_DIR="${2:?--out needs a directory}"; shift ;;
    --scale) SCALE="${2:?--scale needs a number}"; shift ;;
    *) usage ;;
  esac
  shift
done
[[ "$REPEAT" =~ ^[0-9]+$ && "$REPEAT" -ge 1 ]] || { echo "ERROR: --repeat must be a positive number" >&2; exit 2; }
ALL=("${RUST_FAMILIES[@]}" "${JVM_FAMILIES[@]}")
if [[ "$TARGET" == "all" ]]; then FAMILIES=("${ALL[@]}"); else
  [[ " ${ALL[*]} " == *" $TARGET "* ]] || { echo "ERROR: unknown family '$TARGET'" >&2; usage; }
  FAMILIES=("$TARGET")
fi
mkdir -p "$OUT_DIR"

GOV_FILE=/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor
if [ -r "$GOV_FILE" ] && [ "$(cat "$GOV_FILE")" != "performance" ]; then
  echo "WARNING: CPU governor is '$(cat "$GOV_FILE")', not 'performance'; the run will be diagnostic." >&2
fi

is_rust() { [[ " ${RUST_FAMILIES[*]} " == *" $1 "* ]]; }
rust_binary() { case "$1" in rust-local) echo adb-benchmark-rust ;; *) echo "$1" ;; esac; }
statuses=(0)

cd "$ROOT"
for family in "${FAMILIES[@]}"; do
  out="$OUT_DIR/$family.json"
  if is_rust "$family"; then
    cargo build --release -q -p adb-benchmark-rust
    cmd="$ROOT/target/release/$(rust_binary "$family") --output {out}"
    [[ "$family" == workloads ]] && cmd="$cmd --scale $SCALE"
  else
    cargo build --release -q -p adb-ffi -p adb-benchmark-rust
    cmd="$ROOT/scripts/bench-jvm-family.sh $family {out} $SCALE"
  fi
  code=0
  if [[ -n "$AB" ]]; then
    if is_rust "$family"; then
      python3 scripts/bench_ab.py --family "$family" --baseline "$AB" --rounds 3 --out "$OUT_DIR/$family.ab.json" || code=$?
    else
      echo "== $family: skipped for --ab (JVM families do not build against an old engine)"
    fi
  elif [[ "$REPEAT" -gt 1 ]]; then
    python3 scripts/bench_repeat.py --runs "$REPEAT" --out "$out" --cmd "$cmd" || code=$?
    gate=0; python3 scripts/bench_gate.py "$out" || gate=$?
    [[ $gate -gt $code ]] && code=$gate
  else
    python3 scripts/bench_run.py --out "$out" --cmd "$cmd" || code=$?
  fi
  echo "== $family: exit $code"
  statuses+=("$code")
done

# Worst status: fail (1) outranks diagnostic (4) outranks unstable (3); 2 is a tool error.
worst=0
for s in "${statuses[@]}"; do
  case "$s" in
    2) worst=2; break ;;
    1) worst=1 ;;
    4) [[ $worst -ne 1 ]] && worst=4 ;;
    3) [[ $worst -eq 0 ]] && worst=3 ;;
  esac
done
echo "benchmark: exit $worst (reports in $OUT_DIR)"
exit "$worst"
