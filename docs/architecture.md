# Architecture — Milestone 2.4.0

Milestone 2.1.3 added relational execution (joins, aggregation, sorting) on top of the 2.0.3
storage architecture below. 2.2.3 added optimizer statistics owned by the engine and a
cost-based planner in the JVM. 2.3.0 adds secondary indexes as a third kind of projection, an
order-preserving key encoding (data format 2), and cost-based access paths. 2.4.0 moves the
catalog into the engine: versioned schemas are a fourth projection of the log, changed by
`ALTER TABLE` without rewriting rows. See
[Query path](#query-path-milestone-21),
[Statistics and cost-based planning](#statistics-and-cost-based-planning-milestone-223),
[Secondary indexes](#secondary-indexes-milestone-230) and
[Schema evolution](#schema-evolution-milestone-240).

## The one decision everything follows from

**The canonical log is the source of truth.** The current store and the version store are
*projections* of the log: they are updated in memory, persisted only by checkpoints, and can be
rebuilt from the log at any time. The log is never pruned by the engine.

Milestone 2.0.2 had it the other way round (stores forced to disk on every commit, WAL used only
as redo and pruned after checkpoints). That made a change feed, projection rebuilds and replay
impossible, and paid several `fsync`s per commit. 2.0.3 reverses it.

```text
                         Database (adb-engine)
   begin/get/commit ─────────────┐        read_changes ─────────────┐
                                 ▼                                   ▼
                  ┌──────────────────────────────┐      ┌──────────────────────────┐
                  │ commit pipeline              │      │ change feed (cdc)        │
                  │ validate → append → apply    │      │ WalCursor + TxAssembler  │
                  │ → group fsync → publish      │      │ durable prefix only      │
                  └──────┬──────────────┬────────┘      └────────────┬─────────────┘
                         │ append       │ apply (memory)              │ read
                         ▼              ▼                             │
     ┌────────────────────────┐   ┌──────────────────────────────┐    │
     │ CANONICAL LOG (adb-wal)│   │ PROJECTIONS (adb-storage)    │    │
     │ segments, never pruned │◄──┼─ rebuild_projections / replay│    │
     │ BEGIN VERSION* PUT/DEL │   │ current: B+Tree + heap (FSM) │    │
     │ COMMIT  (one run / tx) │───┼─►version: B+Tree + heap      │    │
     └───────────▲────────────┘   └──────────────┬───────────────┘    │
                 └───────────────────────────────┼────────────────────┘
                                                 │ checkpoint (dirty pages + roots + record)
                                                 ▼
                                 ┌──────────────────────────────────┐
                                 │ CHECKPOINT JOURNAL (adb-journal) │
                                 │ redo/doublewrite, atomic publish │
                                 └──────────────────────────────────┘
```

## Crates (dependency order)

| Crate | Responsibility |
|---|---|
| `adb-core` | Ids (`RowId::compose/entity_id/primary_key`), rows, values, `KeyRange`. No I/O. |
| `adb-journal` | Atomic multi-file publication (checkpoint journal) and the checksummed metadata envelope. |
| `adb-page` | 16 KiB checksummed pages; slotted heap pages with delete, slot reuse and compaction. |
| `adb-buffer` | **No-steal** page cache; lazy page allocation; dirty pages leave memory only via a checkpoint. |
| `adb-btree` | One generic `BPlusTree<K: TreeKey>` (get, floor, range scan, insert, remove, journaled persistence). `BTree` and `VersionBTree` are type aliases. |
| `adb-storage` | Heap with free-space map, current store, version store, checkpoint record v3, integrity check. |
| `adb-schema` | Versioned entity schemas (2.4.0): changes, validation, `RowAdapter`, `SchemaRegistry`, `CatalogEvent`. |
| `adb-wal` | Segmented log writer, forward `WalCursor` from any record boundary; catalog records since 2.4.0. |
| `adb-tx` | Transactions, isolation levels, OCC validation, snapshot registry. Storage-agnostic. |
| `adb-execution` | Slot-addressed, pull-based operators: scans, filter, project, limit, hash and nested-loop joins, aggregate, sort, TopK; plan validation; query `MemoryTracker`; runtime profiles. |
| `adb-plan-wire` | Versioned JSON plan wire format (v4 since 2.4.0, carrying the catalog epoch): bounded, version-checked, strict decoding plus validation. |
| `adb-stats` | Optimizer statistics: document model, `ANALYZE` options and limits, collector (exact counts, HyperLogLog, reservoir sample, histograms, most common values). |
| `adb-engine` | `Database`: commit pipeline, recovery, checkpoints, vacuum, change feed, consumer offsets, `ANALYZE`, statistics store, modification counters. |
| `adb-ffi` | C ABI v7 (`include/adb.h`). |

## Write path

Under the **commit lock**:

1. **Validate** (`adb_tx::validate`): every written row unchanged since the snapshot; under
   `Serializable` (default) also every row *read*. Conflicts abort with no side effects.
2. **Allocate** a commit timestamp.
3. **Append** the transaction to the log as one contiguous run
   `Begin, Version* (before-images), Put/Delete*, Catalog*, Commit` — not yet durable. (Since
   2.4.0 every `Put` of a table with a schema was first stamped, completed and validated by the
   schema registry; a DDL statement is a transaction of `Catalog` records alone.)
4. **Apply** it to the projections in memory (catalog first, then history, then current state).

Outside the lock:

5. **Group commit**: `LogWriter::sync_through` makes the log durable up to the transaction's end.
   The `fsync` runs on a cloned file handle while only the durability lock is held, so other
   committers keep appending and share the next `fsync`.
6. **Publish** the timestamp to new snapshots (monotonic maximum; safe because a durable later
   commit implies durable earlier ones).
7. If the dirty-page budget is exceeded, **checkpoint**.

A failure in steps 3–5 means memory and log may disagree: the instance is **poisoned**
(`EngineHealth`) and the caller gets `CommitOutcomeUnknown`. Every later call fails with
`Poisoned` until the database is reopened, when recovery decides from the log.

## Read path

Readers never take the commit lock. A snapshot read at `ts` returns the current record if it was
committed at or before `ts`, otherwise the version whose interval `[begin, end)` contains `ts`.
Uncommitted and not-yet-durable writes are invisible because snapshots only use published
timestamps.

Scans are **key-range scans paged with a keyset cursor**: `EntityScan` reads exactly
`[entity << 64, (entity + 1) << 64)` from the primary index, `batch_size` rows at a time; nothing
is materialized beyond one batch. If the snapshot is older than the vacuum horizon, rows that only
survive in the version index (vacuumed tombstones) are merged in, so time-travel scans stay exact.

## Durability and recovery

* **No-steal buffer pool.** Projection pages modified after the last checkpoint never reach their
  files. On disk, every projection always equals the last checkpoint exactly.
* **Checkpoint** = one journal containing every dirty page of all four files, both B+Tree roots and
  the checkpoint record (`replay_from`, last ids, vacuum horizon). The journal's atomic rename is the
  commit point; then pages are written in place and the journal is deleted. Torn pages are
  impossible: a crash during the in-place writes is redone from the journal on the next open.
* **Recovery** = finish an interrupted journal, then replay committed transactions from
  `replay_from`. Cost is proportional to the log since the last checkpoint.
* **Repair** = `Database::rebuild_projections` deletes both projections and replays the whole log.
  Use it when an error reports `is_corruption()`.

## Space management

* Current heap: updates free the previous tuple; slotted pages reuse free slots and compact; a
  free-space map (rebuilt on open) finds room. Repeated updates keep the heap size constant.
* Tombstones: `Database::vacuum` removes delete tombstones committed at or before the oldest snapshot
  held by a live transaction (they are needed only for write-conflict detection). The deleted row's
  history stays in the version store.
* Version store and log: append-only. Retention/archival is future work (see roadmap).

## Isolation

| Level | Guarantees | Cost |
|---|---|---|
| `Serializable` (default) | Equivalent to running each transaction atomically at its commit point; no write skew. | Validates the read set. |
| `Snapshot` | First-committer-wins on writes; write skew possible. | Validates the write set only. |

Read-only transactions never abort. Snapshot leases (`SnapshotRegistry`) are released when a
transaction is committed, rolled back or simply dropped.

## Lock order

`commit lock → log writer → projections (RwLock) → modification counters → transaction manager`. The durability lock is
taken without the commit lock (group commit) and only nests the log writer briefly.

## Query path (Milestone 2.1)

```text
SQL ─► SqlParser ─► SelectBinder ─► LogicalPlanner ─► RuleOptimizer ─► PhysicalPlanner ─► PlanJsonEncoder
        (JOIN, aliases,  (relation scope,   (canonical        (predicate        (PlanningPolicy:
         GROUP BY,        dense SlotIds,     relational        pushdown, point   HashJoin | NestedLoopJoin,
         ORDER BY)        GROUP BY rules)    tree)             lookups)          TopK; decisions + reasons)
                                                                                         │ plan wire v2
                                                                                         ▼
 JVM ◄── batch format v2 + profile JSON ─── C ABI v5 ◄── QueryCursor ◄── operators ◄── adb-plan-wire::decode_json
```

**Who decides what.** Scala owns meaning: name resolution, types, GROUP BY rules, join-condition
analysis, the choice of physical strategy. Rust owns execution and safety: it re-validates every
plan (limits, slot consistency), enforces memory and work budgets at runtime, and reports what
actually happened. Neither side trusts the other's input blindly; the wire format is pinned by a
cross-language contract test.

**Slots.** Every column of every relation *instance* gets its own query-scoped slot, allocated on
first reference by the binder. Self-joins and aliases therefore never collide, scans read only
referenced fields, and native rows are plain arrays (see [execution.md](execution.md)).

**Adaptive loop.** Adaptive DB is meant to adapt its physical execution to the workload and to
the user's declared intent. 2.1.3 establishes both halves of that loop without guessing at
statistics it does not have yet:

* *Decide and explain:* every strategy choice (join algorithm, build side, TopK vs. sort) goes
  through a replaceable `PlanningPolicy` and is recorded as a `PlanDecision` with its reason,
  shown by `EXPLAIN`. Since 2.2.3 the default is the statistics-driven `CostBasedPolicy` (the
  rule-based policy remains as `SET optimizer = rule`); workload or intent advisors (5.x)
  replace the policy, not the planner.
* *Observe:* every operator reports rows, time, memory and operator-specific counters
  (`adb_query_profile_json`, shown by `EXPLAIN ANALYZE`), so the same plan can be compared with
  what it actually cost.

**Bounded by construction.** Blocking operators reserve memory from a per-query
`MemoryTracker` before holding it, materialization and join fanout are capped, nested-loop work is
capped, and INT64 aggregation is checked. A runaway query fails with `RESOURCE_LIMIT` or
`ARITHMETIC_OVERFLOW`; the database is never affected.

## Statistics and cost-based planning (Milestone 2.2.3)

```text
            ANALYZE                                 commit apply
               │                                         │
     adb-stats::analyze (snapshot scan)        modification counters ──► checkpoint journal
               │                                         │                 (stats/modifications.meta)
     stats/entity-<id>.stats ◄── atomic replace          │
               │                                         │
               └──── C ABI 5: adb_statistics_json, adb_modifications_since_analyze ────┐
                                                                                        ▼
 JVM: StatisticsProvider ─► CardinalityEstimator ─► CostModel ─► JoinReorderRule + CostBasedPolicy
                                                                       │ estimates by pre-order node id
                                                                       ▼
          EXPLAIN (estimates, decisions, statistics, warnings)   EXPLAIN ANALYZE ◄── profile node_id
                                                                       │ q-error
                                                                       ▼
                                                           planner-feedback.jsonl
```

Statistics are derived data, like the projections. They are collected and persisted by the engine,
because only the commit pipeline can count the changes that make them stale. The counters travel
with the projections through the checkpoint journal and log replay. The JVM decides how to use
them: estimates, costs, join order and build sides. Every decision stays explained, and every
estimate is compared with what the engine measured. That comparison closes the adaptive loop:
2.1.3 could decide and observe, and 2.2.3 can now also predict and measure how good the
prediction was. Details: [statistics.md](statistics.md), [optimizer.md](optimizer.md).

## Secondary indexes (Milestone 2.3.0)

```text
 commit apply ─► current store ─┬─► version store                (before-images)
                                └─► IndexProjection per index    (entries with begin_ts/end_ts)
                                        │ ByteTree (variable-length byte keys, keyset scans)
 checkpoint ─► journal publishes all projections + indexes/catalog.meta together
                                        │
 plan wire v3: index_scan ─► IndexScanOperator (pages, snapshot re-check) ─► RowFetchOperator
                                        ▲                                      (sorted batch lookups)
 JVM: IndexCatalog (C ABI 6) ─► IndexPredicateMatcher ─► AccessPathEnumerator ─► CostModel.accessCost
                                                       ─► CostBasedPolicy (index vs scan, order)
```

An index follows the decision this architecture rests on: it is a projection of the log. It
writes no log records. It is maintained inside the commit apply step from the same row images
as the other projections, published by the same checkpoint, replayed by recovery and rebuilt by
`rebuild_projections`. Its entries are versioned with the same `begin_ts`/`end_ts` as rows, so
any snapshot the index serves reads it exactly as it reads the table: `AS OF` included, until
`VACUUM` trims the history. Keys use the order-preserving encoding of `adb_core::key_encoding`.
Since data format 2, row ids of BIGINT primary keys use it as well, so primary-key order equals
storage order and a sequential scan can deliver `ORDER BY id`. The JVM decides when to use an
index. The engine checks that every scan page is served at the query snapshot. Details:
[indexes.md](indexes.md), [optimizer.md](optimizer.md#access-paths-230).

## Schema evolution (Milestone 2.4.0)

```text
 DDL ─► commit_catalog (commit lock): build event ─► apply to a copy of the registry ─► log Catalog record
                                      ─► SchemaRegistry (projection) ─► checkpoint: schema/registry.meta
 writes ─► prepare_write: stamp latest version, fill defaults, check ─► Put
 reads  ─► RowAdapter(row version → version at the read view's epoch) ─► scans, lookups, indexes, ANALYZE
 plans  ─► plan wire v4 {schema_epoch} ─► execute_planned refuses another epoch (SCHEMA_CHANGED)
 JVM: EngineCatalog (C ABI 7, cached per epoch) ─► Binder ─► planner (SchemaStatistics) ─► gateway replans once
```

The catalog follows the same decision: schemas are catalog records in the log and the registry
is a projection, published by the checkpoint journal and rebuilt from the log. DDL is therefore
transactional and durable together with the rows. Rows keep the version they were written under
and are adapted when read; nothing is rewritten by `ALTER TABLE`, which costs one log record on
any table. A query pins the catalog epoch it starts at, so DDL committing meanwhile never changes
the shape of its rows. Details: [schema.md](schema.md).

## Control plane

The JVM (Scala parser/binder/planner, Java FFM) keeps its role: everything up to the physical plan.
Since 2.0.3 a table scan is planned as `EntityScan` instead of `Filter(Scan, _entity_id = N)`; since
2.1 it carries a field-to-slot mapping of the referenced columns only.
