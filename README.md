# Adaptive DB — Milestone 2.2.3

Adaptive DB is an adaptive, intent-driven database in which **the canonical log is the source of
truth** and every physical structure (the current store, the version store, optimizer statistics
and, in the future, columnar, search and graph projections) is rebuildable, derived data. Physical
execution is meant to adapt to the workload and to the user's declared intent; the engine
therefore explains every planning decision it takes, estimates what each operator will do and
measures what it actually did.

This repository implements the V1 core: a single-node transactional engine with history, a native
change stream, relational query execution (joins, aggregation, sorting) and, since 2.2.3,
statistics and a cost-based optimizer.

## What 2.2.3 adds

| Area | 2.1.3 | 2.2.3 |
|---|---|---|
| Statistics | — | native **`ANALYZE`**: exact counts, NULLs, min/max, **HyperLogLog** NDV, **histograms**, **most common values**; persisted by the engine |
| Freshness | — | per-entity **modification counters** (checkpointed, replayed); stale above 20% changed rows |
| SQL | joins, GROUP BY, ORDER BY | + `ANALYZE [table]`, `SET optimizer = cost \| rule`, `REFERENCES t(c) NOT ENFORCED` foreign-key hints |
| Planning | rules, SQL join order, right side built | **cardinality estimation** with confidence, **cost model** checked against engine limits, **join ordering** (DP ≤ 10, greedy ≤ 32), **build-side swap** |
| Observability | EXPLAIN decisions, EXPLAIN ANALYZE profile | + **estimates per node**, statistics status and warnings; **q-error** per operator matched by profile **node id**; planner feedback log |
| Boundary | C ABI 4 | **C ABI 5** (`adb_analyze_entity_json`, `adb_statistics_json`, `adb_statistics_generation`, `adb_modifications_since_analyze`); plan wire and batch format unchanged |
| Version | 2.1.3 | engine, crates and JVM build **2.2.3** |
| Tests | 158 Rust, 40 JVM | **186 Rust, 90 JVM** (incl. DP-optimality and result-equality properties) |

Order-preserving key encoding (needed for Sort elision) is planned for 2.3 together with secondary
indexes, so storage migrates once. How the 2.2.3 proposal was adapted, and why:
[docs/milestone-2.2.3.md](docs/milestone-2.2.3.md).

Workload benchmark (`scripts/benchmark-db.sh`, `scripts/benchmark-ffi.sh`; 100 customers, 1,000
orders, 100 iterations, 2-vCPU cloud VM; expect run-to-run variation):

| Workload | rust_native p50 | ffi_prepared_plan p50 | scala_cbo_ffi_rust p50 |
|---|---|---|---|
| A: hash join + TopK | 0.82 ms | 1.24 ms | 4.34 ms |
| B: hash join + aggregate + TopK | 0.87 ms | 1.12 ms | 3.34 ms |

Join order (20,000 → 2,000 → 20 rows, selective filter): cost mode `(a JOIN (b JOIN c))` 21.3 ms
vs. rule mode `((a JOIN b) JOIN c)` 22.7 ms, identical results; `ANALYZE` of 100,000 rows ≈ 165 ms.
Recorded runs: [examples/results/2.2.3-database.json](examples/results/2.2.3-database.json),
[examples/results/2.2.3-ffi.json](examples/results/2.2.3-ffi.json).

## Architecture

```text
SQL -> Scala: parse -> bind (slots) -> logical plan -> rules -> join order -> physical plan
              (estimates, cost, decisions) <- statistics <- C ABI v5 <- Rust ANALYZE / counters
    -> plan wire v2 JSON -> Java FFM -> C ABI v5 -> Rust: validate -> operators -> batch v2
                                                                   -> runtime profile JSON (node ids)

  commit: validate -> append to LOG -> apply to projections (memory) -> group fsync -> publish
  read:   snapshot over current + version projections; EntityScan = key-range scan
  CDC:    WalCursor over the durable log prefix
  checkpoint: dirty pages + roots + record -> journal -> in place
```

Details: [docs/architecture.md](docs/architecture.md) (including the query path and the adaptive
loop), execution layer: [docs/execution.md](docs/execution.md), statistics:
[docs/statistics.md](docs/statistics.md), optimizer: [docs/optimizer.md](docs/optimizer.md), invariants mapped to tests:
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
```

The full grammar and its rules are in [docs/milestone-2.1.3.md](docs/milestone-2.1.3.md#supported-sql-213)
and [docs/milestone-2.2.3.md](docs/milestone-2.2.3.md#supported-sql-223-additions).

## Usage (Rust)

```rust
use adb_core::{Row, RowId, Value};
use adb_engine::{AnalyzeOptions, ChangeCursor, ChangeFilter, Database};

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
let profile = cursor.profile();                            // per-operator profile, node ids
let stats = db.analyze(1, &AnalyzeOptions::default())?;    // optimizer statistics of entity 1
let stale = db.modifications_since_analyze(1);
db.close()?;                                               // checkpoint (optional)
```

A complete relational example: `cargo run --release -p adb-rust-demo`.

## Build and validation

```bash
./build-milestone-2.2.3.sh   # fmt, clippy (incl. docs lints), tests, rustdoc, Rust demo,
                             # benchmarks, Java FFM smoke test, JVM tests + benchmarks + demo
./demo/run-demo.sh           # feature tour 1.0.1 -> 2.2.3 through the JVM CLI
./scripts/benchmark-db.sh    # 2.2.3 workloads, rust_native path
./scripts/benchmark-ffi.sh   # 2.2.3 workloads, FFI and full SQL paths, join order, calibration
```

Requirements: Rust (stable) and JDK 22+. The Scala/JVM part uses the Gradle wrapper in `jvm/` and
needs access to Maven Central. The Java FFM → Rust path is also checked without Gradle by
`examples/jvm-cdc-smoke/run.sh`.

## Repository layout

```text
crates/        Rust engine (core, journal, page, buffer, btree, storage, wal, tx, execution,
               plan-wire, stats, engine, ffi)
include/adb.h  C ABI v5
jvm/           Scala/Java: model, statistics, catalog, SQL, binder, optimizer (estimation, cost,
               join order), physical planning, FFM, gateway, CLI, benchmark
scripts/       workload benchmarks
proto/         target binary plan contract
examples/      Rust demo and benchmark, JVM CDC smoke test, results
demo/          feature tour through the JVM CLI
docs/          architecture, execution, statistics, optimizer, invariants, CDC, ABI, wire formats,
               milestone notes, roadmap
```
