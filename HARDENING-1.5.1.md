# Milestone 1.5.1 — Persistent Storage Hardening

This patch preserves the Milestone 1.5 architecture while hardening crash/corruption behavior.

Inherited from 1.0.1:
- bounded WAL payloads,
- crash-tail truncation before append.

Additional hardening:
- page format v1 CRC32 checksum,
- structural common-header and heap slot validation,
- legacy 1.5 page read compatibility with upgrade-on-write,
- partial trailing page normalization on page-file open,
- checked page-offset arithmetic,
- no `std::sync::Mutex::unwrap()` in the page store,
- B+Tree node count/bounds/shape/order validation,
- panic-free B+Tree payload reads/writes,
- CRC/version/length protected B+Tree root metadata,
- CRC/version/length protected checkpoint metadata,
- temp-file fsync -> rename -> directory fsync metadata publication,
- buffer closure lifetime protected by the pool mutex without panic-leakable manual pin counters.

Still intentionally deferred:
- dual-slot checkpoint/root fallback,
- page free-list / compaction,
- heap garbage reclamation,
- persistent Version Store (Milestone 1.6).
