# Adaptive DB Changelog

This changelog summarizes the evolution of the repository from the original Milestone 1.0 through
Milestone 2.2.3. Hardened patch releases preserve the functional scope of
their parent milestone and focus on correctness, crash safety, corruption handling, documentation,
and build reproducibility.

## 2.2.3 — Statistics and cost-based optimization

Implements the 2.2 plan (with the identical-workload benchmarks of 2.2.2 and the limits of
2.2.1) on top of 2.1.3, as adapted in `docs/milestone-2.2.3.md`. Statistics are stored natively
by the engine; foreign-key hints are included; order-preserving key encoding is deferred to 2.3.

### Added (Rust)
- New crate `adb-stats`: `TableStatistics` / `ColumnStatistics` (JSON document, format 1, 4 MiB
  bound), `AnalyzeOptions` with limits, and the collector: exact row/NULL/min/max/width counts,
  distinct values exact up to 10,000 then HyperLogLog (p = 12), a seeded reservoir sample
  (30,000 rows) for equi-depth histograms (≤ 64 buckets) and most common values (≤ 32).
- Engine: `Database::analyze`, `statistics`, `modifications_since_analyze`; documents in
  `stats/entity-<id>.stats` (checksummed, atomic replace, unreadable = absent); per-entity
  modification counters updated by the commit apply step, published with the projections
  through the checkpoint journal (`stats/modifications.meta`), replayed after a crash and
  recounted by `rebuild_projections`; `DbError::Statistics`.
- Execution: `OperatorProfile.node_id` (plan pre-order), `PhysicalPlan::node_count`.
- C ABI 5: `adb_analyze_entity_json`, `adb_statistics_json` (`NOT_FOUND` if never analyzed),
  `adb_statistics_generation`, `adb_modifications_since_analyze`; invalid options → `INVALID_ARGUMENT`, ANALYZE limits →
  `RESOURCE_LIMIT`.
- `adb-benchmark-rust --bin workloads` (`rust_native` path of the 2.2.3 workloads).

### Added (JVM)
- Module `adb-statistics`: statistics model, strict `StatisticsCodec`, `EntityStatistics`
  (freshness: stale above 20% changed rows; confidence), `StatisticsProvider`.
- SQL: `ANALYZE [table]`, `SET optimizer = cost | rule`, `REFERENCES t(c) NOT ENFORCED`;
  catalog `Field.references` (`ForeignKeyRef`), validated and persisted by `FileCatalog`.
- Optimizer: `CardinalityEstimator` (PK, MCV, NDV, histogram, min/max, AND/OR/NOT, foreign-key
  and NDV joins, LEFT JOIN, aggregates; confidence and source; EXPLAIN basis), `CostModel`
  (`Cost`, `CostWeights`, saturating arithmetic, engine-limit checks), `OptimizerConfig` /
  `EngineLimits`, `JoinGraph` and `JoinReorderRule` (DP ≤ 10, greedy ≤ 32, SQL order beyond).
- Physical planning: `CostBasedPolicy` (strategy and build side by cost and feasibility,
  INNER input swap, warnings), `JoinChoice`, `NodeEstimate` per pre-order node id,
  statistics warnings in `PlannedQuery`.
- Gateway: cost mode by default and rule mode; EXPLAIN with estimates, join order, statistics
  section and warnings; EXPLAIN ANALYZE with estimate and q-error per operator;
  `EngineStatisticsProvider` (documents cached by generation, undecodable = missing); `PlannerFeedbackLog`
  (`planner-feedback.jsonl`, bounded, rotating, SQL fingerprinted).
- Java binding: `NativeDatabase.analyzeJson` (runs outside the object monitor; `close()` waits
  for it), `statisticsJson`, `statisticsGeneration`, `modificationsSinceAnalyze`.
- JVM benchmark `--workloads`: `ffi_prepared_plan` and `scala_cbo_ffi_rust` paths, join-order
  workload in cost and rule mode, cost calibration. Demo tour phase 2.2.3.
- `scripts/benchmark-db.sh`, `scripts/benchmark-ffi.sh`, `build-milestone-2.2.3.sh`.
- Extended workload benchmark: one Rust-defined dataset of 10 tables at a scale factor
  (`ADB_BENCH_SCALE`; the Rust binary prepares the database the JVM opens), time-based warm-up
  and p99, planning time separated, peak memory and rows/s, a star schema whose SQL order
  exceeds the memory budget at scale 10, estimation stress (Zipf, correlation, stale statistics),
  planner timing for 2–12 relations, calibration over every plan, and
  `scripts/check-benchmarks.py` for the invariants of the results.
- GitHub Actions: `ci.yml` (Rust, JVM, integration with a checked benchmark smoke run on every
  push to master and pull request), `benchmarks.yml` (manual, scale 1 or 10), Dependabot.
- The join-order DP forms only connected subsets (10-relation joins planned in milliseconds
  instead of ~250 ms).

### Changed
- `PlanningPolicy.chooseJoin` returns a `JoinChoice`; `chooseTopK` also receives its input;
  `JoinRequest` carries the logical join.
