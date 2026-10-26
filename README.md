# Adaptive DB — Milestone 2.3.0

[![CI](https://github.com/stamich/adaptive-db/actions/workflows/ci.yml/badge.svg)](https://github.com/stamich/adaptive-db/actions/workflows/ci.yml)

Adaptive DB is an adaptive, intent-driven database in which **the canonical log is the source of
truth** and every physical structure (the current store, the version store, secondary indexes,
optimizer statistics and, in the future, columnar, search and graph projections) is rebuildable,
derived data. Physical execution is meant to adapt to the workload and to the user's declared
intent; the engine therefore explains every planning decision it takes, estimates what each
operator will do and measures what it actually did.

This repository implements the V1 core: a single-node transactional engine with history, a native
change stream, relational query execution (joins, aggregation, sorting), statistics and a
cost-based optimizer (2.2.3) and, since 2.3.0, secondary indexes with cost-based access paths.

## What 2.3.0 adds

| Area | 2.2.3 | 2.3.0 |
|---|---|---|
| Indexes | — | **secondary indexes** as MVCC-versioned **projections of the log**: maintained in the commit, recovered, vacuumed, verified and rebuilt like the other projections; `INCLUDE` (covering) columns; `AS OF` reads through indexes |
| SQL | `ANALYZE`, `SET optimizer` | + `CREATE INDEX name ON t (c [ASC\|DESC], ...) [INCLUDE (...)]`, `DROP INDEX`, `SHOW INDEXES [ON t]` |
| Storage | primary keys as raw bits (negative keys last) | **order-preserving key encoding**, **data format 2** (primary-key order = storage order); `adb-migrate` for older directories (LSNs and CDC offsets kept) |
| Planning | joins and strategies by cost | + **access paths by cost** (index vs sequential scan, covering scans, residuals before row fetches), **Sort / TopK elision** from index and primary-key order |
| Execution | scans, joins, aggregates, sorts | + `IndexScan` (keyset pages, snapshot re-check) and batched `RowFetch` |
| Statistics | format 1 | **format 2**: index entries, height, leaf pages, distinct prefixes, `pk_locality` |
| Observability | estimates, q-error | + index names and bounds by column, rejected access paths, `Indexes:` section; index access summary in EXPLAIN ANALYZE |
| Boundary | C ABI 5, plan wire v2 | **C ABI 6** (`adb_create_index_json`, `adb_drop_index`, `adb_list_indexes_json`, status `INCOMPATIBLE_FORMAT`), **plan wire v3** (`index_scan`, `row_fetch`) |
| Version | 2.2.3 | engine, crates and JVM build **2.3.0** (from now on: minor = milestone, patch = fixes) |
| Tests | 186 Rust, 90 JVM | **258 Rust, 120 JVM** (incl. model-based index scans at every snapshot, crash points of builds and drops, fuzzing of the key encoder and plan wire v3) |

How the 2.3.3 proposal was implemented as 2.3.0, and why: [docs/milestone-2.3.0.md](docs/milestone-2.3.0.md).
Indexes in depth: [docs/indexes.md](docs/indexes.md).

**Upgrading from 2.2.3:** the engine refuses a directory written by an earlier version
(`INCOMPATIBLE_FORMAT`). Migrate it once with
`cargo run --release -p adb-migrate -- /path/to/db` (in place, keeps a backup) or
`... -- /path/to/db --to /new/path`.

Workload benchmark (`scripts/benchmark-db.sh`, `scripts/benchmark-ffi.sh`, `ADB_BENCH_SCALE`;
2-vCPU cloud VM, p50 after warm-up; expect run-to-run variation). Scale 1 holds 424,000 rows in
11 tables, scale 10 holds 4.2 million.

| | scale 1 | scale 10 |
|---|---|---|
| C: covering index equality — rust_native / ffi_prepared_plan / full SQL | 0.025 / 0.040 / 0.105 ms | 0.083 / 0.148 / 0.49 ms |
| D: index range + row fetch, ORDER BY ... LIMIT 100 without a sort | 0.16 / 0.17 / 0.37 ms | 0.35 / 0.41 / 0.83 ms |
| E: the same table without a usable index (sequential scan) | 57 / 88 / 83 ms | 1,703 / 912 / 936 ms |
| selectivity sweep 0.01% → 50%: faster plan chosen by cost mode | 5 of 6 (50%: 6% slower) | 6 of 6 |
| `CREATE INDEX` per index | 4 s | 40 s |
| inserts with 1 / 3 indexes, relative to none | 3.0× / 5.5× slower | 3.1× / 6.7× slower |
| A: hash join + TopK — rust_native / ffi_prepared_plan / full SQL | 1.04 / 0.75 / 1.24 ms | 7.3 / 11.4 / 9.4 ms |
| B: + aggregate — rust_native / ffi_prepared_plan / full SQL | 0.66 / 0.78 / 1.08 ms | 9.8 / 9.0 / 10.9 ms |
| join order, chain / star — cost vs rule mode | 15 vs 22 / 72 vs 131 ms | 183 vs 321 / 767 ms vs **aborted** (`RESOURCE_LIMIT`) |
| `ANALYZE` throughput | 670,000 rows/s | 640,000 rows/s |

Indexes make selective reads two to three orders of magnitude faster and writes 3–7× slower. Builds
are single-threaded tree inserts. Both are 2.3.x follow-ups ([docs/roadmap.md](docs/roadmap.md)).
Recorded runs and how to read them: [examples/results/README.md](examples/results/README.md).

## Architecture

```text
SQL -> Scala: parse -> bind (slots) -> logical plan -> rules -> join order -> physical plan
              (access paths, estimates, cost, decisions) <- statistics, index catalog <- C ABI v6
    -> plan wire v3 JSON -> Java FFM -> C ABI v6 -> Rust: validate -> operators -> batch v2
                                                                   -> runtime profile JSON (node ids)

  commit: validate -> append to LOG -> apply to projections incl. indexes (memory) -> group fsync -> publish
  read:   snapshot over current + version projections; EntityScan = key-range scan in PK order;
          IndexScan = versioned index entries at the snapshot, RowFetch = batched lookups
  CDC:    WalCursor over the durable log prefix
  checkpoint: dirty pages + roots + record -> journal -> in place
```

Details: [docs/architecture.md](docs/architecture.md) (including the query path and the adaptive
loop), execution layer: [docs/execution.md](docs/execution.md), statistics:
[docs/statistics.md](docs/statistics.md), optimizer: [docs/optimizer.md](docs/optimizer.md), indexes:
[docs/indexes.md](docs/indexes.md), invariants mapped to tests:
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
```

The full grammar and its rules are in [docs/milestone-2.1.3.md](docs/milestone-2.1.3.md#supported-sql-213)
[docs/milestone-2.2.3.md](docs/milestone-2.2.3.md#supported-sql-223-additions)
and [docs/milestone-2.3.0.md](docs/milestone-2.3.0.md#supported-sql-230-additions).

## Usage (Rust)

```rust
use adb_core::{Row, RowId, Value};
use adb_engine::{AnalyzeOptions, ChangeCursor, ChangeFilter, Database, IndexColumn, IndexSpec};

let db = Database::open("data")?;

let mut tx = db.begin();                                   // Serializable
let id = RowId::from_primary_key(/* entity */ 1, /* BIGINT pk */ 42); // order-preserving
if db.get_in_tx(&mut tx, id)?.is_none() {
    tx.put(id, Row::new().with_field(1, Value::Int64(100)));
}
let ts = db.commit(tx)?;                                   // durable when it returns

let old = db.get_at(id, ts)?;                              // time travel
let changes = db.read_changes(ChangeCursor::BEGINNING, 100, &ChangeFilter::entities([1]))?;

let (plan, _shape) = adb_plan_wire::decode_json(plan_json)?; // validated physical plan
let mut cursor = db.execute(plan)?;
while let Some(batch) = cursor.next_batch()? { /* columns by output slot */ }
let profile = cursor.profile();                            // per-operator profile, node ids
let stats = db.analyze(1, &AnalyzeOptions::default())?;    // optimizer statistics of entity 1
let stale = db.modifications_since_analyze(1);
db.create_index(IndexSpec {                                // built now, maintained by every commit
    name: "idx_value".into(), entity_id: 1, include: vec![],
    columns: vec![IndexColumn { field_id: 1, direction: adb_core::Direction::Asc }],
})?;
db.close()?;                                               // checkpoint (optional)
```

A complete relational example: `cargo run --release -p adb-rust-demo`.

## Build and validation

```bash
./build-milestone-2.3.0.sh   # fmt, clippy (incl. docs lints), tests, rustdoc, Rust demo,
                             # benchmarks, Java FFM smoke test, JVM tests + benchmarks + demo
./demo/run-demo.sh           # feature tour 1.0.1 -> 2.3.0 through the JVM CLI
./scripts/benchmark-db.sh    # 2.3.0 workloads, rust_native path, index builds, write overhead
./scripts/benchmark-ffi.sh   # 2.3.0 workloads, FFI and full SQL paths, selectivity sweep, calibration
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
crates/        Rust engine (core, journal, page, buffer, btree, storage, wal, tx, index, execution,
               plan-wire, stats, engine, ffi) and the adb-migrate tool
include/adb.h  C ABI v6
jvm/           Scala/Java: model, statistics, catalog (incl. indexes), SQL, binder, optimizer
               (estimation, cost, join order, access paths), physical planning, FFM, gateway,
               CLI, benchmark
scripts/       workload benchmarks
proto/         target binary plan contract
examples/      Rust demo and benchmark, JVM CDC smoke test, results
demo/          feature tour through the JVM CLI
docs/          architecture, execution, statistics, optimizer, indexes, invariants, CDC, ABI, wire
               formats, milestone notes, roadmap
```
