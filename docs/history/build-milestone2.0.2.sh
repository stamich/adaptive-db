#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

command -v cargo >/dev/null || { echo "ERROR: cargo required" >&2; exit 1; }
command -v gradle >/dev/null || { echo "ERROR: gradle required" >&2; exit 1; }
JAVA_MAJOR="$(java -version 2>&1 | sed -n '1s/.*version "\([0-9][0-9]*\).*/\1/p')"
[[ -n "$JAVA_MAJOR" && "$JAVA_MAJOR" -ge 22 ]] || { echo "ERROR: JDK 22+ required" >&2; exit 1; }

cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
cargo build --release -p adb-ffi
cargo run --release -p adb-benchmark-2-0-2-rust -- --rows 1000 --iters 100

(cd jvm && gradle clean test)
(cd jvm && gradle :adb-benchmark:run --args='--iterations 1000')

./demo/run-demo.sh

echo "Milestone 2.0.2 validation completed."
