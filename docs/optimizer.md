# Cost-based optimizer (Milestones 2.2.3 and 2.3.0)

The planner of 2.1.3 decided by rules: joins ran in SQL order, the right input was always built,
and LIMIT over ORDER BY always became a TopK. Since 2.2.3 it decides by estimated cost, using the
engine's statistics ([statistics.md](statistics.md)). Since 2.3.0 it also chooses how each table
is read: through a secondary index ([indexes.md](indexes.md)) or by a sequential scan. It explains
every decision, and EXPLAIN ANALYZE checks every estimate against what the engine measured.

```text
SQL → parse → bind → LogicalPlanner → RuleOptimizer (pushdown, point lookups)
    → JoinReorderRule (cost mode)            ┐
    → PhysicalPlanner + CostBasedPolicy      ├─ CardinalityEstimator + CostModel
      (access paths, join strategies, order) │   over StatisticsProvider + IndexCatalog
      (or DefaultPlanningPolicy, rule mode)  ┘
    → plan wire v3 → engine → runtime profile (node ids) → EXPLAIN ANALYZE q-error, feedback log
```

`SET optimizer = cost` (the default) or `SET optimizer = rule` selects the mode per session. Rule
mode is the 2.1.3 planner and ignores indexes. It is kept for comparison and as a fallback if a plan gets worse
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

