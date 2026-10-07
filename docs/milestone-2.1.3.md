# Milestone 2.1.3 — relational execution

2.1.3 implements the relational-execution plan written down as "Milestone 2.1.2" (joins,
aggregation, sorting, slot-based execution, query memory accounting, plan wire crate, JVM SQL
2.1, demos and benchmarks) on top of the 2.0.3 engine. This document records how the plan was
adapted and why.

## Scope

| Area | Delivered |
|---|---|
| Identity | JVM `RelationId`, `SlotId`, `ColumnOrigin`, `Attribute`; Rust `SlotId`, `ScanColumn`, `ExecRow` |
| Physical plan | `HashJoin`, `NestedLoopJoin` (`JoinType` inner/left), `Aggregate` (`AggregateSpec`), `Sort` / `TopK` (`SortKey`); all existing nodes migrated to slots |
| Validation | depth ≤ 128, lists ≤ 4096, slots < 4096, LIMIT/TopK ≤ 1,000,000, slot and expression consistency |
| Expressions | `Expr::Slot`, `Literal`, `Binary`, `Not` |
| Memory | `MemoryTracker` (atomic reserve), RAII `MemoryReservation`, query budget, `collect_all_bounded` |
| Joins | build/probe hash join with streaming probe, INNER/LEFT, NULL fill, fanout ≤ 16,384, bounded pending output; nested-loop fallback ≤ 10,000,000 comparisons |
| Aggregation | GROUP BY, COUNT/SUM/MIN/MAX/AVG, SUM via `i128` with checked narrowing (`ArithmeticOverflow`) |
| Sorting | materializing multi-key ASC/DESC sort with fallible comparisons; `Limit(Sort)` → `TopK` |
| Wire | `adb-plan-wire`: `encode_json`, `decode_json`, validation after decode, 8 MiB |
| JVM SQL | JOIN (INNER/LEFT/CROSS), aliases, qualified columns, GROUP BY, aggregates, ORDER BY |
| Binder | `SelectBinder`: relation and alias scope, slot allocation, ambiguity detection, aggregate binding, GROUP BY validation |
| Planner | equi condition → `HashJoin`, other → `NestedLoopJoin`; full gateway path SQL → Rust |
| Hardening | resource limits, aggregate overflow, plan validation, parser/binder budgets, full `u128` RowId wire |
| Demos / benchmarks | `adb-rust-demo`, relational sections of `adb-benchmark-rust`, JVM relational planner benchmark, demo tour phase 2.1.3, `build-milestone-2.1.3.sh` |

## Changes to the 2.1.2 plan

1. **Built on 2.0.3, not on 2.1.1-buildfix.** The plan's rule "no existing production file
   changed" cannot hold when Scan/Filter/Project/Limit move to slots; the existing operators were
   migrated instead of being duplicated next to new ones.
2. **No hidden entity slot.** The plan filtered a global storage scan down to one relation through a
   hidden entity-id slot. 2.0.3 already reads exactly one entity's key range (`EntityScan`), so a
   relation scan is `EntityScan` plus a field-to-slot mapping; the hidden slot would only cost work.
3. **`SlotId` is unambiguous.** `adb_core::SlotId` meant the index of a tuple in a heap page. It is
   now `TupleSlot`, and `SlotId` means only the execution slot.
4. **Sequential slot allocation.** The plan's example (`A.field 1 → slot 1`, `B.field 1 → slot 11`)
   derives slots arithmetically, which collides once an entity has more fields than the offset.
   The binder allocates slots densely on first reference instead. Native rows become plain
   `Vec<Value>` arrays indexed by slot rather than maps, and scans read only referenced fields.
5. **Explicit versions at the boundary.** Plans travel in a `{"wire_version":2,...}` envelope with
   unknown fields rejected; batches use format v2 (optional row ids, slot column ids); the C ABI is
   version 4 with `RESOURCE_LIMIT` and `ARITHMETIC_OVERFLOW` statuses. A mismatched JVM and native
   library now fail with a clear message instead of misreading each other. A fixture shared by a
   Scala and a Rust test pins the wire format from both sides.
6. **Adaptive, intent-driven seams instead of guesses.** Without statistics (2.2) a cost model
   would be guesswork, so 2.1.3 builds the two halves of the adaptive loop:
   *decide and explain* (`PlanningPolicy` + `PlanDecision`, shown by `EXPLAIN`) and *observe*
   (per-operator runtime profiles, `adb_query_profile_json`, shown by `EXPLAIN ANALYZE`). Future
   statistics-, workload- or intent-driven policies replace the policy, not the planner.
7. **Predicate pushdown.** Not in the plan, but needed for joins to be efficient: WHERE and ON
   conjuncts move into join inputs where the join semantics allow (never into the null-filled side
   of a LEFT JOIN), so a primary-key equality on a join input still becomes a point lookup.
8. **One version number.** Engine, crates, JVM build and ABI report `2.1.3` instead of the
   previous `0.2.x` mapping.
9. **No version-named wrappers.** The plan added `run-demo-2.1.2.sh` and version-named benchmark
   packages next to the old ones; 2.1.3 updates `demo/run-demo.sh` and renames the benchmark
   package to `adb-benchmark-rust` so later milestones do not multiply scripts.

## Supported SQL (2.1.3)

```sql
SELECT [*] | item [, item ...]                       -- item: [alias.]column | COUNT(*) |
                                                     --   COUNT|SUM|MIN|MAX|AVG([alias.]column)
                                                     --   each optionally AS name
FROM table [[AS] alias]
  { [INNER] JOIN | LEFT [OUTER] JOIN } table [[AS] alias] ON condition
  | CROSS JOIN table [[AS] alias]
[AS OF VERSION ts]
[WHERE condition]                                    -- comparisons, AND, OR, NOT, literals
[GROUP BY [alias.]column, ...]
[ORDER BY output_name | [alias.]column | aggregate [ASC|DESC], ...]
[LIMIT n]                                            -- n <= 1,000,000
```

Rules: unqualified columns must be unambiguous; an `ON` condition sees only the tables joined so
far; with GROUP BY or aggregates every selected column must be grouped; `SELECT *` cannot be
combined with aggregation; aggregates are not allowed in WHERE or ON. Not yet supported (see the
roadmap): computed SELECT expressions, `HAVING`, `IS NULL`, `IN`, `DISTINCT`, RIGHT/FULL joins.

## Known limitations

* AND / OR treat NULL as false (no three-valued logic yet).
* Blocking operators fail with `RESOURCE_LIMIT` instead of spilling to disk.
* The hash join always builds on the right input (no statistics yet; LEFT JOIN requires it anyway).
* The JVM gateway materializes results (up to 1,000,000 rows).
