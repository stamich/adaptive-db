# Cost-based optimizer (Milestone 2.2.3)

The planner of 2.1.3 decided by rules: joins ran in SQL order, the right input was always built,
and LIMIT over ORDER BY always became a TopK. Since 2.2.3 it decides by estimated cost, using the
engine's statistics ([statistics.md](statistics.md)). It explains every decision, and EXPLAIN
ANALYZE checks every estimate against what the engine measured.

```text
SQL → parse → bind → LogicalPlanner → RuleOptimizer (pushdown, point lookups)
    → JoinReorderRule (cost mode)            ┐
    → PhysicalPlanner + CostBasedPolicy      ├─ CardinalityEstimator + CostModel
      (or DefaultPlanningPolicy, rule mode)  ┘   over StatisticsProvider
    → plan wire v2 → engine → runtime profile (node ids) → EXPLAIN ANALYZE q-error, feedback log
```

`SET optimizer = cost` (the default) or `SET optimizer = rule` selects the mode per session. Rule
mode is the 2.1.3 planner. It is kept for comparison and as a fallback if a plan gets worse
after `ANALYZE`, and EXPLAIN still shows estimates in it.

## Cardinality estimation (`adb-optimizer/cardinality`)

Every logical operator gets an `Estimate(rows, confidence, source)`. `source` is the weakest
input: fresh statistics, stale statistics, or defaults.

| Operator / predicate | Estimate |
|---|---|
| scan | analyzed row count; without statistics 1,000 rows at confidence 0.1 |
| `pk = v` | `1 / rows` (a point lookup: at most one row) |
| `col = v` | most common value: its sampled frequency; outside `[min, max]`: 0; otherwise `(nonNull − mcvRows) / (distinct − mcvCount)` rows per value |
| `<`, `<=`, `>`, `>=` | histogram fraction (linear interpolation inside a numeric bucket, half a bucket otherwise); without a histogram, min/max interpolation |
| no column statistics | equality 0.1, range 1/3 |
| `AND`, `OR`, `NOT` | `a·b`, `a + b − a·b`, `1 − a` (independence) |
| `a = b` (columns) | `1 / max(distinct)` × both non-NULL fractions |
| inner join with foreign-key hint | `|child| · (1 − nullFraction) · |parent side| / |parent table|` (below 1× for a filtered parent side, above 1× when the parent side repeats parent rows) |
| other inner join | `|L| · |R| / max(distinct(l), distinct(r))` per key; distinct counts capped by their side's rows; default 200 distinct values without statistics |
| LEFT JOIN | at least `|L|` |
| CROSS JOIN | `|L| · |R|` |
| GROUP BY | product of the grouping columns' distinct counts, capped by the input; one row without GROUP BY |
| LIMIT | `min(limit, input)` |

Except for LIMIT, estimates never fall below one row when the input has rows, because a zero
would make every plan above it look free. Confidence is multiplied down for every
uninformed step: 0.9 per filter with statistics and 0.5 without, 0.95 for a foreign-key join,
0.8 or 0.5 for other joins, 0.8 or 0.5 per aggregate. EXPLAIN shows the **basis** of each
filter and join estimate: `primary key`, `mcv`, `ndv`, `histogram`, `min/max`, `foreign key`,
`cross product` or `default`.

Foreign keys are declared as hints and never checked:

```sql
CREATE TABLE orders (id BIGINT PRIMARY KEY,
                     customer_id BIGINT NOT NULL REFERENCES customer(id) NOT ENFORCED, ...);
```

The target must be another existing table's primary key with the same type. `NOT ENFORCED` is
mandatory, so the syntax never suggests that integrity is checked.

## Cost model (`adb-optimizer/cost`)

`Cost(cpu, io, memoryBytes)` with saturating arithmetic (every component is clamped to
`[0, 10^18]`, and NaN counts as the maximum), combined by `CostWeights` (cpu 1, io 8 per 8 KiB
page, memory 0.0001 per byte):

| Operator | cpu | io | memory |
|---|---|---|---|
| scan | rows | rows · width / 8 KiB | – |
| point lookup | 1 | 1 | – |
| filter | 0.25 · input · conjuncts | – | – |
| hash join | 2 · build + probe + 0.5 · output | – | build · width |
| nested-loop join | 0.5 · outer · inner + 0.5 · output | – | inner · width |
| aggregate | 1.5 · input + groups | – | groups · width |
| sort | rows · log2(rows) | – | rows · width |
| TopK | rows · (1 + log2(k)) | – | k · width |

Widths come from the column statistics, or from type defaults when there are none, plus 16
bytes per slot.

