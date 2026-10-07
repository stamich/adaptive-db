# Adaptive DB — Milestone 2.1.3

Adaptive DB is an adaptive, intent-driven database in which **the canonical log is the source of
truth** and every physical structure (the current store, the version store and, in the future,
columnar, search and graph projections) is a rebuildable projection of that log. Physical
execution is meant to adapt to the workload and to the user's declared intent; the engine
therefore explains every planning decision it takes and measures what each operator actually did.

This repository implements the V1 core: a single-node transactional engine with history, a native
change stream and, since 2.1.3, relational query execution (joins, aggregation, sorting).

## What 2.1.3 adds

| Area | 2.0.3 | 2.1.3 |
|---|---|---|
| SQL | single table: filter, project, limit | **JOIN / LEFT JOIN / CROSS JOIN**, aliases, qualified columns, **GROUP BY** with COUNT/SUM/MIN/MAX/AVG, **ORDER BY** |
| Column identity | storage `FieldId` | query-scoped **`SlotId`** per relation instance (self-join safe, scans read only referenced fields) |
| Operators | scan, filter, project, limit | + **HashJoin**, **NestedLoopJoin**, **Aggregate**, **Sort**, **TopK** |
| Resource safety | per-batch size limit | + query **memory tracker**, row/fanout/comparison caps, **checked INT64 aggregation**, plan validation |
| Planning | point-lookup rule | + **predicate pushdown**, strategy **`PlanningPolicy`** with **explained decisions** |
| Observability | — | **EXPLAIN ANALYZE** with the native per-operator **runtime profile** |
| Boundary | plan JSON v1, batch v1, C ABI 3 | **plan wire v2** (`adb-plan-wire`, versioned, strict), **batch v2**, **C ABI 4** |
| Version | engine 0.2.3 | engine, crates and JVM build **2.1.3** |
| Tests | 113 Rust, 13 JVM | **158 Rust, 40 JVM** (plus a cross-language plan-wire contract) |

The storage engine of 2.0.3 (log as source of truth, journaled checkpoints, group commit,
serializable isolation, native CDC) is unchanged. How the 2.1.2 plan was adapted, and why:
[docs/milestone-2.1.3.md](docs/milestone-2.1.3.md).

Relational benchmark (`examples/rust-benchmark`, 5,000 fact rows, 100 groups, 2-vCPU cloud VM;
expect run-to-run variation): hash join 5,000 → 5,000 rows ≈ 3–5 ms, nested-loop join
1,000 × 100 pairs ≈ 10 ms, GROUP BY + SUM ≈ 2–6 ms, full sort ≈ 3 ms, TopK 10 ≈ 2 ms,
join + aggregate + TopK ≈ 3–5 ms; plan wire v2 decode + validate ≈ 125,000 plans/s.
Recorded run: [examples/results/2.1.3-rust-local.json](examples/results/2.1.3-rust-local.json).

## Architecture

```text
SQL -> Scala: parse -> bind (slots) -> logical plan -> optimize -> physical plan (+ decisions)
    -> plan wire v2 JSON -> Java FFM -> C ABI v4 -> Rust: validate -> operators -> batch v2
                                                                   -> runtime profile JSON

  commit: validate -> append to LOG -> apply to projections (memory) -> group fsync -> publish
  read:   snapshot over current + version projections; EntityScan = key-range scan
  CDC:    WalCursor over the durable log prefix
  checkpoint: dirty pages + roots + record -> journal -> in place
```

Details: [docs/architecture.md](docs/architecture.md) (including the query path and the adaptive
loop), execution layer: [docs/execution.md](docs/execution.md), invariants mapped to tests:
[docs/invariants.md](docs/invariants.md), CDC: [docs/cdc.md](docs/cdc.md), ABI:
[docs/ffi.md](docs/ffi.md), wire formats: [docs/plan-wire-format.md](docs/plan-wire-format.md),
[docs/batch-format.md](docs/batch-format.md), roadmap: [docs/roadmap.md](docs/roadmap.md).

## SQL (JVM layer)

```sql
CREATE TABLE customer (id BIGINT PRIMARY KEY, name STRING NOT NULL, city STRING);
CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL, amount BIGINT NOT NULL);
INSERT INTO customer VALUES (1, 'Ada', 'Krakow');
INSERT INTO orders VALUES (10, 1, 120);
UPDATE orders SET amount = 130 WHERE id = 10;
DELETE FROM orders WHERE id = 10;

SELECT c.name, o.amount FROM customer c JOIN orders o ON o.customer_id = c.id;      -- HashJoin
SELECT c.name, COUNT(o.id) AS n FROM customer c LEFT JOIN orders o
  ON o.customer_id = c.id GROUP BY c.name;                                             -- LEFT JOIN
SELECT c.city, SUM(o.amount) AS total FROM customer c JOIN orders o ON o.customer_id = c.id
  GROUP BY c.city ORDER BY total DESC LIMIT 3;                                         -- TopK
SELECT a.id, b.id FROM orders a JOIN orders b ON a.customer_id = b.customer_id
  AND a.id < b.id;                                                                     -- self-join
SELECT * FROM customer AS OF VERSION 3 WHERE id = 1;                                   -- time travel
EXPLAIN ANALYZE SELECT ...;            -- plans, decisions with reasons, per-operator profile
```

The full grammar and its rules are in [docs/milestone-2.1.3.md](docs/milestone-2.1.3.md#supported-sql-213).

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

let (plan, _shape) = adb_plan_wire::decode_json(plan_json)?; // validated physical plan
let mut cursor = db.execute(plan)?;
while let Some(batch) = cursor.next_batch()? { /* columns by output slot */ }
let profile = cursor.profile();                            // per-operator runtime profile
db.close()?;                                               // checkpoint (optional)
```

A complete relational example: `cargo run --release -p adb-rust-demo`.

## Build and validation

```bash
./build-milestone-2.1.3.sh   # fmt, clippy (incl. docs lints), tests, rustdoc, Rust demo,
                             # benchmark, Java FFM smoke test, JVM tests + benchmark + demo
./demo/run-demo.sh           # feature tour 1.0.1 -> 2.1.3 through the JVM CLI
```

Requirements: Rust (stable) and JDK 22+. The Scala/JVM part uses the Gradle wrapper in `jvm/` and
needs access to Maven Central. The Java FFM → Rust path is also checked without Gradle by
`examples/jvm-cdc-smoke/run.sh`.

## Repository layout

```text
crates/        Rust engine (core, journal, page, buffer, btree, storage, wal, tx, execution,
               plan-wire, engine, ffi)
include/adb.h  C ABI v4
jvm/           Scala/Java: model, catalog, SQL, binder, optimizer, physical planning, FFM,
               gateway, CLI, benchmark
proto/         target binary plan contract
examples/      Rust demo and benchmark, JVM CDC smoke test, results
demo/          feature tour through the JVM CLI
docs/          architecture, execution, invariants, CDC, ABI, wire formats, milestone notes, roadmap
```
