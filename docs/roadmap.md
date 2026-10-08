# Roadmap

## 2.0.3 — log as source of truth (done)
- canonical log retained; projections rebuildable (`rebuild_projections`)
- journaled atomic checkpoints, no-steal buffer pool, group commit, poisoning
- space reuse in the current heap, tombstone vacuum
- streaming entity key-range scans
- serializable isolation (OCC read-set validation) by default
- native change data capture with durable consumer offsets (C ABI v3)

## 2.0.x follow-ups
- log retention driven by consumer offsets; archival of old segments to object storage
- version-store retention (`Order.history: 7 years`) via immutable, droppable segments
- push/long-poll change subscriptions
- predicate (range) read sets for phantom-free serializable scans inside transactions

## 2.1.3 — relational execution (done)
- slot-addressed execution (`RelationId` / `SlotId`), self-join safe
- HashJoin and NestedLoopJoin (INNER / LEFT, CROSS), predicate pushdown
- Aggregate / GROUP BY with COUNT, SUM, MIN, MAX, AVG (checked INT64 arithmetic)
- Sort / TopK
- query memory tracker and work limits
- plan wire v2 (`adb-plan-wire`), batch format v2, C ABI v4
- explained planning decisions (`PlanningPolicy`) and EXPLAIN ANALYZE runtime profiles

## 2.1.x follow-ups
- computed expressions in SELECT (arithmetic, functions), `HAVING`, `IS [NOT] NULL`, `IN`,
  `DISTINCT`, ordinal ORDER BY
- three-valued logic for AND / OR / NOT (today NULL behaves as false)
- spilling for blocking operators once the memory budget is exceeded
- right/full outer joins; a left-build hash join for LEFT JOIN
- streaming JVM results instead of materializing up to 1,000,000 rows in the gateway

## 2.2.3 — statistics and cost-based optimizer (done)
- native `ANALYZE`: row counts, NULLs, min/max, NDV (HyperLogLog), histograms, most common values
- statistics persisted by the engine; per-entity modification counters through the checkpoint journal
- `REFERENCES ... NOT ENFORCED` foreign-key hints
- cardinality estimation with confidence; cost model with engine-limit feasibility
- join ordering (DP ≤ 10, greedy ≤ 32) inside INNER blocks; cost-based build side and strategies
- EXPLAIN estimates, EXPLAIN ANALYZE q-error by profile node id, planner feedback log; C ABI 5

## 2.2.x follow-ups
- automatic `ANALYZE` driven by the modification counters
- multi-column (correlated) statistics; distinct counts narrowed by filters
- refitting `CostWeights` per machine from the feedback log
- feeding q-errors back into planning (adaptive re-planning)

## 2.3 — secondary indexes and order-preserving keys
- **order-preserving key encoding** (sign-flipped BIGINT keys) with a one-time storage migration,
  done together with the index key format so storage migrates once
- secondary B+Tree
- IndexScan
- covering index metadata
- cost-based SeqScan vs IndexScan
- Sort elision from key and index order (needs the new key encoding)

## 2.4 — schema evolution v1
- ADD/RENAME/logical DROP column
- versioned catalog snapshots
- virtual defaults
- compatibility checks

## 3.x — distribution
- 3.0 replicated single partition
- 3.1 LogicalPartition + RoutingTable + TopologyEpoch
- 3.2 Transaction Domains
- 3.3 cross-domain transactions
- 3.4 metadata/control plane

## 4.x — HTAP
- canonical-log projection framework (2.0.3 provides the log cursor and transaction assembler)
- column segments
- vectorized analytics
- projection watermarks and delta repair

## 5.x — adaptive physical design
- workload telemetry (built on the per-operator runtime profiles of 2.1)
- intent-driven planning: declared intents (latency, memory, freshness) as `PlanningPolicy` inputs
- projection/index advisors
- adaptive partition split/merge/move
- transaction affinity graph

## 6.x — integrated access structures
- 6.0 full-text search projection
- 6.1 graph adjacency projection
- 6.2 vector ANN projection

## 7.x — storage/geo
- RAM/NVMe/SSD/object tiering
- residency and failure domains
- geo replicas

## 8.x — ML advisory plane
Python may predict workload/hotspots/costs, but semantic validation stays in Scala and deterministic execution/correctness stays in Rust.
