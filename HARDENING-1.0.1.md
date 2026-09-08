# Milestone 1.0.1 — WAL / Recovery Hardening

This patch preserves Milestone 1 scope and data model.

Implemented:
- bounded WAL payloads (`16 MiB` per serialized record),
- oversized length rejection before allocation,
- crash-tail discovery,
- automatic truncation of incomplete WAL suffix before any new append,
- checked `u32` frame-length conversion,
- regression tests for tail truncation and oversized disk lengths.

Still intentionally deferred:
- segmented WAL,
- checkpointing,
- streaming/bounded whole-WAL recovery,
- persistent current store (belongs to 1.5).
