# Feature map: 1.0.1 → 2.3.0

## 1.0.1 — transactions, WAL and recovery

The demo inserts two rows, closes the native database, and reopens the same directory. The subsequent query demonstrates that committed state survives reopening. In the current engine this durability is backed by the hardened WAL/recovery chain plus the persistent structures added later.

## 1.5.1 — persistent current state and primary B+Tree

`EXPLAIN SELECT * FROM account WHERE id = 2` exposes the optimizer's point-lookup path. The SQL optimizer itself belongs to 2.0, but the underlying persistent current-state/B+Tree capability originates in 1.5.

## 1.6.1 — persistent historical versions

The demo captures the commit timestamp returned by the original insert, updates the row, and executes:

```sql
SELECT * FROM account AS OF VERSION <old-commit-ts> WHERE id = 2;
```

The result demonstrates durable MVCC history across versions.

## 1.7.1 — physical execution and native batch boundary

`EXPLAIN ANALYZE` and a multi-row SELECT exercise the physical plan executor. The JVM sends physical-plan JSON through Java FFM and consumes ADB Batch v1 RecordBatch results from Rust.

## 2.0.1 — SQL/control plane

The final phase exercises the whole JVM stack:

```text
SQL
 ↓
Tokenizer / Parser
 ↓
Catalog + Binder
 ↓
Logical Planner
 ↓
Rule Optimizer
 ↓
Physical Planner
 ↓
Java FFM
 ↓
Rust Execution Engine
```

It demonstrates CREATE TABLE, INSERT, UPDATE, DELETE, filtered SELECT, LIMIT and EXPLAIN.


## 2.0.3 — log as source of truth, native CDC

The SQL tour is unchanged; table scans now run as native `EntityScan`. The data-plane features of
2.0.3 (change feed, consumer offsets, vacuum, checkpoints) are exercised by
`examples/jvm-cdc-smoke/run.sh` (Java FFM) and by the Rust tests in `crates/adb-engine/tests`.

## 2.1.3 — relational execution

The last phase creates `customer` and `orders` and runs `sql/2.1.3-relational.sql`:

```text
SQL with JOIN / GROUP BY / ORDER BY
 ↓
SqlParser (aliases, qualified columns, aggregates)
 ↓
SelectBinder (relation scope, dense SlotIds, GROUP BY rules)
 ↓
LogicalPlanner → RuleOptimizer (predicate pushdown, point lookups)
 ↓
PhysicalPlanner + PlanningPolicy (HashJoin | NestedLoopJoin, TopK; decisions with reasons)
 ↓
plan wire v2 → Java FFM → C ABI v4
 ↓
Rust: HashJoin / NestedLoopJoin / Aggregate / Sort / TopK under a query MemoryTracker
 ↓
batch format v2 (slot columns) + runtime profile → EXPLAIN ANALYZE
```

| Query | Shows |
|---|---|
| join + `ORDER BY` | hash join over slot-addressed rows, native sort |
| `LEFT JOIN` + `COUNT(o.id)` | null-filling (a customer without orders counts 0) |
| `GROUP BY city` + `SUM`/`AVG` + `LIMIT` | checked aggregation, TopK |
| `orders a JOIN orders b` | self-join: one entity, two relation instances, distinct slots |
| `ON o.customer_id <> c.id` | nested-loop fallback, filter pushed below the join |
| `EXPLAIN ANALYZE` | plans, planner decisions with reasons, per-operator profile |

## 2.2.3 — statistics and cost-based optimization

This phase creates `region`, `shop` and `sale` (with foreign-key hints), loads 4 / 40 / 400
rows and runs `sql/2.2.3-statistics.sql`:

```text
ANALYZE ─► adb-stats collector (snapshot scan) ─► stats/entity-<id>.stats
commits ─► modification counters ─► checkpoint journal
          │
          ▼ C ABI 5
StatisticsProvider ─► CardinalityEstimator ─► CostModel ─► JoinReorderRule + CostBasedPolicy
          │
          ▼
EXPLAIN (estimates, join order, statistics, warnings) / EXPLAIN ANALYZE (q-error by node id)
```

