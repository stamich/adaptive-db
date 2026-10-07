# Adaptive DB Changelog

This changelog summarizes the evolution of the repository from the original Milestone 1.0 through
Milestone 2.0.3. Hardened patch releases preserve the functional scope of
their parent milestone and focus on correctness, crash safety, corruption handling, documentation,
and build reproducibility.

## 2.0.3 — Log as source of truth, native CDC

### Architecture
- The canonical log is the source of truth and is never pruned. The current and version stores are
  projections: updated in memory, persisted by checkpoints, rebuildable with
  `Database::rebuild_projections`. `prune_wal_before_checkpoint` was removed.
- New crate `adb-journal`: atomic multi-file publication used by checkpoints (redo journal /
  doublewrite) plus the shared checksummed metadata envelope.
- No-steal buffer pool with lazy page allocation: projection files always equal the last
  checkpoint; torn pages cannot occur.
- Recovery replays only the log after the checkpoint (`WalCursor`), instead of reading the whole log.

### Fixed
- Commits no longer force four page files and the checkpoint to disk; one WAL `fsync`, shared by
  concurrent committers (group commit, `fsync` outside the appender lock).
- A failure after the first log append poisons the instance and returns `CommitOutcomeUnknown`
  instead of leaving memory and log silently diverged.
- Current heap no longer leaks: updates free the old tuple; slotted pages reuse slots and compact;
  a free-space map finds room. `Database::vacuum` removes tombstones below the oldest live snapshot.
- Table scans read only the entity's key range (`PhysicalPlan::EntityScan`) and stream in pages
  instead of materializing every row of every table.
- `Database::get` no longer reads applied-but-not-yet-durable commits.

### Added
- `IsolationLevel::Serializable` (default; read-set validation, no write skew) and `Snapshot`.
- Native change data capture: `read_changes` (commit order, before/after images, entity filters,
  durable prefix only), `change_feed_end`, durable named consumer offsets.
- C ABI v3: `adb_read_changes_json`, `adb_change_feed_end`, `adb_commit_consumer_offset`,
  `adb_consumer_offset`, `adb_checkpoint`, `adb_vacuum`; statuses `POISONED` (8) and
  `LOG_TRUNCATED` (9); `adb_close` checkpoints. Java binding updated (`NativeDatabase`).
- Scala planner emits `EntityScan` for table scans (`{"op":"entity_scan","entity_id":N}`).
- `DatabaseOptions` (buffer pages, checkpoint dirty-page budget, segment size).
- Checkpoint format 3; format-2 checkpoints of 2.0.2 databases are migrated on open.

### Refactored
- One generic `BPlusTree<K: TreeKey>` replaces the duplicated current/version trees (same on-disk
  node layout) and adds range scans, floor lookups and removes.
- `adb-tx` no longer depends on storage (validation takes a lookup function).
- Log format has one owner (`committed.rs`: `CommittedTx` ⇄ records, `TxAssembler`).
- Removed dead code: single-file WAL reader/writer, in-memory `CurrentStore`/`VersionStore`/`Stores`.
- Generated "Implements the `x` operation" doc comments replaced with real documentation (Rust,
  Scala physical-plan module).

### Tests and tooling
- Rust tests: 30 → 113 (crash/recovery, interrupted checkpoint, poisoning, corruption repair,
  group commit, write skew, CDC, vacuum, scans, page reuse, journal protocol).
- `examples/rust-benchmark` (2.0.3): commit throughput 1 vs N threads, churn, CDC, recovery.
- `examples/jvm-cdc-smoke`: Java FFM → C ABI → engine end-to-end check with plain `javac`.
- `clippy -D warnings` and `rustdoc -D warnings` clean.

### Compatibility
- Engine 0.2.3, native ABI 3 (JVM checks it). Page, B+Tree and WAL formats unchanged; checkpoint
  format 3 (format 2 migrated). `Database::get_in_tx` takes `&mut Transaction`;
  `DbError::TransactionConflict` carries the `Conflict`.
- Root-level validation logs, audits and build scripts of earlier milestones removed (available in git history).

## 2.0.1 — Hardened SQL/JVM control plane + demo

