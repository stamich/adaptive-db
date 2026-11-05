#!/usr/bin/env bash
# Milestone 2.5.4.1 validation.
#
# Rust (cargo) and the Java FFM smoke test need only cargo + JDK 22. The Scala/JVM part uses the
# Gradle wrapper in jvm/ (or a Gradle 9 on PATH) and needs access to Maven Central; it is skipped
# with a clear message when neither is available.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

command -v cargo >/dev/null || { echo "ERROR: cargo required" >&2; exit 1; }
JAVA_MAJOR="$(java -version 2>&1 | sed -n 's/.*version "\([0-9][0-9]*\).*/\1/p' | head -n1)"
[[ -n "$JAVA_MAJOR" && "$JAVA_MAJOR" -ge 22 ]] || { echo "ERROR: JDK 22+ required" >&2; exit 1; }

echo "== Rust: format, lints (incl. documentation of every item), tests, docs"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings -W missing_docs -W clippy::missing_docs_in_private_items
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

echo "== Rust: release build, relational demo, migration tool, data-plane benchmark, workload benchmark (rust_native)"
cargo build --release -p adb-ffi -p adb-migrate -p adb-benchmark-rust
cargo run --release -p adb-rust-demo
# Benchmark gates: exit 3 (unstable) and 4 (diagnostic, e.g. the governor is not `performance`) say how
# far the numbers can be trusted; they are not build failures. Only 1 (fail) and 2 (tool error) are.
bench() { "$@" || { code=$?; if [[ $code -eq 3 || $code -eq 4 ]]; then echo "note: gate status $code (see docs/BENCHMARKS.md)"; else exit "$code"; fi; }; }
echo "== Benchmark tools: unit tests and the report validator on the committed reports"
python3 -m unittest discover -s scripts/tests
python3 scripts/bench_validate.py examples/results/*.json
bench python3 scripts/bench_run.py --out examples/results/2.5.4.1-rust-local.json \
  --cmd "target/release/adb-benchmark-rust --output {out}"
bench ./scripts/benchmark-db.sh
bench ./scripts/benchmark.sh memory --out examples/results/2.5.4.1
bench ./scripts/benchmark.sh scaling --out examples/results/2.5.4.1

echo "== Java FFM -> C ABI v9 -> engine smoke test (incl. ANALYZE, statistics, indexes, schemas, commit order, change-log cursors and projections)"
./examples/jvm-cdc-smoke/run.sh

if command -v scalac >/dev/null && command -v scala >/dev/null; then
  echo "== JVM modules -> real engine: sessions, tokens, SET SESSION CONSISTENCY (plain scalac, no Gradle)"
  ./examples/jvm-consistency-smoke/run.sh
  echo "== JVM modules -> real engine: plan cache hits and invalidation (plain scalac, no Gradle)"
  ./examples/jvm-plan-cache-smoke/run.sh
  echo "== JVM modules -> real engine: telemetry hot path (plain scalac, no Gradle)"
  ./examples/jvm-telemetry-smoke/run.sh
  echo "== JVM modules -> real engine: workload advisor, hysteresis, audit log (plain scalac, no Gradle)"
  ./examples/jvm-advisor-smoke/run.sh
  echo "== JVM modules -> real engine: AUTO_SAFE actuator, verification, rollback, budget (plain scalac, no Gradle)"
  ./examples/jvm-actuator-smoke/run.sh
else
  echo "== SKIPPED consistency, plan-cache, telemetry, advisor and actuator smoke tests: scalac/scala not found"
fi

if [[ -x jvm/gradlew ]]; then GRADLE=(./gradlew); elif command -v gradle >/dev/null; then GRADLE=(gradle); else GRADLE=(); fi
if (( ${#GRADLE[@]} )); then
  echo "== Scala/JVM tests, planner benchmark, workload benchmark (FFI paths) and demo"
  (cd jvm && "${GRADLE[@]}" clean test)
  for family in sql-path telemetry actuator ffi; do
    bench ./scripts/benchmark.sh "$family" --out examples/results/2.5.4.1
  done
  bench ./scripts/benchmark-ffi.sh
  python3 scripts/check-benchmarks.py examples/results/2.5.4.1-database.json examples/results/2.5.4.1-ffi.json
  ./demo/run-demo.sh
else
  echo "== SKIPPED Scala/JVM tests, FFI benchmark and demo: neither jvm/gradlew nor gradle found"
fi

echo "Milestone 2.5.4.1 validation completed."