| Step | Shows |
|---|---|
| `EXPLAIN` before `ANALYZE` | default estimates (1,000 rows, confidence 0.1) and "run ANALYZE" warnings |
| `ANALYZE;` | statistics for every table |
| `EXPLAIN ANALYZE` | the selective region filter joined first, estimates per node with their basis, actual rows and q-error |
| same query in cost and rule mode | identical rows from different join orders |
| 200 more sales | statistics of `sale` reported stale (50% changed) |

## 2.3.0 — secondary indexes

The last phase creates `ledger`, loads 2,000 rows and runs `sql/2.3.0-indexes.sql`. Indexes are
derived projections of the log, like the current store. They are versioned with the same
`begin_ts`/`end_ts` as rows, so a snapshot reads an index exactly as it reads the table.

```text
commit ─► WAL (rows only) ─► current store ─┬─► version store
                                            └─► index projections (same commit, entries with begin/end ts)
CREATE INDEX ─► build at one snapshot ─► checkpoint publishes indexes/catalog.meta
                                            │
SQL ─► IndexPredicateMatcher ─► AccessPathEnumerator ─► CostModel.accessCost ─► CostBasedPolicy
                                            │                       (index_scan / row_fetch / entity_scan)
                                            ▼ plan wire v3, C ABI 6
                            IndexScan (keyset pages, snapshot check) ─► RowFetch (sorted batch lookups)
```

| Step | Shows |
|---|---|
| `EXPLAIN` without an index | sequential scan is the only access path |
| `CREATE INDEX ... INCLUDE (amount)`, `SHOW INDEXES` | build over existing rows, serving range of snapshots |
| `EXPLAIN ANALYZE` on `account = 42` | covering `IndexScan` with bounds by column, rejected `entity_scan`, index statistics, index access summary |
| `SELECT id, memo ...` | `RowFetch` for columns the index does not hold |
| `ORDER BY amount DESC LIMIT 5` | backward index scan with a LIMIT, no TopK; rule mode returns the same rows |
| `UPDATE` then `AS OF VERSION` | the index serves the older snapshot from its versioned entries |
| `DROP INDEX` | the plan falls back to the sequential scan and TopK |

## 2.4.0 — schema evolution

The last phase evolves `ledger` with `sql/2.4.0-schema.sql`. Schemas are a projection of the
log, like the current store and the indexes: a DDL statement is a catalog transaction, so it is
durable with the data and replayed on recovery. Rows keep the schema version they were written
under and are adapted when read.

```text
SQL DDL ─► Binder ─► EngineCatalog ─► SchemaEvolutionPlanner + DependencyGraph (RESTRICT, messages)
                                  │
                                  ▼ C ABI 7: adb_alter_entity_json
              commit_catalog ─► WAL Catalog record ─► SchemaRegistry (versions, epochs) ─► schema/registry.meta
                                                               │
writes ─► prepare_write: stamp latest version, fill defaults, check types ─► WAL Put
reads  ─► RowAdapter(version of row → version of the query's epoch): hide dropped, add defaults
plans  ─► plan wire v4 {"schema_epoch":N} ─► execute_planned: refused with SCHEMA_CHANGED if N moved
```

| Step | Shows |
|---|---|
| `ADD COLUMN currency ... DEFAULT 'EUR'` | a new version, no row rewritten; old rows read the default |
| `EXPLAIN ... WHERE currency = 'EUR'` | Schema section with the plan's epoch; the estimate derived from the default |
| `UPDATE` / `INSERT` omitting `currency` | lazy migration to version 2; defaults filled on write |
| `RENAME COLUMN memo TO note` | metadata only: the field id and the values stay |
| `DROP COLUMN account` | refused: `idx_ledger_account` depends on it (RESTRICT) |
| `DROP COLUMN note`, `SELECT *` | the column disappears from reads; history keeps its values |
| mistyped `INSERT` | refused: rows are validated against the schema |
| `SHOW SCHEMA ledger` | every version with its epoch, change and columns |
| close, reopen, `SHOW SCHEMA` | the catalog is replayed from the log with the rows |