**Feasibility.** `EngineLimits` mirrors the engine's `ExecutionLimits`: 256 MiB per query,
1,000,000 materialized rows, a fanout of 16,384 and 10,000,000 nested-loop comparisons. Every
alternative is checked against these limits. A plan within them always beats one outside them;
when nothing fits, the cheapest plan runs and EXPLAIN warns that it may be aborted. The engine
still enforces the limits at run time.

## Join ordering (`adb-optimizer/join`)

`JoinGraph` flattens each maximal block of INNER and CROSS joins into its relations and the
conjuncts of their conditions. LEFT JOINs, aggregates and other operators are boundaries: their
inputs are reordered on their own, but nothing moves across them, so null-filling stays
correct. A conjunct that reads a single relation (normally pushed down already) stays on that
relation as a filter. Each block is ordered as follows:

| Relations | Method |
|---|---|
| ≤ 10 | dynamic programming over subsets (bushy trees), connected splits only unless the block is disconnected |
| ≤ 32 | greedy: repeatedly join the connected pair with the smallest estimated result |
| > 32 | SQL order |

Every candidate join is costed with its cheapest strategy and build side. Conditions are attached
to the lowest join that sees all their relations. Operators above a join address columns by slot,
so the result is unaffected by the new attribute order
(`JoinOrderPropertiesTest.reorderingPreservesResults`). The DP is checked against exhaustive
enumeration on random graphs (`dynamicProgrammingIsOptimal`).

## Strategies (`CostBasedPolicy`)

* **Joins**: it costs a hash join (when an equality key exists) and a nested-loop join, with every
  allowed build side, and picks the cheapest alternative within the limits. For INNER and CROSS
  joins the smaller input is built, which means **swapping the inputs** when it is the logical left
  one (the engine always builds its right input). A LEFT JOIN never swaps.
* **ORDER BY + LIMIT**: it uses a TopK whenever the engine accepts the limit (≤ 1,000,000), because
  its memory is bounded by the limit rather than by the input.

Every decision is a `PlanDecision` with its reason. The reason includes the cost, the build side,
the estimated rows and confidence, and the next-best alternative.

## EXPLAIN and EXPLAIN ANALYZE

`PlannedQuery.estimates` maps the **pre-order id** of every physical node to its
`NodeEstimate(rows, cost, confidence, source, basis)`. The engine numbers its runtime profile
the same way (`OperatorProfile.node_id`, see [execution.md](execution.md#runtime-profile)), so
estimates and actuals are joined by id:

```text
Physical (optimizer=cost):
[0] Project [...] (est. rows=1 cost=1137.1 conf=0.65)
  [1] Aggregate ... (est. rows=4 cost=1136.9 conf=0.65)
    [2] HashJoin INNER keys=[s.shop_id#0 = sh.id#1] (est. rows=100 cost=982.9 conf=0.81 basis=foreign key)
...
Runtime profile:
[2] hash_join rows=100 est=100 q=1.0 batches=1 time=0.36ms build_rows=10 ...
max q-error 4.0 at [1] aggregate
```

The q-error is `max(est, actual) / min(est, actual)`, with both counted as at least one row. EXPLAIN
also lists the join-order decisions and every table's statistics status, and warns about stale
or missing statistics and infeasible plans.

**Feedback log.** With a `PlannerFeedbackLog`, which the CLI writes to
`<data>/planner-feedback.jsonl`, every executed SELECT appends one JSON line: a SHA-256 prefix
of the SQL (literals never reach the log), the mode, and estimated rows, actual rows and q-error
per node. The log rotates to `.1` at 4 MiB, and write failures never fail a query.

## Configuration

`OptimizerConfig(estimation, weights, limits, maxDpRelations = 10, maxGreedyRelations = 32)`.
It is also where declared intents (latency first, memory first, ...) will plug in later.

## Calibration

`scripts/benchmark-ffi.sh` reports, for each workload, the estimated root cost next to the
measured time (`calibration.ms_per_1k_cost`). On the reference VM workloads A and B land at
0.60 and 0.68 ms per 1,000 cost units. That is consistent within 15%, so the default weights
were kept. A machine whose ratios diverge between workloads should adjust `CostWeights`.

## Known limitations

* Most-common-value matching compares values numerically across BIGINT and DOUBLE and by content
  for BYTES.
* Independence between predicates and between join keys: correlated columns are
  under-estimated. Confidence drops with every such assumption, and the q-error shows where it
  matters.
* A filter does not narrow the distinct count of its column for a later GROUP BY. For example,
  `WHERE region = 'x' GROUP BY region` is estimated with every region (the demo shows q = 4).
* No multi-column statistics, no automatic `ANALYZE`, no statistics of historical versions.
* Sort elision by key order waits for the order-preserving key encoding of 2.3.
