#!/usr/bin/env bash
set -euo pipefail
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
cargo run --release -p adb-demo-1-6-2
cargo run --release -p adb-benchmark-1-6-2 -- \
  --rows 1000 --batch-size 100 --lookups 5000 --versions-per-row 4 --temporal-rows 100 \
  --output examples/results/1.6.2-smoke.json
echo "Milestone 1.6.2 demo/benchmark validation completed."
