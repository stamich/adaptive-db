# Milestone 1.6.1 — Security, Durability and Recovery Review

## Fixed high-severity correctness/resilience issues

### Segmented WAL crash tail
The original 1.6 reader tolerated an incomplete final segment suffix, but the writer reopened
at physical EOF. New commits could therefore be appended behind damaged bytes and become
invisible on a later restart. 1.6.1 scans the newest segment to its last complete frame and
truncates only the incomplete suffix before append.

### Unbounded WAL payload allocation
Persisted `payload_len` is now capped at 16 MiB before allocating. This prevents a corrupted
header from requesting multi-gigabyte memory.

### Missing/non-tail WAL segments
All retained segment ids must be contiguous. A partial frame is tolerated only in the newest
segment; truncation in an older segment is corruption.

### Temporal B+Tree disk corruption
Both RowId and `(RowId, CommitTs)` codecs now validate count, encoded byte range, node shape,
and strictly increasing key order. Persisted counts no longer drive unchecked slicing or
`Vec::with_capacity` without a maximum.

### B+Tree cycles and invalid roots
Root ids are checked against physical page count. Descent and leaf-chain scans are bounded by
page count, and recursive inserts have a hardened depth ceiling, preventing corrupted pointers
from spinning forever or recursively overflowing the stack.

### Page-file integrity
Hardened pages carry a format marker and non-zero CRC. Free-space bounds and heap slot ranges
are checked before higher-level decoding. An incomplete trailing physical page is removed on
open. Legacy pre-hardened pages remain readable after structural validation and are upgraded
on write.

### Root/checkpoint crash consistency
B+Tree root metadata and checkpoint v2 now have magic/version/length/CRC envelopes and use:
`write temp -> fsync temp -> rename -> fsync parent directory`.

### Logical WAL state
Recovery rejects mutation/version/commit/abort records without a preceding BEGIN, duplicate
transaction reuse, and invalid historical intervals.

## Positive properties retained
- WAL is synchronized before Current/Version pages become durable.
- Version Store is independently persistent.
- Current and Version checkpoint frontiers remain independent.
- Recovery remains logically idempotent.
- Production Rust code contains no `unsafe` block in Milestone 1.6.1.

## Residual risks / intentional scope limits
- Recovery still materializes all retained WAL records in memory; very large retained WAL can
  produce high restart memory usage.
- Integrity verification and statistics materialize large index/history sets and are not
  streaming operations.
- Root/checkpoint metadata has one active file, not a dual-slot previous-generation fallback.
- CRC32 detects accidental corruption; it is not a cryptographic MAC against a filesystem
  attacker capable of rewriting data and checksum together.
- The Current/Version heaps are append-oriented and do not reclaim obsolete physical records.
- B+Trees use coarse tree mutexes and do not implement delete/merge.
- `TransactionManager::publish_commit` still contains an internal monotonicity assertion; live
  commit timestamp allocation makes it unreachable under valid engine state, but it remains an
  invariant panic rather than a typed error.
- No user authentication/encryption exists: 1.6 is an embedded storage-engine milestone.
