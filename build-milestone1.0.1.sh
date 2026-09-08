#!/usr/bin/env bash
set -euo pipefail
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
echo "Milestone 1.0.1 hardened build completed."
