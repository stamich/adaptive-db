# Adaptive DB — Milestone 1

Minimal, single-node transactional storage engine in Rust.

## Scope

- logical WAL,
- CRC32 checksum per WAL record,
- crash recovery,
- Current Store,
- Version Store,
- Snapshot Isolation,
- read-your-writes,
- write-write conflict detection,
- temporal `get_at`,
- global serialized commit path (intentional and temporal simplification of V1).

## Out of scope

- SQL,
- persistent B+Tree,
- checkpointing,
- WAL segment rotation,
- Raft,
- distributed transactions,
- column/search/vector/graph projections.

## Run

```bash
cargo test --workspace
```

## The main invariant

Response `commit()` is returned after `fsync` of record `Commit` in WAL.
