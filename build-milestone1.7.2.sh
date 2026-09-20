#!/usr/bin/env bash
set -euo pipefail
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
cargo run --release -p adb-demo-1-7-2
cargo run --release -p adb-benchmark-1-7-2 -- --rows 1000 --iters 100
printf 'Milestone 1.7.2 validation completed.\n'
