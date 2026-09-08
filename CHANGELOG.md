# Changelog

## Milestone 1.0.2

Based directly on Milestone 1.0.1 Hardened.

Added:
- Rust-only demo under `examples/demo`;
- Rust-only benchmark baseline under `examples/benchmark`;
- current point-read, historical-read, durable single-commit, durable batch and WAL-recovery measurements;
- p50/p95/p99 latency reporting where applicable;
- optional machine-readable JSON benchmark output;
- example/benchmark methodology documentation.

Unchanged:
- engine architecture and data model;
- in-memory Current/Version stores;
- MVCC semantics;
- transaction behavior;
- WAL format, durability and recovery behavior;
- all Milestone 1.0.1 hardening constraints.

No later-milestone functionality has been backported.

## Milestone 1.0.1

WAL/recovery hardening patch over Milestone 1.0, including bounded WAL payloads,
oversized length rejection, crash-tail discovery/truncation, checked frame-length
conversion and regression coverage.
