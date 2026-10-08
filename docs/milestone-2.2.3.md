# Milestone 2.2.3: statistics and cost-based optimization

2.2.3 implements the 2.2 plan (statistics, cardinality estimation, a cost model and join
ordering) on top of 2.1.3. It also adopts the identical-workload benchmarks of 2.2.2 and the
hardening limits of 2.2.1. This document records how the 2.2.3 proposal was adapted and why.
The design is described in [statistics.md](statistics.md) and [optimizer.md](optimizer.md).

## Decisions taken before implementation

1. **Statistics are stored natively in Rust.** The engine computes them, persists them next to
   the data and owns the modification counters. The JVM decodes and caches them. 2.2.2 kept them
   in a Scala `FileStatisticsStore`, but staleness needs counts only the commit pipeline sees.
2. **Foreign-key hints are included.** `REFERENCES t(c) NOT ENFORCED` is stored in the catalog
   and used for join estimates. The engine never checks it.
3. **Order-preserving key encoding moves to 2.3.** BIGINT primary keys are stored as unsigned
   (`-1` sorts after `2^63 − 1`), so a scan in key order is not in value order for negative keys.
   Fixing that changes the storage key format, which 2.3 changes anyway for secondary indexes.
   Doing both together costs one storage migration instead of two. Until then, Sort elision is
   not attempted.

## Scope

| Area | Delivered |
|---|---|
| Rust `adb-stats` | model, options, collector (exact counts, HyperLogLog, reservoir sample, histograms, MCVs), limits |
| Engine | `Database::analyze`, `statistics`, `modifications_since_analyze`; `stats/` directory; counters through the checkpoint journal; `DbError::Statistics` |
| Execution | `OperatorProfile.node_id` (pre-order), `PhysicalPlan::node_count` |
| C ABI 5 | `adb_analyze_entity_json`, `adb_statistics_json`, `adb_modifications_since_analyze`; Java binding |
| JVM `adb-statistics` | model, strict codec, `EntityStatistics` (freshness, confidence), `StatisticsProvider` |
| SQL / catalog | `ANALYZE [table]`, `SET optimizer = cost \| rule`, `REFERENCES t(c) NOT ENFORCED`, `Field.references` |
| Optimizer | `CardinalityEstimator`, `CostModel`, `OptimizerConfig` / `EngineLimits`, `JoinGraph`, `JoinReorderRule` |
| Physical planning | `CostBasedPolicy`, `JoinChoice` (build-side swap, warnings), `NodeEstimate` per node id |
| Gateway | cost / rule mode, EXPLAIN estimates, statistics and warnings, EXPLAIN ANALYZE q-error, `PlannerFeedbackLog` |
| Benchmarks | workloads A and B on `rust_native`, `ffi_prepared_plan` and `scala_cbo_ffi_rust`; join-order workload; calibration |
| Demo / build | tour phase 2.2.3, `build-milestone-2.2.3.sh`, `scripts/benchmark-db.sh`, `scripts/benchmark-ffi.sh` |

## Changes to the proposal

1. **No plan wire v3.** The proposal added an optional `id` to every plan node and had the
   profile echo it. The executor already builds exactly one operator per plan node, so the
   operator's pre-order position *is* a stable node id. The engine numbers the profile in
   pre-order, and the JVM numbers its physical plan the same way (`PhysicalPlan.preorder`).
   Plan wire v2 and its cross-language contract stay untouched. If a future engine rewrites
   plans (fusing or splitting operators), explicit ids become necessary and wire v3 is still
   the way to add them.
2. **A full scan per `ANALYZE`, with sampling only for histograms and MCVs.** Storage has no
   block sampling, so exact counts cost nothing extra once the scan runs. The working set stays
   bounded (64 MiB by default) because distinct values switch from a hash set to HyperLogLog at
   10,000.
3. **Cost weights were checked, not refitted.** The benchmark reports the measured time per cost
   unit for each workload. It was consistent within 15% on the reference VM, so the defaults
   stayed. Fitting cpu, io and memory weights separately needs workloads that stress them
   separately, which is follow-up work.
4. **`Cost` has no network component.** Nothing in 2.2.3 crosses a network; the field arrives
   with distribution (3.x).
5. **Statistics status in EXPLAIN** is a section per table (fresh, stale or missing, with the
   counts) plus the per-node basis (`mcv`, `histogram`, `ndv`, `foreign key`, ...), rather
   than a single source per filter.

## Definition of done

| Item | Proven by |
|---|---|
| `ANALYZE` yields rows, NULLs, NDV, min/max, histogram, MCVs | `collector::small_table_is_exact`, `statistics::analyze_persists_document` |
| skewed column PL 50% / DE 25% / US 25% within 5% | `collector::skewed_column_reports_most_common_values` |
| HyperLogLog within 3% (up to 1,000,000 values) | `hll::tests`, `collector::large_table_uses_sketch_within_error_bound` |
| statistics survive restart and interrupted publication | `statistics::analyze_persists_document`, `statistics::interrupted_publication_keeps_the_previous_document` |
| counters exact after recovery and rebuild | `statistics::modification_counters_track_commits`, `statistics::rebuild_recounts_modifications` |
| stale warning above 20% changed rows | `StatisticsCodecTest.freshnessAndConfidence`, `CostBasedPlanningTest.warnsAboutStatistics` |
| selectivity rules (PK, MCV, NDV, histogram, AND/OR/NOT) | `CardinalityEstimatorTest` |
| FK join ≈ child rows; NDV join; GROUP BY capped | `CardinalityEstimatorTest.foreignKeyJoin`, `.distinctCountJoinAndLeftJoin`, `.aggregates` |
| hash join vs nested loop; TopK vs sort | `CostModelTest.hashVersusNestedLoop`, `.cumulativeCostsAndTopK` |
| DP equals the exhaustive minimum (≤ 6 relations) | `JoinOrderPropertiesTest.dynamicProgrammingIsOptimal` |
| A 1,000,000 / B 10,000 / C 100 → `(B ⋈ C) ⋈ A` | `JoinOrderPropertiesTest.goldenThreeTableOrder` |
| reordering keeps results, LEFT JOIN included | `JoinOrderPropertiesTest.reorderingPreservesResults`, `JoinReorderRuleTest.leftJoinIsABoundary` |
| an over-budget build is avoided | `CostBasedPlanningTest.infeasibleBuildIsAvoided` |
| EXPLAIN estimates; node ids over the ABI | `CostBasedGatewayTest.costModeExplain`, `relational::join_aggregate_runs_and_reports_a_profile` |
| identical results on all benchmark paths | equal `checksum`s in `examples/results/2.2.3-*.json` |
| CBO beats SQL order on the join-order workload | `join_order` in `examples/results/2.2.3-ffi.json` |

## Supported SQL (2.2.3 additions)

```sql
CREATE TABLE sale (id BIGINT PRIMARY KEY,
                   shop_id BIGINT NOT NULL REFERENCES shop(id) NOT ENFORCED,
                   amount BIGINT NOT NULL);
ANALYZE sale;                 -- or ANALYZE; for every table
SET optimizer = rule;         -- the 2.1.3 planner; SET optimizer = cost to return
EXPLAIN SELECT ...;           -- estimates, join order, statistics, warnings
EXPLAIN ANALYZE SELECT ...;   -- plus actual rows and q-error per operator
```

Everything from [milestone-2.1.3.md](milestone-2.1.3.md#supported-sql-213) still applies.

## Out of scope

Secondary indexes and `IndexScan` (2.3), spill to disk (2.5), parallel execution, automatic
`ANALYZE`, incremental and multi-column statistics, and learned cost models.
