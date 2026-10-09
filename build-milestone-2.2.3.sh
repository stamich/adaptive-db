#!/usr/bin/env bash
# Milestone 2.2.3 validation.
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

echo "== Rust: release build, relational demo, data-plane benchmark, workload benchmark (rust_native)"
cargo build --release -p adb-ffi
cargo run --release -p adb-rust-demo
cargo run --release -p adb-benchmark-rust -- --rows 5000 --iters 1000 --groups 100 \
  --nested-rows 1000 --relational-iters 5 --output examples/results/2.2.3-rust-local.json
./scripts/benchmark-db.sh

echo "== Java FFM -> C ABI v5 -> engine smoke test (incl. ANALYZE and statistics)"
./examples/jvm-cdc-smoke/run.sh

if [[ -x jvm/gradlew ]]; then GRADLE=(./gradlew); elif command -v gradle >/dev/null; then GRADLE=(gradle); else GRADLE=(); fi
if (( ${#GRADLE[@]} )); then
  echo "== Scala/JVM tests, planner benchmark, workload benchmark (FFI paths) and demo"
  (cd jvm && "${GRADLE[@]}" clean test)
  (cd jvm && "${GRADLE[@]}" :adb-benchmark:run --args='--iterations 1000')
  ./scripts/benchmark-ffi.sh
  python3 scripts/check-benchmarks.py examples/results/2.2.3-database.json examples/results/2.2.3-ffi.json
  ./demo/run-demo.sh
else
  echo "== SKIPPED Scala/JVM tests, FFI benchmark and demo: neither jvm/gradlew nor gradle found"
fi

echo "Milestone 2.2.3 validation completed."