- `JsonReader` moved from the gateway to `adb-model`; it bounds nesting, reads integers beyond
  `Long` as `BigInt` and rejects invalid escapes.
- Engine, crates and JVM build report version 2.2.3; C ABI 5 (`EXPECTED_ABI = 5` in Java).

### Tests
- Rust: 158 → 186 (collector accuracy and limits, persistence, counters across crash /
  checkpoint / rebuild, ABI 5, profile node ids). JVM: 40 → 90, including DP equal to
  exhaustive enumeration on random graphs and result equality of reordered plans (INNER, LEFT,
  CROSS) under a reference interpreter.

## 2.1.3 — Relational execution

Implements the relational-execution plan of "Milestone 2.1.2" on top of 2.0.3, with the changes
described in `docs/milestone-2.1.3.md`.

### Added (Rust)
- Slot-addressed execution: `SlotId`, `ScanColumn`, `ExecRow` (a `Vec<Value>` indexed by slot);
  leaf nodes map stored fields to slots, everything above them works on slots only.
- Operators `HashJoin` and `NestedLoopJoin` (INNER / LEFT; cross join), `Aggregate`
  (GROUP BY; COUNT(*), COUNT, SUM, MIN, MAX, AVG), `Sort`, `TopK`.
- `PhysicalPlan::validate` returns the plan shape and enforces depth, list, slot and LIMIT/TopK
  limits plus slot consistency (no slot produced twice, every slot read is produced by the input).
- `MemoryTracker` / RAII `MemoryReservation` (per-query budget), `ExecutionLimits` (materialized
  rows, join fanout, nested-loop comparisons), `collect_all_bounded`.
- Checked INT64 aggregation (`i128` accumulation, `ExecutionError::ArithmeticOverflow`).
- Per-operator runtime profiles (`QueryCursor::profile`, `OperatorProfile`, `QueryProfile`);
  `QueryMetrics::source_rows` is maintained.
- New crate `adb-plan-wire`: plan wire v2 envelope, 8 MiB bound, version check before decoding,
  unknown fields rejected, validation after decoding.
- C ABI 4: `adb_query_profile_json`, statuses `RESOURCE_LIMIT` (10) and `ARITHMETIC_OVERFLOW` (11),
  consistent execution-error mapping; batch format v2 (flags word, optional row ids, slot column ids).
- `examples/rust-demo` (`adb-rust-demo`); relational sections in `examples/rust-benchmark`
  (renamed `adb-benchmark-rust`).

### Added (JVM)
- Model: `RelationId`, `SlotId`, `AggregateFunction`, `ColumnOrigin`, `Attribute`.
- SQL: INNER / LEFT [OUTER] / CROSS JOIN, table aliases, qualified columns, aggregate calls,
  select aliases, GROUP BY, ORDER BY ASC/DESC; parser budgets.
- `SelectBinder`: relation and alias scope, dense slot allocation on first reference, ambiguity
  and visibility errors, GROUP BY validation, aggregate result types, hidden ORDER BY aggregates,
  LIMIT ≤ 1,000,000.
- Relational logical plan and EXPLAIN rendering; `PredicatePushdownRule`; `PointLookupRule` works on
  join inputs and keeps remaining conjuncts.
- `PlanningPolicy` / `DefaultPlanningPolicy` with `PlanDecision`s (join strategy, TopK) shown by
  EXPLAIN; EXPLAIN ANALYZE shows the native runtime profile.
- Java binding: ABI 4, batch v2 decoder, `NativeRecordBatch.column(slot)`, `NativeQuery.profileJson()`.
- Demo tour phase 2.1.3; JVM benchmark relational stages.

### Changed
- `adb_core::SlotId` (heap tuple index) renamed to `TupleSlot`.
- Plans must be plan wire v2; v1 bare plans are rejected with a version message.
- Engine, crates and JVM build report version 2.1.3 (previously 0.2.x).
- `build-milestone-2.1.3.sh` replaces `build-milestone2.0.3.sh`; `demo/run-demo.sh` uses the
  Gradle wrapper.
- `include/adb.h`: removed an accidental second copy of the header.
- Java `BatchDecoder`: columns that mix NULL and non-NULL values decode correctly (`List.copyOf`
  rejected the NULLs; latent before 2.1, common with LEFT JOIN and NULL group keys).
- `RIGHT` / `FULL` / `NATURAL JOIN` are rejected with a clear message instead of being misread as
  an alias followed by an INNER JOIN; selecting a column or aggregate twice projects its slot once.

### Tests
- Rust: 113 → 158 (plan validation, joins, aggregation/sort/TopK, memory tracker, profiles, plan
  wire, ABI). JVM: 13 → 40. A shared fixture pins the plan wire format from both the Scala encoder
  and the Rust decoder.

### Compatibility
- Native ABI 4 (the JVM refuses 3); plan wire 2; batch format 2. Page, B+Tree, WAL and checkpoint
  formats unchanged: 2.0.3 databases open as they are.

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
