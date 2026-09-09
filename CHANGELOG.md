# Changelog

## 1.5.2
- Based on Milestone 1.5.1 Hardened; no new database-engine functionality.
- Added Rust-only `examples/demo` covering MVCC, persistence, checkpoint/recovery, B+Tree persistence and page CRC validation.
- Added Rust-only `examples/benchmark` baseline for B+Tree, durable commits, persistent Current reads, historical reads and reopen/recovery.
- Added machine-readable benchmark JSON output and `examples/results` convention.
- Added detailed `TASKS-1.5.2.md` implementation sequence from the 1.0.2 contract to 1.5.x physical storage.

## 1.5.1
- Hardened the Milestone 1.5 persistent-storage architecture.
- Added page CRC/version validation and structural checks.
- Hardened B+Tree decoding and root metadata.
- Hardened checkpoint publication and page-file crash-tail behavior.
- Retained bounded WAL and WAL crash-tail handling inherited from 1.0.1.

## 1.5
- Added persistent fixed pages, BufferPool, persistent Current heap, primary B+Tree and checkpoint + WAL recovery.

## 1.0.2
- Added Rust-only demo and benchmark baseline over the 1.0.1 hardened transactional/WAL engine.
