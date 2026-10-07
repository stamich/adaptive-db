# Execution layer (Milestone 2.1.3)

`adb-execution` turns a validated physical plan into a tree of pull-based, batch-oriented
operators. It depends only on `adb-core`: all data access goes through `DataSource`
(`point_lookup`, `scan_page`), so the layer knows nothing about pages, tuple slots, the B+Tree or
the log.

```text
plan wire v2 JSON ──► adb-plan-wire::decode_json ──► PhysicalPlan::validate ──► PlanShape
                                                                                (width, output slots)
                                                        │
                                                        ▼
                                Executor ── builds ──► operator tree (every node wrapped in Profiled)
                                                        │ pulls RowBatch = Vec<ExecRow>
                                                        ▼
                                QueryCursor::next_batch ──► RecordBatch (slot columns) ──► batch format v2
                                QueryCursor::profile    ──► QueryProfile (EXPLAIN ANALYZE)
```

## Slots instead of field ids

Storage addresses a value by `(entity, FieldId)`. A query can read one entity several times
(self-join, aliases), so field ids are not unique inside one operator row. Since 2.1 every column
of every relation *instance* gets a query-scoped **`SlotId`**, assigned densely by the JVM binder
(`0, 1, 2, …` in order of first reference):

```text
SELECT a.id, b.id FROM orders a JOIN orders b ON a.customer_id = b.customer_id

  a.customer_id -> #0   b.customer_id -> #1   a.id -> #2   b.id -> #3
  (a.id and b.id are both field 1 of the same entity, but never share a slot)
```

* Leaf nodes (`PointLookup`, `Scan`, `EntityScan`) carry `columns: [{field_id, slot}]`; this is the
  only place where storage field ids appear. Only referenced fields are listed, so scans read
  nothing else.
* An **`ExecRow`** is a `Vec<Value>` indexed by slot, as wide as the plan (`PlanShape::width`).
  Slots an operator does not produce stay `NULL`; a join output is the left row with the right
  side's slots copied in.
* Expressions read slots (`{"kind":"slot","slot":n}`), so one expression works on scan, join and
  aggregate output alike.
* `Project` selects and orders the output slots and clears the others, so blocking operators above
  it hold only returned values.

`adb_core::TupleSlot` (formerly `SlotId`) is the unrelated index of a tuple inside a heap page.

## Operators

| Operator | Kind | Notes |
|---|---|---|
| `PointLookup`, `Scan`, `EntityScan` | leaf, streaming | key-range scans paged with a keyset cursor (`batch_size` rows per page) |
| `Filter` | streaming | NULL counts as false; never returns an empty batch |
| `Project` | streaming | output slots, in output order |
| `Limit` | streaming | stops pulling its input once satisfied |
| `HashJoin` | build right, stream left | INNER / LEFT; equality keys `left.slot = right.slot`; optional residual |
| `NestedLoopJoin` | materialize right, stream left | INNER / LEFT; any predicate or none (cross join) |
| `Aggregate` | blocking | GROUP BY slots + COUNT(*) / COUNT / SUM / MIN / MAX / AVG |
| `Sort` | blocking | stable multi-key sort; NULLS LAST ascending, NULLS FIRST descending |
| `TopK` | streaming into O(k) buffer | exactly `Limit(Sort)`, in O(n log k) time and O(k) memory |

### Join semantics

* NULL (and NaN) keys never match. Keys are normalized for hashing (`KeyPart`): `-0.0` equals
  `0.0`.
* The hash join's **residual** and the nested-loop **predicate** are part of the join condition:
  under LEFT JOIN a left row whose candidate matches all fail it is still emitted once with NULL
  right slots (unlike a `Filter` above the join).
* Join output rows have no storage row id.
* Output is batched: one output batch holds at most `batch_size + max_join_fanout` rows (the last
  left row may overshoot the batch size by its matches).

### Aggregation semantics

* NULL group keys form one group. Without GROUP BY there is always exactly one output row, also
  for empty input (`COUNT = 0`, other aggregates NULL).
* `SUM` / `AVG` of INT64 accumulate exactly in `i128`; the final INT64 result is narrowed with a
  check, and overflow is `ExecutionError::ArithmeticOverflow` (C status `ARITHMETIC_OVERFLOW`),
  never a wrapped value. `AVG` returns FLOAT64. Mixed value types are an expression error.
* Groups are emitted in first-seen order.

### Sorting

Comparisons can fail (mixed types, NaN). The standard library sort requires a total order, so the
rows are checked first (`check_sortable`) and the comparator is then total and infallible: an
unsortable column is a clean `Expression` error, never a panic.

## Resource limits

Structural limits are checked once by `PhysicalPlan::validate`; runtime limits are part of the
`ExecutionContext` (`ExecutionLimits`, adjustable per query with `with_limits`).

| Limit | Value | Enforced by |
|---|---|---|
| plan depth / expression depth | 128 / 128 | validation |
| list length (scan columns, slots, keys, aggregates, sort keys) | 4096 | validation |
| slot ids | `< 4096` | validation |
| `LIMIT`, `TopK` | ≤ 1,000,000 | validation |
| plan document | ≤ 8 MiB | `adb-plan-wire` |
| query memory (all blocking operators together) | 256 MiB | `MemoryTracker` |
| materialized rows per blocking operator (build side, sort input, groups) | 1,000,000 | `collect_all_bounded`, `Aggregate` |
| matches per left row (join fanout) | 16,384 | both joins |
| nested-loop comparisons per query | 10,000,000 | `NestedLoopJoin` |
| output batch | 64 MiB estimated | `QueryCursor` |

Validation also guarantees that no slot is produced twice, join inputs produce disjoint slots,
hash-join keys come from the correct side, aggregate outputs are new slots, and every slot an
operator reads is produced by its input. Operators can therefore index rows without re-checking.

### Memory accounting

`MemoryTracker` is an atomic per-query byte budget shared by every blocking operator.
`MemoryReservation` is an RAII guard: operators reserve bytes **before** holding them and the bytes
return when the reservation (or the operator) is dropped, so a failed or cancelled query cannot
leak budget. Exceeding the budget is `ResourceLimit` (C status `RESOURCE_LIMIT`); the database is
not affected.

## Runtime profile

Every operator is wrapped in `Profiled`, which records rows, batches and inclusive wall-clock time.
Operators add their own counters:

| Operator | Counters |
|---|---|
| scans | `pages` |
| `filter` | `rows_in` (selectivity = `rows_out / rows_in`) |
| `hash_join` | `build_rows`, `build_keys`, `probe_rows`, `peak_memory_bytes` |
| `nested_loop_join` | `inner_rows`, `outer_rows`, `comparisons`, `peak_memory_bytes` |
| `aggregate` | `rows_in`, `groups`, `peak_memory_bytes` |
| `sort` | `rows_sorted`, `peak_memory_bytes` |
| `top_k` | `rows_in`, `compactions`, `peak_memory_bytes` |

`QueryCursor::profile()` returns the tree plus the query's peak memory and budget; the C ABI
returns it as JSON (`adb_query_profile_json`) and the JVM renders it for `EXPLAIN ANALYZE`. The
profile is the *observation* half of Adaptive DB's adaptive loop; the planner's explained decisions
(see [architecture.md](architecture.md#query-path-milestone-21)) are the *decision* half.
