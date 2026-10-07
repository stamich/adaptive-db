# Feature map: 1.0.1 → 2.1.3

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
