# Adaptive DB — Milestone 2.0.3

Adaptive DB is a database in which **the canonical log is the source of truth** and every physical
structure (the current store, the version store and, in the future, columnar, search and graph
projections) is a rebuildable projection of that log. This repository implements the V1 core of
that design: a single-node transactional engine with history and a native change stream.

## What 2.0.3 changes

| Area | 2.0.2 | 2.0.3 |
|---|---|---|
| Source of truth | stores; WAL pruned after checkpoints | **the log, never pruned**; stores are rebuildable (`rebuild_projections`) |
| Commit durability | ~6 fsyncs per commit, globally serialized | one WAL fsync + **group commit**; store pages written only by checkpoints |
| Checkpoints / torn pages | pages written in place, unprotected | **atomic journal (doublewrite)** covering all files at once |
| Failure after the WAL write | client gets an error, memory diverges | **poisoning** + `CommitOutcomeUnknown`; recovery decides |
| Current heap | every UPDATE left a dead tuple | slot reuse, compaction, free-space map; **`vacuum()`** of tombstones |
| Table scan | scan of the whole database + filter, materialized in a `Vec` | **`EntityScan`**: the entity's key range, streamed in pages |
| Isolation | Snapshot (write skew possible) | **Serializable by default** (read-set validation), Snapshot optional |
| CDC | none | **native change stream** from the log: before/after images, cursors, filters, durable consumer offsets |
| C ABI | v2 | **v3** (CDC, checkpoint, vacuum, `POISONED` and `LOG_TRUNCATED` statuses) |
| B+Tree | two near-identical implementations | **one generic** `BPlusTree<K>` (same on-disk format) |
| Rust tests | 30 | **113** |

Measurements (`examples/rust-benchmark`, 5,000 rows per entity, same machine):

| Metric | 2.0.2 | 2.0.3 |
|---|---:|---:|
| single-row commit, 1 thread | 522 /s | ~3,500–3,900 /s |
| single-row commit, 8 threads | 459 /s | ~9,900 /s |
| bulk load (100 rows / tx) | 21,147 rows/s | 48,189 rows/s |
| scan of one entity out of 3 (5,000 rows) | 71 ms | 2.9 ms |
| heap pages after 2,000 updates of 100 rows | 1 → 7 | 1 → 1 |

## Architecture

```text
SQL -> Scala (parser, binder, planner) -> PhysicalPlan JSON -> Java FFM -> C ABI v3 -> Rust
                                                                                      |
  commit: validate -> append to LOG -> apply to projections (memory) -> group fsync -> publish
  read:   snapshot over current + version projections; EntityScan = key-range scan
  CDC:    WalCursor over the durable log prefix
  checkpoint: dirty pages + roots + record -> journal -> in place
```

Details: [docs/architecture.md](docs/architecture.md); invariants mapped to tests:
[docs/invariants.md](docs/invariants.md); CDC: [docs/cdc.md](docs/cdc.md); ABI:
[docs/ffi.md](docs/ffi.md); plan format: [docs/plan-wire-format.md](docs/plan-wire-format.md);
roadmap: [docs/roadmap.md](docs/roadmap.md).

## Usage (Rust)

```rust
use adb_core::{Row, RowId, Value};
use adb_engine::{ChangeCursor, ChangeFilter, Database};

let db = Database::open("data")?;

let mut tx = db.begin();                                   // Serializable
let id = RowId::compose(/* entity */ 1, /* pk */ 42);
if db.get_in_tx(&mut tx, id)?.is_none() {
    tx.put(id, Row::new().with_field(1, Value::Int64(100)));
}
let ts = db.commit(tx)?;                                   // durable when it returns

let old = db.get_at(id, ts)?;                              // time travel
let changes = db.read_changes(ChangeCursor::BEGINNING, 100, &ChangeFilter::entities([1]))?;
db.vacuum()?;
db.close()?;                                               // checkpoint (optional)
```

## Supported SQL (JVM layer, unchanged since 2.0)

```sql
CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING);
INSERT INTO account VALUES (1, 100, 'Alice');
SELECT id, balance FROM account WHERE balance > 50 LIMIT 10;   -- EntityScan
SELECT * FROM account AS OF VERSION 3 WHERE id = 1;            -- PointLookup at a snapshot
UPDATE account SET balance = 200 WHERE id = 1;
DELETE FROM account WHERE id = 1;
EXPLAIN SELECT * FROM account WHERE id = 1;
```

## Build and validation

```bash
./build-milestone2.0.3.sh          # fmt, clippy -D warnings, tests, rustdoc, benchmark, JVM smoke test
```

Requirements: Rust (stable) and JDK 22+. Gradle 9 and access to Maven Central are needed only for
the Scala/JVM tests and demo (`cd jvm && gradle clean test`, `./demo/run-demo.sh`). The Java
FFM → Rust path is checked without Gradle by `examples/jvm-cdc-smoke/run.sh`.

## Repository layout

```text
crates/        Rust engine (core, journal, page, buffer, btree, storage, wal, tx, execution, engine, ffi)
include/adb.h  C ABI v3
jvm/           Scala/Java: model, catalog, SQL, planning, FFM, gateway, CLI
proto/         target binary plan contract
examples/      Rust benchmark, JVM CDC smoke test, results
demo/          feature tour through the JVM CLI
docs/          architecture, invariants, CDC, ABI, wire formats, roadmap
```