`Cost(cpu, io, memoryBytes, hashRows)` with saturating arithmetic (every component is clamped to
`[0, 10^18]`, and NaN counts as the maximum), combined by `CostWeights` (cpu 1, io 4,500 per
8 KiB page of row data, memory 0.0001 per byte, hash row 1; fitted, see [Calibration](#calibration)):

| Operator | cpu | hash rows | io | memory |
|---|---|---|---|---|
| scan | rows | – | rows · width / 8 KiB | – |
| point lookup | 1 | – | 1 | – |
| filter | 0.25 · input · conjuncts | – | – | – |
| hash join | 0.5 · output | build + probe | – | build · width |
| nested-loop join | 0.5 · outer · inner + 0.5 · output | – | – | inner · width |
| aggregate | 1.5 · input + groups | – | – | groups · width |
| sort | rows · log2(rows) | – | – | rows · width |
| TopK | rows · (1 + log2(k)) | – | – | k · width |

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
| ≤ 10 | dynamic programming over connected subsets (bushy trees, no cross products); a disconnected block is ordered per component and the components are cross-joined, smallest first |
| ≤ 32 | greedy: repeatedly join the connected pair with the smallest estimated result |
| > 32 | SQL order |

Every candidate join is costed with its cheapest strategy and build side. Conditions are attached
to the lowest join that sees all their relations. Operators above a join address columns by slot,
so the result is unaffected by the new attribute order
(`JoinOrderPropertiesTest.reorderingPreservesResults`). The DP is checked against exhaustive
enumeration of the same search space on random graphs (`dynamicProgrammingIsOptimal`).
Planning a 10-relation chain takes about 2 ms and a 10-relation star about 10 ms
(`planner` section of the workload benchmark).

## Strategies (`CostBasedPolicy`)

* **Joins**: it costs a hash join (when an equality key exists) and a nested-loop join, with every
  allowed build side, and picks the cheapest alternative within the limits. For INNER and CROSS
  joins the smaller input is built, which means **swapping the inputs** when it is the logical left
  one (the engine always builds its right input). A LEFT JOIN never swaps.
* **ORDER BY + LIMIT**: it uses a TopK whenever the engine accepts the limit (≤ 1,000,000), because
  its memory is bounded by the limit rather than by the input, unless the chosen access path
  already produces the order (see below).

Every decision is a `PlanDecision` with its reason. The reason includes the cost, the build side,
the estimated rows and confidence, and the next-best alternative.

## Access paths (2.3.0)

For every relation, `CostBasedPolicy.chooseAccess` compares the sequential scan with every index
of the table that serves the query snapshot (`AS OF VERSION` included; see
[indexes.md](indexes.md#lifecycle)):

1. **Matching** (`IndexPredicateMatcher`). Conjuncts `column op literal` (`=`, `<`, `<=`, `>`,
   `>=`, either side) whose literal converts exactly to the column type form the bounds.
   Equalities on the leading key columns become the prefix. The first key column without an
   equality may take a range, and the tightest of several comparisons wins. Everything else
   (and a comparison after the range column) stays a residual filter, which runs right after
   the scan and, for non-covering scans, *before* the row fetch.
2. **Enumeration** (`AccessPathEnumerator`). An index is a candidate if it has bounds, or if it
   produces a required order (`ORDER BY` on its key columns, forward or backward; key columns
   bound by equality may be skipped). A scan is **covering** when every column the query reads
   is a key or `INCLUDE` column; otherwise a `RowFetch` adds the rest.
3. **Costing** (`CostModel.accessCost`). Index entries cost a row operation each plus their pages
   (encoded key + 40 bytes per entry). A non-covering scan adds, per matched row, a random
   access (weight `randomIo`) scaled by `1 − pk_locality` plus the page reads of the fetched
   rows. `matched` comes from `CardinalityEstimator.indexSelectivity`, which floors the
   independence estimate of a multi-column equality prefix by the index's distinct-prefix
   counts. Without index statistics, every fetch counts as a random access.
4. **Order**. With a required order, a path that produces it costs only the fraction of its rows
   a `LIMIT` needs. Every other path is charged the sort, or the TopK under a LIMIT, that it
   would still need (`CostModel.sortCost`). The primary-key order of the sequential scan counts
   too, because since data format 2 rows are stored in primary-key order. When the winner
   produces the order, the planner drops the Sort (decision `access order`) or replaces the TopK
   with a Limit over the scan (decision `access order + limit`).

The selectivity sweep of the workload benchmark
([examples/results/README.md](../examples/results/README.md)) runs a non-covering range from
0.01% to 50% of the table as an index plan and as a sequential plan, at 100,000 and 1,000,000
rows. Cost mode picked the faster plan at every point. The only exception was 50% at scale 1,
where both plans take the same time within noise and the chosen sequential plan was 6% slower.

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

**Index access (2.3.0).** In cost mode EXPLAIN names indexes and prints their bounds by column
instead of by id and position. A scan with no `RowFetch` above it is marked `covering`, which
means it answers from index entries alone. The access decision lists the next best path and up
to three more rejected ones, each with its cost. An `Indexes:` section shows every index of the
tables read, the range of snapshots it serves (or that it cannot serve an `AS OF` snapshot) and
its statistics from the last `ANALYZE`, or a hint to run it:

```text
[2] IndexScan idx_balance [balance >= 30000] backward limit=3 covering [f2->account.balance#0, ...]
- access account: index_scan idx_balance backward (covering) (cost 227.9, est. rows=1; produces the
  required order; next best index_scan idx_balance (covering) + top_k cost 229.9; also rejected
  entity_scan + top_k cost 28323.4, ...)
Indexes:
- idx_balance ON account (balance) INCLUDE (owner): serves ts 1000..1000; 1000 entries (1000 stored),
  height 2, 4 leaf pages, prefix ndv [1000], pk locality 0.00
```

EXPLAIN ANALYZE adds a line with the index operators' totals:
`index access: 2 pages, 40 entries examined, 25 returned (63% visible); row fetch: 1 batches, 25 rows`.
Entries that are examined but not returned are versions invisible at the snapshot, or entries
beyond a limit. A low visible share points at history that the next `VACUUM` can remove.

**Feedback log.** With a `PlannerFeedbackLog`, which the CLI writes to
`<data>/planner-feedback.jsonl`, every executed SELECT appends one JSON line: a SHA-256 prefix
of the SQL (literals never reach the log), the mode, and estimated rows, actual rows and q-error
per node. The log rotates to `.1` at 4 MiB, and write failures never fail a query.

## Configuration

`OptimizerConfig(estimation, weights, limits, maxDpRelations = 10, maxGreedyRelations = 32)`.
It is also where declared intents (latency first, memory first, ...) will plug in later.

## Calibration

`scripts/benchmark-ffi.sh` records a calibration point for every measured plan: the unweighted
cost components of the plan's root (`cpu`, `io`, `memory_bytes`, `hash_rows`, and since 2.3.0
`random_io`) and the plan's
median engine time on the prepared-plan path (planning excluded). The points cover join plans
(workloads A and B, the chain and star join orders in both modes) and scan-only plans (the
estimation queries: filter and aggregate, nothing hashed), so row reads, row operations and
hashed rows can be told apart. Since 2.3.0 the points also include workloads C–E and both plans
of every selectivity-sweep point, so random accesses can be told apart too.
`scripts/fit-cost-weights.py REPORT.json ...` fits

```text
engine_ms ≈ c + a · (cpu + memory · w_memory) + d · io + b · hash_rows + r · random_io
```

by non-negative least squares on the relative error (`c` is the fixed cost of a query: plan
decoding, the FFI round trips) and prints the weights relative to one row operation:
`io = d / a`, `hash_row = b / a`, `random_io = r / a`. Several reports can be pooled.

Fitted on the reference VM over three runs (scale factors 1 and 2), a page of row data takes
about 140 µs (walking the primary index and decoding about 200 rows) and a row operation about
30 ns: `io = 4,545`, rounded to **4,500** (it was 8). Hashing adds no measurable time beyond the
row operations already counted, so a hashed row is kept at **1** row operation (the 2.2.3
formula counted `2 · build + probe`). Under the 2.2.3 weights, scan-heavy plans ran 4–5× slower
per cost unit than join and TopK plans; with the fitted weights the spread is about 2× (the
remaining variation is mostly run-to-run noise of the shared VM; the row-operation time is the
least well determined term, because in these workloads the rows a plan reads and its row
operations grow together). The decisions the benchmarks exercise are unchanged by the refit.
**Random accesses (2.3.0).** Fitted on the 2.3.0 reports at scales 1 and 10, a random access (an
index descent plus a row fetched by key) takes about 0.7 µs, or 50–56 row operations. The same
fit also moves `io` to about 11,000 and `hash_row` to 0, and on these points it spreads the
engine time per cost unit more than the current weights do (41 vs 26). The weights therefore
stay at `io = 4,500`, `hash_row = 1` and `random_io = 100`. With them, the modelled crossover
between index and sequential plans matches the measured one at both scales: cost mode chose the
faster plan at every sweep point, as noted above. More index-heavy workloads should settle
`random_io`. Re-fit after changing the storage engine or the cost formulas.

## Known limitations

* Most-common-value matching compares values numerically across BIGINT and DOUBLE and by content
  for BYTES.
* Independence between predicates and between join keys: correlated columns are
  under-estimated. Confidence drops with every such assumption, and the q-error shows where it
  matters.
* A filter does not narrow the distinct count of its column for a later GROUP BY. For example,
  `WHERE region = 'x' GROUP BY region` is estimated with every region (the demo shows q = 4).
* No multi-column statistics, no automatic `ANALYZE`, no statistics of historical versions.
* Access paths use one index per relation: no index intersection or union, and `OR`, `IN`,
  `IS NULL` and expressions over columns are not matched.
* Index nested-loop joins (probing an index per outer row) are not planned yet; joins still
  hash or nested-loop over scans.
