# Roadmap after Milestone 2.0

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

## 2.1 — relational execution
- HashJoin and NestedLoopJoin
- Aggregate / GroupBy
- Sort / TopK
- expression functions
- EXPLAIN ANALYZE operator metrics

## 2.2 — statistics and cost-based optimizer
- row counts / NDV / min-max / histograms
- cardinality estimation
- cost model
- join ordering
- persisted statistics

## 2.3 — secondary indexes
- secondary B+Tree
- IndexScan
- covering index metadata
- cost-based SeqScan vs IndexScan

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
- workload telemetry
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