### Added
- chronological demo covering features introduced from 1.0.1 through 2.0.1,
- `demo/run-demo.sh`, feature map, historical walkthroughs and SQL tour,
- full Rustdoc and Scaladoc audits,
- bounded SQL input/token/nesting limits,
- bounded JVM result materialization,
- SHA-256 checked and fsync-safe `FileCatalog`,
- stricter Java ADB Batch decoder,
- bounded mutation/plan/path inputs at the FFI boundary,
- root `CHANGELOG.md`.

### Corrected in the demo build
- Scala upgraded from 3.3.3 to **3.3.8** in every Scala Gradle module,
- Scala bytecode target changed from 21 to **22**, matching the JDK 22 Java/FFM toolchain,
- `adb-wal` hardening tests now declare `tempfile` as a dev dependency,
- checkpoint restore uses an explicit `Checkpoint` type so the framed/legacy bincode branches infer consistently,
- temporal B+Tree corruption test imports `version_codec::decode_version_node`,
- point-lookup RowId wire encoding is lossless for the complete `u128` domain: canonical JSON uses a decimal string and Rust also accepts the legacy small numeric form,
- `PhysicalPlanner` separates logical and physical namespaces with `LPlan` / `PPlan` aliases instead of ambiguous companion wildcard imports,
- generated hardened Rust sources were normalized for rustfmt-style readability.

### Compatibility
- Milestone label remains `2.0.1`,
- engine/JVM version remains `0.2.0+hardening.1`,
- native ABI remains **2**,
- no Milestone 2.1+ query features are imported.

## 2.0 — SQL and JVM control plane
- introduced Scala domain/model modules, catalog, SQL tokenizer/parser, binder, logical plan, optimizer, physical plan and gateway,
- introduced Java FFM adapter over the native Rust ABI,
- added SQL DDL/DML/query path into the Rust execution engine,
- added metadata and mutation C ABI functions,
- kept the Rust storage/execution engine as the source of persistence truth.

## 1.7.1 — Hardened execution/FFI
- inherited the 1.6.1 storage/WAL hardening,
- bounded physical-plan JSON and ADB Batch wire sizes,
- checked wire-length conversions,
- initialized output handles deterministically,
- contained Rust panics at the C ABI boundary,
- documented raw-pointer/opaque-handle lifetime limitations.

## 1.7 — Physical execution + C ABI
- added `adb-execution`, physical plans and operators,
- added point lookup, scan, filter, project and limit execution,
- added `RecordBatch` and ADB Batch Format v1,
- added query cursor/cancellation/metrics foundations,
- added first C FFI database/query lifecycle.

## 1.6.1 — Hardened persistent MVCC
- repaired segmented-WAL crash tails before append,
- bounded WAL frame allocation,
- validated segment continuity and interior truncation,
- hardened current and temporal B+Tree codecs against corrupt counts/ranges,
- added traversal depth/cycle protections,
- added page checksums and stronger page/slot validation,
- made root/checkpoint metadata CRC/version/length framed and fsync-safe.

## 1.6 — Persistent Version Store
- added persistent historical/version storage,
- added temporal `VersionKey` B+Tree,
- persisted MVCC history across restart,
- split Current Store and Version Store checkpoint frontiers,
- extended recovery to replay historical versions idempotently.

## 1.5.1 — Hardened page/current storage
- inherited 1.0.1 WAL hardening,
- added page CRC and structural validation,
- normalized incomplete page-file tails,
- made B+Tree decoding bounds-checked and panic-resistant,
- hardened checkpoint/root metadata publication,
- removed panic-sensitive manual buffer pin accounting.

## 1.5 — Persistent Current Store
- added fixed 16 KiB pages, page store and BufferPool,
- added slotted heap pages,
- added persistent primary B+Tree,
- persisted Current Store independently of WAL replay,
- added checkpoint metadata and restart/recovery integration.

## 1.0.1 — Hardened WAL/recovery
- bounded serialized WAL records,
- rejected oversized persisted frame lengths before allocation,
- detected the last complete WAL frame,
- truncated incomplete crash suffixes before reopening for append,
- retained strict CRC/magic/version failures for fully framed corruption.

## 1.0 — Initial transactional storage foundation
- introduced core identifiers, values and rows,
- added transaction manager and snapshot-based MVCC semantics,
- added Current Store and Version Store abstractions,
- introduced write-ahead logging with CRC-framed records,
- implemented commit ordering as WAL durable before state visibility,
- added restart recovery that applies committed transactions only.
