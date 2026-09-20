# Adaptive DB — Milestone 1.7.1 Hardened

Hardening release based on Milestone 1.7. It inherits all 1.6.1 storage/WAL protections and
adds bounded execution-wire/FFI inputs, safer batch encoding and explicit raw-C lifetime rules.

See `HARDENING-1.7.1.md`, `SECURITY-REVIEW.md`, `RUSTDOC-AUDIT.txt`, and `VALIDATION-1.7.1.txt`.

## Milestone 1.7.2 demo/benchmark package

This package preserves the 1.7.1 Hardened engine scope and adds Rust-only examples under `examples/`.

```bash
cargo run --release -p adb-demo-1-7-2
cargo run --release -p adb-benchmark-1-7-2 -- --rows 10000 --iters 1000 --output examples/results/1.7.2-local.json
```
