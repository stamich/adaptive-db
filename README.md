# Adaptive DB — Milestone 2.4.0

[![CI](https://github.com/stamich/adaptive-db/actions/workflows/ci.yml/badge.svg)](https://github.com/stamich/adaptive-db/actions/workflows/ci.yml)

Adaptive DB is an adaptive, intent-driven database in which **the canonical log is the source of
truth** and every physical structure (the current store, the version store, secondary indexes,
optimizer statistics and, in the future, columnar, search and graph projections) is rebuildable,
derived data. Physical execution is meant to adapt to the workload and to the user's declared
intent; the engine therefore explains every planning decision it takes, estimates what each
operator will do and measures what it actually did.

This repository implements the V1 core: a single-node transactional engine with history, a native
change stream, relational query execution (joins, aggregation, sorting), statistics and a
cost-based optimizer (2.2.3), secondary indexes with cost-based access paths (2.3.0) and, since
2.4.0, schema evolution: versioned schemas owned by the engine and changed without rewriting rows.

## What 2.4.0 adds

| Area | 2.3.0 | 2.4.0 |
|---|---|---|
| Catalog | JVM file (`catalog.properties`) | **owned by the engine**: schemas are catalog records in the log and a registry projection, so DDL is transactional, durable with the rows and the same for every client; a 2.3.0 catalog file is imported once |
| SQL | `CREATE TABLE`, indexes | + `ALTER TABLE ... ADD / RENAME / DROP COLUMN [RESTRICT]`, `DROP TABLE`, `SHOW SCHEMA t`, `DEFAULT literal` |
| Rows | stored as written | **versioned**: written rows are stamped, completed with defaults and validated; old rows are **adapted on read** (virtual defaults, hidden drops) and **migrated when written** — `ALTER TABLE` rewrites nothing |
| Consistency | — | **catalog epochs**: a query pins the schemas of its start; a plan of another epoch is refused (`SCHEMA_CHANGED`) and the gateway plans it again |
| Dependencies | — | RESTRICT: indexed columns, primary keys and referenced tables cannot be dropped; the JVM `DependencyGraph` explains why |
| Planning | statistics by field | + statistics of columns added after `ANALYZE` derived from their default; EXPLAIN **Schema** section |
| Change feed | row images | + `schema` / `drop_table` entries and the `schema_version` of every row change |
| Storage | data format 2 | **data format 3**, upgraded in place from format 2 (nothing rewritten; format 1 still via `adb-migrate`); statistics format 3 |
| Boundary | C ABI 6, plan wire v3 | **C ABI 7** (five schema functions, `adb_current_schema_epoch`, status `SCHEMA_CHANGED`), **plan wire v4** (`schema_epoch`) |
| Version | 2.3.0 | engine, crates and JVM build **2.4.0** |
| Tests | 258 Rust, 120 JVM | **320 Rust, 159 JVM** (incl. fuzzing of schema documents, catalog records and the adapter, and concurrent DDL, writes and scans) |

How the 2.4.0 proposal was implemented, and why: [docs/milestone-2.4.0.md](docs/milestone-2.4.0.md).
Schema evolution in depth: [docs/schema.md](docs/schema.md). The 2.3.0 additions (secondary
indexes, data format 2, access paths) are described in [docs/milestone-2.3.0.md](docs/milestone-2.3.0.md)
and [docs/indexes.md](docs/indexes.md).

**Upgrading from 2.3.0:** nothing to do. The engine upgrades the data directory in place on open,
and the gateway imports `catalog.properties` into the engine catalog at the first open (renaming
the file to `catalog.properties.imported`). Directories written before 2.3.0 still need
`cargo run --release -p adb-migrate -- /path/to/db` once.

Schema-evolution workloads (`scripts/benchmark-db.sh`, scale 1: tables of 50,000 rows; 2-vCPU
cloud VM, p50 after warm-up; expect run-to-run variation):

| | scale 1 |
|---|---|
| G: scan + filter on a column added later + aggregate — 0 % / 50 % / 100 % of rows migrated / unversioned control | 37 / 42 / 46 / 37 ms (same result on all four) |
| F: point lookup of an old row (default filled in) / of a migrated row | 0.0030 / 0.0029 ms |
| H: refusing a plan of an older epoch / running it at the current epoch | 0.00006 / 0.0049 ms |
| `ALTER TABLE ADD COLUMN` on 1,000 / 50,000 rows | 0.38 / 0.30 ms, 296 / 300 log bytes |
| inserts into a table with a schema, relative to none | 1.04× slower |

Adapting old rows costs nothing measurable: rows that predate a column are smaller and read its
default from the schema. `ALTER TABLE` writes one catalog record whatever the table's size. The
2.3.0 workloads are unchanged (the JVM benchmark now runs on the engine catalog); recorded runs
and how to read them: [examples/results/README.md](examples/results/README.md).

## Architecture

```text
SQL -> Scala: parse -> bind (slots, engine catalog at epoch E) -> logical plan -> rules -> join order
              -> physical plan (access paths, estimates, cost, decisions) <- statistics, indexes <- C ABI v7
    -> plan wire v4 JSON {schema_epoch: E} -> Java FFM -> C ABI v7 -> Rust: check epoch -> validate
                                                     -> operators (rows adapted to E) -> batch v2
                                                     -> runtime profile JSON (node ids)

  commit: validate + schema check -> append to LOG -> apply to projections incl. schema registry
          and indexes (memory) -> group fsync -> publish
  DDL:    catalog transaction: validated on a registry copy -> Catalog record -> registry, epoch + 1
  read:   snapshot over current + version projections; EntityScan = key-range scan in PK order;
          IndexScan = versioned index entries at the snapshot, RowFetch = batched lookups
  CDC:    WalCursor over the durable log prefix
  checkpoint: dirty pages + roots + record -> journal -> in place
```

Details: [docs/architecture.md](docs/architecture.md) (including the query path and the adaptive
loop), execution layer: [docs/execution.md](docs/execution.md), statistics:
[docs/statistics.md](docs/statistics.md), optimizer: [docs/optimizer.md](docs/optimizer.md), indexes:
[docs/indexes.md](docs/indexes.md), schema evolution: [docs/schema.md](docs/schema.md), invariants mapped to tests:
[docs/invariants.md](docs/invariants.md), CDC: [docs/cdc.md](docs/cdc.md), ABI:
[docs/ffi.md](docs/ffi.md), wire formats: [docs/plan-wire-format.md](docs/plan-wire-format.md),
[docs/batch-format.md](docs/batch-format.md), roadmap: [docs/roadmap.md](docs/roadmap.md).

## SQL (JVM layer)

```sql
CREATE TABLE customer (id BIGINT PRIMARY KEY, name STRING NOT NULL, city STRING);
CREATE TABLE orders (id BIGINT PRIMARY KEY,
  customer_id BIGINT NOT NULL REFERENCES customer(id) NOT ENFORCED,  -- optimizer hint
  amount BIGINT NOT NULL);
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
ANALYZE;                               -- statistics for every table
EXPLAIN SELECT ...;                    -- plans with estimates, join order, decisions, warnings
EXPLAIN ANALYZE SELECT ...;            -- plus the per-operator profile and q-error
SET optimizer = rule;                  -- the 2.1.3 planner (cost is the default)
CREATE INDEX idx_amount ON orders (amount) INCLUDE (customer_id);
SELECT customer_id, amount FROM orders WHERE amount >= 500;                           -- covering IndexScan
SELECT id, amount FROM orders ORDER BY amount DESC LIMIT 5;                          -- no TopK
SHOW INDEXES;
DROP INDEX idx_amount;
ALTER TABLE orders ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR';                -- no row rewritten
ALTER TABLE orders RENAME COLUMN amount TO total;
ALTER TABLE customer DROP COLUMN city;                                                -- RESTRICT
SHOW SCHEMA orders;                                                                   -- version history
DROP TABLE orders;
```

The full grammar and its rules are in [docs/milestone-2.1.3.md](docs/milestone-2.1.3.md#supported-sql-213)
[docs/milestone-2.2.3.md](docs/milestone-2.2.3.md#supported-sql-223-additions),
[docs/milestone-2.3.0.md](docs/milestone-2.3.0.md#supported-sql-230-additions)
and [docs/milestone-2.4.0.md](docs/milestone-2.4.0.md#supported-sql-240-additions).

## Usage (Rust)

```rust
use adb_core::{Row, RowId, Value};
use adb_engine::{AnalyzeOptions, ChangeCursor, ChangeFilter, Database, IndexColumn, IndexSpec};
use adb_engine::{EntitySpec, NewField, SchemaChange};

let db = Database::open("data")?;
let field = |name: &str, nullable: bool| NewField { name: name.into(), value_type: adb_core::ValueType::Int64,
    nullable, default: None, references: None, id: None };
db.create_entity(EntitySpec { name: "t".into(), primary_key: "id".into(), entity_id: Some(1),
    fields: vec![field("id", false), field("value", true)] })?;          // schema version 1

let mut tx = db.begin();                                   // Serializable
let id = RowId::from_primary_key(/* entity */ 1, /* BIGINT pk */ 42); // order-preserving
if db.get_in_tx(&mut tx, id)?.is_none() {
    tx.put(id, Row::new().with_field(1, Value::Int64(100)));
}
let ts = db.commit(tx)?;                                   // durable when it returns

let old = db.get_at(id, ts)?;                              // time travel
let changes = db.read_changes(ChangeCursor::BEGINNING, 100, &ChangeFilter::entities([1]))?;

let decoded = adb_plan_wire::decode_json(plan_json)?;        // validated plan + its catalog epoch
let mut cursor = db.execute_planned(decoded.plan, decoded.schema_epoch, None)?; // SchemaChanged if stale
while let Some(batch) = cursor.next_batch()? { /* columns by output slot */ }
let profile = cursor.profile();                            // per-operator profile, node ids
let stats = db.analyze(1, &AnalyzeOptions::default())?;    // optimizer statistics of entity 1
let stale = db.modifications_since_analyze(1);
db.create_index(IndexSpec {                                // built now, maintained by every commit
    name: "idx_value".into(), entity_id: 1, include: vec![],
    columns: vec![IndexColumn { field_id: 1, direction: adb_core::Direction::Asc }],
})?;
db.alter_entity(1, &SchemaChange::RenameField { from: "value".into(), to: "amount".into() })?;
db.close()?;                                               // checkpoint (optional)
```

A complete relational example: `cargo run --release -p adb-rust-demo`.

## Build and validation

```bash
./build-milestone-2.4.0.sh   # fmt, clippy (incl. docs lints), tests, rustdoc, Rust demo,
                             # benchmarks, Java FFM smoke test, JVM tests + benchmarks + demo
./demo/run-demo.sh           # feature tour 1.0.1 -> 2.4.0 through the JVM CLI
./scripts/benchmark-db.sh    # workloads, rust_native path, index builds, schema evolution (F-H)
./scripts/benchmark-ffi.sh   # workloads, FFI and full SQL paths, selectivity sweep, calibration
```

**CI** (`.github/workflows/ci.yml`) runs the same checks on every push to `master` and every
pull request. The **Rust** job runs format, clippy with the documentation lints, tests, rustdoc
and the C header. The **JVM** job validates the Gradle wrapper and runs the Gradle tests. The
**integration** job runs the FFM smoke test, both demos and a scale-1 workload benchmark checked
by `scripts/check-benchmarks.py`. `Benchmarks` (`.github/workflows/benchmarks.yml`) is started by
hand at scale 1 or 10 and uploads its reports. Dependabot proposes weekly updates for actions,
crates and Gradle dependencies.

Requirements: Rust (stable) and JDK 22+. The Scala/JVM part uses the Gradle wrapper in `jvm/` and
needs access to Maven Central. The Java FFM → Rust path is also checked without Gradle by
`examples/jvm-cdc-smoke/run.sh`.

## Repository layout

```text
crates/        Rust engine (core, journal, page, buffer, btree, storage, schema, wal, tx, index,
               execution, plan-wire, stats, engine, ffi) and the adb-migrate tool
include/adb.h  C ABI v7
jvm/           Scala/Java: model, statistics, schema, catalog (engine catalog, indexes), SQL, binder, optimizer
               (estimation, cost, join order, access paths), physical planning, FFM, gateway,
               CLI, benchmark
scripts/       workload benchmarks
proto/         target binary plan contract
examples/      Rust demo and benchmark, JVM CDC smoke test, results
demo/          feature tour through the JVM CLI
docs/          architecture, execution, statistics, optimizer, indexes, schema, invariants, CDC, ABI, wire
               formats, milestone notes, roadmap
```
