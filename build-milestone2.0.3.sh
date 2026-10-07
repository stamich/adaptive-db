#!/usr/bin/env bash
# Milestone 2.0.3 validation. The Rust part and the Java FFM smoke test need only cargo + JDK 22.
# The Scala/JVM part additionally needs Gradle 9 and access to Maven Central; it is skipped with a
# clear message when Gradle is missing.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

command -v cargo >/dev/null || { echo "ERROR: cargo required" >&2; exit 1; }
JAVA_MAJOR="$(java -version 2>&1 | sed -n 's/.*version "\([0-9][0-9]*\).*/\1/p' | head -n1)"
[[ -n "$JAVA_MAJOR" && "$JAVA_MAJOR" -ge 22 ]] || { echo "ERROR: JDK 22+ required" >&2; exit 1; }

echo "== Rust: format, lints, tests, docs"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

echo "== Rust: release build and benchmark"
cargo build --release -p adb-ffi
cargo run --release -p adb-benchmark-2-0-3-rust -- --rows 5000 --iters 1000 \
  --output examples/results/2.0.3-rust-local.json

echo "== Java FFM -> C ABI v3 -> engine smoke test"
./examples/jvm-cdc-smoke/run.sh

if command -v gradle >/dev/null; then
  echo "== Scala/JVM tests, benchmark and demo"
  (cd jvm && gradle clean test)
  (cd jvm && gradle :adb-benchmark:run --args='--iterations 1000')
  ./demo/run-demo.sh
else
  echo "== SKIPPED Scala/JVM tests and demo: gradle not found"
fi

echo "Milestone 2.0.3 validation completed."
