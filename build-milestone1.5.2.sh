#!/usr/bin/env bash
set -euo pipefail
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
cargo run --release -p adb-demo-1-5-2
cargo run --release -p adb-benchmark-1-5-2 -- --rows 1000 --batch-size 100 --lookups 10000 --historical-versions 200
printf '%s\n' 'Milestone 1.5.2 validation completed.'
