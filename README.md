# Adaptive DB — Milestone 2.5.4.1

[![CI](https://github.com/stamich/adaptive-db/actions/workflows/ci.yml/badge.svg)](https://github.com/stamich/adaptive-db/actions/workflows/ci.yml)

Adaptive DB is an adaptive, intent-driven database in which **the canonical log is the source of
truth** and every physical structure (the current store, the version store, secondary indexes,
optimizer statistics and, in the future, columnar, search and graph projections) is rebuildable,
derived data. Physical execution is meant to adapt to the workload and to the user's declared
intent; the engine therefore explains every planning decision it takes, estimates what each
operator will do and measures what it actually did.

This repository implements the V1 core: a single-node transactional engine with history, a native
change stream, relational query execution (joins, aggregation, sorting), statistics and a
cost-based optimizer (2.2.3), secondary indexes with cost-based access paths (2.3.0), schema
evolution (2.4.0: versioned schemas owned by the engine, changed without rewriting rows) and, since
2.5.0, a global commit order, a change log derived from the canonical log, a registry of
projections with their freshness, and session-level consistency (`SET SESSION CONSISTENCY`) and, since
2.5.1, a plan cache in the SQL gateway and, since 2.5.2, sampled query telemetry (cheaper to capture since 2.5.2.1) and, since 2.5.3, a workload advisor that recommends indexes and `ANALYZE` from that telemetry and, since 2.5.4, an opt-in `auto_safe` actuator that carries a recommendation out, measures it and keeps or undoes it.

## What 2.5.4.1 adds

2.5.4.1 changes no engine behaviour. It gives the project a **trustworthy benchmark harness**: the
new crate `adb-bench` (measurement plan, median of medians, batch MAD stability, outliers that are
reported and never removed, a record of the machine and the storage, and an fsync control probe).
The reason is the 2.5.4 finding that identical code wrote 5.1k or 42.0k rows/s depending on the CPU
governor. The C ABI stays 9, plan wire v4 and data format 4 are unchanged, and the Cargo crates stay
`2.5.4`; the engine string, the JVM build, the scripts and the workflows say **2.5.4.1**.

The milestone is delivered task by task (31 in all; 28 are done, the calibration of the limits and the
release run need the reference machine). Run it with `scripts/benchmark.sh all` (`--quick`,
`--repeat N`, `--ab REF`); the rules of measurement, the families, the gates and their exit codes are in [docs/BENCHMARKS.md](docs/BENCHMARKS.md) and
[docs/milestone-2.5.4.1.md](docs/milestone-2.5.4.1.md).

## What 2.5.4 adds

2.5.4 adds an **actuator** in a new JVM module, `adb-actuator`. With `SET adaptive = auto_safe` the
database creates the index the advisor recommends, measures whether queries now read fewer rows,
keeps it if they do and drops it again if they do not. It changes nothing in the engine: the
**C ABI stays 9**, plan wire v4 and data format 4 are the same, and no data migration is needed.

| Area | 2.5.3 | 2.5.4 |
|---|---|---|
| Acting | never: recommends only | **opt-in**: `SET adaptive = off \| advisory \| auto_safe` (default `off`) |
| Statements it may run | none | `CREATE INDEX`, `ANALYZE`, `DROP INDEX` of its own indexes, nothing else |
| Decision | hysteresis only | **policy gate**: 11 rejection reasons (evidence, ratio, cooldowns, index count, table size, bytes, builds in flight), a budget and cooldowns |
| Measuring | — | rows read per execution before and after (exact engine counter); time can only veto beyond 2x; write cost against per-entity **write counters** |
| Undo | — | rollback of an index that did not help; the rollback cooldown doubles per rollback of the same target |
| Ownership | — | only `adv_*` indexes it created can be dropped (two guards); idle ones are retired; a person's drop is respected |
| History | audit log of advice | **action log**: applied, kept, rolled_back, rejected, failed, externally_dropped, adopted |
| SQL / shell | `SET advisor`, `\advice` | `SET adaptive`; `\actions`, `\actions log` |
| Version | 2.5.3 | engine, crates, JVM build and scripts **2.5.4** |
| Tests | 417 Rust, 279 JVM | **417 Rust, 405 JVM**, plus the six JVM smoke tests |

```sql
SET adaptive = auto_safe;        -- telemetry goes to `full` while this is on
-- ... run the workload ...
\actions
-- id | target | action       | state | sql                                             | reason
-- 1  | index:7:5 | create_index | kept  | CREATE INDEX adv_ledger_currency ON ledger (currency) | rows read fell 2002.0x, time x0.05
```

The design, the defaults and the safety rules are in [docs/actuator.md](docs/actuator.md); what was
done and left out is in [docs/milestone-2.5.4.md](docs/milestone-2.5.4.md).

**Limits to know before turning it on:** `CREATE INDEX` blocks the database while it builds (about
30 µs per row), so the actuator builds only on tables of up to 50 000 rows; larger tables still get
advice. The default stays `off`.

**Upgrading from 2.5.3:** rebuild the JVM modules; the native library is unchanged except for its
version string. Nothing happens until `SET adaptive = auto_safe`.

**Numbering:** 2.5.4 has three components, so the Cargo crates are `2.5.4`. 2.5.4.1 has four, which Cargo
does not accept, so the crates stay `2.5.4` and the engine string, JVM build and scripts say `2.5.4.1`.

## What 2.5.3 adds

2.5.3 adds a **workload advisor** in a new JVM module, `adb-advisor`. It reads what telemetry
captured, finds problems the physical design could fix and says so. It changes nothing in the
engine: the **C ABI stays 9**, plan wire v4 and data format 4 are the same, and no data migration
is needed.

| Area | 2.5.2.1 | 2.5.3 |
|---|---|---|
| Use of telemetry | counters and shapes to read by hand (`\telemetry shapes`) | **workload model**: a bounded ring of time windows (deltas), so an old workload stops looking hot |
| Findings | none | four detectors: **hot query**, **scan amplification**, **hot join**, **cardinality error** |
| Advice | none | `CREATE INDEX` (heuristic cost-benefit score against one table scan to build it), `ANALYZE` (never analyzed, stale statistics), and information (hot joins, estimates off with fresh statistics) |
| Stability | — | hysteresis: shown after 3 evaluations in a row, withdrawn after 3 misses, withdrawn at once when applied |
| History | — | bounded **audit log** of every confirmed, withdrawn and applied recommendation |
| SQL / shell | `SET telemetry`, `\telemetry` | `SET advisor = on \| off`; `\advice`, `\advice log` |
| Acting | — | **never**: the advisor only recommends; it reads immutable snapshots and has no handle to the engine |
| Version | 2.5.2.1 | engine, crates, JVM build and scripts **2.5.3** |
| Tests | 417 Rust, 237 JVM | **417 Rust, 279 JVM**, plus the five JVM smoke tests |

```sql
SET telemetry = sampled;         -- the advisor reads telemetry
-- ... run the workload ...
\advice
-- for example: index | create an index on orders(customer_id) (benefit/cost 12.4) | CREATE INDEX adv_orders_customer_id ON orders (customer_id)
```

The design, the cost formula, the limits (sampled evidence, no write telemetry, single-column
candidates) and what is checked are in [docs/advisor.md](docs/advisor.md); what was done and left
out is in [docs/milestone-2.5.3.md](docs/milestone-2.5.3.md).

**Upgrading from 2.5.2.1:** rebuild the JVM modules; the native library is unchanged except for
its version string. The advisor is off until `SET advisor = on` (or `\advice` is run by hand) and
needs `SET telemetry = sampled | full` to see queries.

**Numbering:** 2.5.3 has three components, so the Cargo crates are `2.5.3` again.

## What 2.5.2.1 adds

2.5.2.1 is a follow-up of 2.5.2 that makes a **captured** telemetry query cheaper. It adds no SQL
and no feature. It adds one engine function, so the **C ABI is 9** (additive: every ABI 8 function
and struct is unchanged); plan wire v4 and data format 4 are the same.

| Area | 2.5.2 | 2.5.2.1 |
|---|---|---|
| Rows read of a captured query | the JSON profile was fetched, decoded to a `String` and scanned | **`adb_query_telemetry`** fills four integers (rows returned and read, batches, peak memory) in a reused native segment; the profile is fetched only for EXPLAIN ANALYZE and the feedback log |
| Allocation, query skipped by the sampler | +24 bytes (a boxed shape id) | **+0 bytes** (allocation-free known-shape test) |
| Allocation, query captured | +2 344 bytes | **+144 bytes** |
| Checks | timings only (noisy) | allocation held to a budget by `check-benchmarks.py` and the smoke test |
| Version | 2.5.2 | engine string, JVM build and scripts **2.5.2.1**; crates stay 2.5.2 (Cargo has three components) |
| Tests | 413 Rust, 229 JVM | **417 Rust, 237 JVM**, plus the four JVM smoke tests |

What was left out and why (a ring buffer, thread-local counters, double-buffered aggregation) is in
[docs/milestone-2.5.2.1.md](docs/milestone-2.5.2.1.md); the measurements, and why the shared VM's
timings cannot judge a few microseconds, are in [docs/telemetry.md](docs/telemetry.md#measured).

**Measured on the Ryzen 9 5950X** (`examples/results/2.5.2.1-*.json`, quiet disk): read and planning
paths unchanged against 2.5.1 (median -0.8 %), telemetry allocation +0 / +144 bytes. The write rates
of the 2.5.2 report (about half of 2.5.1) were not a regression: the same build gave 6x the rate
on a quiet disk; see [examples/results/README.md](examples/results/README.md).

**Upgrading from 2.5.2:** rebuild the native library and the JVM modules together; a JVM built for
ABI 9 refuses an ABI 8 library and the reverse. No data migration.

**Numbering:** as for 2.5.1 and 2.5.2, a fix-level number is used for a follow-up at the author's
request.

## What 2.5.2 adds

2.5.2 changes only the JVM SQL gateway (new module `adb-telemetry`). The engine, the data format
(4), the C ABI (8) and the plan wire (v4) are the same as in 2.5.1. It observes; it does not decide.

| Area | 2.5.1 | 2.5.2 |
|---|---|---|
| Workload visibility | none | **sampled telemetry**: per query, a literal-free **shape** (entities, filter columns, join edges, order, flags), rows returned and read, time, estimate error, plan-cache hit |
| Hot path | profile fetched on every execution | **off by default**: one volatile read; the sampler decides from values at hand; the shape comes from the cached plan; the profile is fetched only for captured queries |
| Queue | — | bounded, `offer` never blocks, full queue drops and counts; a background worker folds events into per-shape statistics |
| Sampling | — | always: failures, slow queries (10 ms), unseen shapes; known fast queries 1 in 128 (deterministic counter) |
| SQL / shell | `SET plan_cache` | `SET telemetry = off \| sampled \| full`; `\telemetry`, `\telemetry shapes` |
| Version | 2.5.1 | engine, crates, JVM build and scripts **2.5.2** |
| Tests | 413 Rust, 214 JVM | **413 Rust, 229 JVM**, plus `examples/jvm-telemetry-smoke` against the real library |

Cost on a cached point lookup in the validation VM (noisy): telemetry on but skipped −4 % to +4 %
(within noise), captured +12 % to +18 %; the shape is computed once per plan, not per execution.
Details, the sampling rules and the measurements: [docs/telemetry.md](docs/telemetry.md); how the
proposal was implemented and adapted: [docs/milestone-2.5.2.md](docs/milestone-2.5.2.md).
The workload model and the advisor that read this data arrived in 2.5.3 (the actuator in 2.5.4).

**Upgrading from 2.5.1:** nothing to do; there is no format change and telemetry is off until you
run `SET telemetry = sampled`.

**Numbering:** as for 2.5.1, our rule keeps patch numbers for fixes; it is numbered 2.5.2 at the
author's request.

## What 2.5.1 added

2.5.1 changes only the JVM SQL gateway. The engine, the data format (4), the C ABI (8) and the
plan wire (v4) are the same as in 2.5.0.

| Area | 2.5.0 | 2.5.1 |
|---|---|---|
| Repeated SELECT | parsed, bound, planned and encoded on every execution | the **physical plan is cached** and reused; a hit goes straight to the engine |
| Cache key | — | the statement's **token stream** plus the optimizer mode (layout and comments do not matter) |
| Validity | — | the **schema epoch** and the **statistics generation** of each table read, taken from the engine before planning and checked on every hit; DDL, `CREATE INDEX` and `ANALYZE` (from any handle) invalidate, DML does not |
| Not cached | — | `AS OF`, plans over asynchronous projections, `EXPLAIN` |
| Memory | — | byte budget (32 MiB), 8 shards, LRU; stored on the **second** sighting so one-off statements do not fill it |
| SQL / shell | — | `SET plan_cache = on \| off`; `\cache`, `\cache clear`; a `Plan cache:` line in EXPLAIN |
| Version | 2.5.0 | engine, crates, JVM build and scripts **2.5.1** |
| Tests | 413 Rust, 201 JVM | **413 Rust, 214 JVM**, plus `examples/jvm-plan-cache-smoke` against the real library |

On the Ryzen (`examples/results/2.5.1-*.json`), through the full SQL path the covering-index point
read fell from 0.101 to 0.028 ms and the index range query from 0.226 to 0.123 ms; the join queries
gain about 15 % and the 48 ms scan nothing, because the engine dominates them. Engine paths are
unchanged (within 1–4 %). Details,
limits and what was left out: [docs/plan-cache.md](docs/plan-cache.md); how the proposal was
implemented and adapted: [docs/milestone-2.5.1.md](docs/milestone-2.5.1.md).

**Upgrading from 2.5.0:** nothing to do; there is no format change. To switch the cache off, run
`SET plan_cache = off` or pass a budget of 0.

**Numbering:** our rule keeps patch numbers for fixes, and the roadmap had a plan cache under 2.6+;
it is numbered 2.5.1 at the author's request.

## What 2.5.0 added

| Area | 2.4.0 | 2.5.0 |
|---|---|---|
| Commit order | commit timestamps | + **`CommitSeq`**: one number per commit, shared by data and DDL commits, stored in the log record; `Database::commit` returns a `CommitResult` |
| Change log | — | **derived from the canonical log**: envelopes per commit in CRC-framed segments, appended before the number is published, fsynced at checkpoint and close, **rebuilt from the log** when missing or damaged; cursors, retention by consumers, size, age and lag |
| Projections | implicit | a **registry** with state (`Building`, `Ready`, `NeedsRebuild`, `Failed`), frontier, lag and consumption rate; `wait_until`; the current rows and indexes are registered as synchronous projections; asynchronous consumers can register and be fed from the change log |
| Consistency | every read sees every commit | **session tokens** (the commit number of a session's last write) and `SET SESSION CONSISTENCY STRONG \| READ_YOUR_WRITES \| BOUNDED n MS\|COMMITS \| EVENTUAL`; a freshness resolver decides per index whether it may serve the read, must be waited for or must be bypassed |
| Planner | indexes always usable | access paths filtered by freshness; EXPLAIN **Read context** section; a wait that times out re-plans without the index |
| Shell | SQL | + `\session`, `\projections`, `\changes [after] [max]` |
| Storage | data format 3 | **data format 4**, upgraded in place from format 3 (nothing rewritten) |
| Boundary | C ABI 7 | **C ABI 8** (`adb_latest_commit_seq`, `*_commit` mutations, change-log cursors, projection registry); plan wire stays v4 |
| Version | 2.4.0 | engine, crates and JVM build **2.5.0** |
| Tests | 320 Rust, 159 JVM | **413 Rust, 201 JVM**, plus fuzzing of the change log and an end-to-end smoke through the real library |

How the 2.5.0 proposal was implemented, and why: [docs/milestone-2.5.0.md](docs/milestone-2.5.0.md).
The change log and projections: [docs/change-log.md](docs/change-log.md). Consistency levels, tokens
and the freshness decision: [docs/consistency.md](docs/consistency.md). Robustness review:
[docs/security-review-2.5.0.md](docs/security-review-2.5.0.md).

**Upgrading from 2.4.0:** nothing to do. The engine upgrades the data directory to format 4 in
place on open (commits of the old log are numbered by their timestamps) and builds the change log
from the log the first time. 2.5.0 does not change the semantics of any existing statement: indexes
are still maintained inside the commit, so every level reads the same rows until a later milestone
makes projections asynchronous. Benchmarks I–O (`scripts/benchmark-db.sh`) measure the new
machinery; on the Ryzen 9 5950X workloads A–E stay within 1–2 % of 2.4.0, the change log streams
511,000 commits/s, a following projection is reached in 0.11 ms (p50) and an open that rebuilds the
change log from the log takes 69 ms against 20 ms with an intact one
([examples/results/README.md](examples/results/README.md)).

## What 2.4.0 added

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

Benchmarks on an AMD Ryzen 9 5950X (`scripts/benchmark-db.sh`, `scripts/benchmark-ffi.sh`, scale
1, p50 after warm-up), compared with 2.3.0 on the same machine:

| Schema evolution (tables of 50,000 rows) | 2.4.0 |
|---|---|
| G: scan + filter on a column added later + aggregate — 0 % / 50 % / 100 % of rows migrated / unversioned control | 21.3 / 22.5 / 22.0 / 19.8 ms (same result on all four) |
| F: point lookup of an old row (default filled in) / of a migrated row | 1.97 / 1.96 µs |
| H: refusing a plan of an older epoch / running it at the current epoch | 0.03 / 1.9 µs |
| `ALTER TABLE ADD COLUMN` on 1,000 / 50,000 rows | 296 / 300 log bytes (8.2 / 5.4 ms, mostly the log sync) |
| inserts into a table with a schema, relative to none | 0.93× (no measurable cost) |

| 2.3.0 → 2.4.0 | rust_native | prepared plan over FFI | full SQL |
|---|---|---|---|
| A: hash join + TopK | 0.452 → 0.460 ms | 0.459 → 0.509 ms | 0.547 → 0.656 ms |
| C: covering index equality | 0.019 → 0.018 ms | 0.024 → 0.024 ms | 0.086 → 0.093 ms |
| D: index range + row fetch, ORDER BY ... LIMIT 100 | 0.101 → 0.101 ms | 0.111 → 0.114 ms | 0.203 → 0.217 ms |
| E: sequential scan of 100,000 rows | 32.7 → 34.8 ms | 43.1 → 48.4 ms | 43.5 → 48.7 ms |

`ALTER TABLE` writes one catalog record whatever the table's size, and old rows read their
defaults as fast as migrated rows read stored values. The engine path is unchanged (the
rust_native tables have no schema). The JVM paths, whose tables now have one, pay about 10 µs of
planning per query (the epoch check and schema-aware statistics) and about 7–12 % on large scans,
where every row passes the adapter lookup even when it is already at the current version (workload
G: 19.8 ms without a schema, 21.3 ms with one). Both are follow-ups ([docs/roadmap.md](docs/roadmap.md)).
Recorded runs and how to read them: [examples/results/README.md](examples/results/README.md).

## Architecture

```text
SQL -> Scala: parse -> bind (slots, engine catalog at epoch E) -> logical plan -> rules -> join order
              -> physical plan (access paths, estimates, cost, decisions) <- statistics, indexes <- C ABI v9
    -> plan wire v4 JSON {schema_epoch: E} -> Java FFM -> C ABI v9 -> Rust: check epoch -> validate
                                                     -> operators (rows adapted to E) -> batch v2
                                                     -> runtime profile JSON (node ids)

  commit: validate + schema check -> append to LOG -> apply to projections incl. schema registry
          and indexes (memory) -> group fsync -> publish
  DDL:    catalog transaction: validated on a registry copy -> Catalog record -> registry, epoch + 1
  read:   snapshot over current + version projections; EntityScan = key-range scan in PK order;
          IndexScan = versioned index entries at the snapshot, RowFetch = batched lookups
  CDC:    WalCursor over the durable log prefix
  order:  commit_ts + commit_seq at commit -> change-log envelope -> publish; projections follow the
          change log; a session token (its last commit_seq) decides which projections may serve it
  checkpoint: dirty pages + roots + record -> journal -> in place
```

Details: [docs/architecture.md](docs/architecture.md) (including the query path and the adaptive
loop), execution layer: [docs/execution.md](docs/execution.md), statistics:
[docs/statistics.md](docs/statistics.md), optimizer: [docs/optimizer.md](docs/optimizer.md), indexes:
[docs/indexes.md](docs/indexes.md), schema evolution: [docs/schema.md](docs/schema.md), change log:
[docs/change-log.md](docs/change-log.md), consistency: [docs/consistency.md](docs/consistency.md), invariants mapped to tests:
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
SET SESSION CONSISTENCY READ_YOUR_WRITES;   -- STRONG | READ_YOUR_WRITES | BOUNDED n MS|COMMITS | EVENTUAL
```

The full grammar and its rules are in [docs/milestone-2.1.3.md](docs/milestone-2.1.3.md#supported-sql-213)
[docs/milestone-2.2.3.md](docs/milestone-2.2.3.md#supported-sql-223-additions),
[docs/milestone-2.3.0.md](docs/milestone-2.3.0.md#supported-sql-230-additions)
[docs/milestone-2.4.0.md](docs/milestone-2.4.0.md#supported-sql-240-additions)
and [docs/milestone-2.5.0.md](docs/milestone-2.5.0.md#supported-sql-250-additions).

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
let done = db.commit(tx)?;                                 // durable when it returns
let (ts, seq) = (done.commit_ts, done.commit_seq);         // timestamp and place in the commit order

let old = db.get_at(id, ts)?;                              // time travel
let changes = db.read_changes(ChangeCursor::BEGINNING, 100, &ChangeFilter::entities([1]))?;
let mut log = db.change_log().cursor(adb_core::CommitSeq::ZERO)?;   // the change log, in commit order
while let Some(batch) = log.next_batch(500, 4 << 20)? { /* batch.commits */ }

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
./build-milestone-2.5.4.1.sh #  fmt, clippy (incl. docs lints), tests, rustdoc, Rust demo,
                             # benchmarks, Java FFM smoke test, JVM tests + benchmarks + demo
./demo/run-demo.sh           # feature tour 1.0.1 -> 2.5.4 (2.5.4.1 adds no phase) through the JVM CLI
# Benchmarks on Linux: first `sudo cpupower frequency-set -g performance`. With the default governor
# write and commit rates were 2 to 8 times lower for identical code (examples/results/README.md).
./scripts/benchmark-db.sh    # workloads, rust_native path, index builds, schema evolution (F-H), consistency (I-O)
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
`examples/jvm-cdc-smoke/run.sh`; the JVM modules, sessions and `SET SESSION CONSISTENCY` are
checked against the real library, also without Gradle, by `examples/jvm-consistency-smoke/run.sh`
and (plan cache, 2.5.1, [docs/plan-cache.md](docs/plan-cache.md)) `examples/jvm-plan-cache-smoke/run.sh`
and (telemetry, 2.5.2 and 2.5.2.1, [docs/telemetry.md](docs/telemetry.md)) `examples/jvm-telemetry-smoke/run.sh`
and (advisor, 2.5.3, [docs/advisor.md](docs/advisor.md)) `examples/jvm-advisor-smoke/run.sh`
and (actuator, 2.5.4, [docs/actuator.md](docs/actuator.md)) `examples/jvm-actuator-smoke/run.sh`.
The benchmark harness (2.5.4.1) is tested by `cargo test -p adb-bench`.

## Repository layout

```text
crates/        Rust engine (core, journal, page, buffer, btree, storage, schema, wal, tx, index,
               execution, plan-wire, stats, change-log, projection-runtime, engine, ffi), the adb-migrate tool and the adb-bench benchmark harness
include/adb.h  C ABI v9
jvm/           Scala/Java: model, statistics, schema, consistency, catalog (engine catalog, indexes), plan cache, telemetry,
               advisor, actuator, SQL, binder, optimizer (estimation, cost, join order, access paths), physical planning, FFM,
               gateway, CLI, benchmark
scripts/       workload benchmarks
proto/         target binary plan contract
examples/      Rust demo and benchmark, JVM CDC, consistency, plan-cache, telemetry, advisor and actuator smoke tests, results
demo/          feature tour through the JVM CLI
docs/          architecture, execution, statistics, optimizer, indexes, schema, change log, consistency, invariants, CDC, ABI, wire
               formats, milestone notes, roadmap
```
