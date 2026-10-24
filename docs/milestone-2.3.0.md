# Milestone 2.3.0: secondary indexes and access paths

2.3.0 implements Milestone 2.3 on top of 2.2.3: secondary indexes, the order-preserving key
encoding deferred from 2.2.3, and cost-based access paths. It follows the 2.3.3 proposal, which
consolidated the 2.3, 2.3.1 and 2.3.2 plans. The release is numbered 2.3.0 because from this
milestone on the minor number is the milestone and the patch number is for fixes (see
`CHANGELOG.md`). This document records how the proposal was implemented and adapted. The design
is described in [indexes.md](indexes.md) and [optimizer.md](optimizer.md#access-paths-230).

## Decisions taken before implementation

1. **Indexes are projections of the log**, not separately logged structures. They are
   maintained inside the commit apply step from the row images every projection already
   receives, published by the same checkpoint journal and rebuilt from the log. The WAL format
   is unchanged, and an index can never be ahead of or behind the log.
2. **Entries are MVCC-versioned** (`begin_ts`/`end_ts`, like row versions) instead of
   current-only. That costs one extra entry per key-changing update, and in return `AS OF`
   queries can use indexes and no snapshot ever sees an index newer than its rows. `VACUUM`
   trims entries with the row history.
3. **One storage migration.** The order-preserving encoding changes the row id of every BIGINT
   primary key (data format 2), so it ships together with indexes. `adb-migrate` rewrites the
   log in one pass, keeping LSNs (CDC offsets stay valid), and rebuilds the projections. The
   engine refuses unmigrated directories instead of reading them wrongly.
4. **Synchronous builds.** `CREATE INDEX` blocks commits while it reads the table at one
   snapshot, then publishes the index with a checkpoint. Online (asynchronous) builds and
   asynchronous projections belong to the HTAP framework of 4.x
   ([roadmap.md](roadmap.md)).
5. **The JVM decides, the engine checks.** The planner chooses indexes from the catalog it reads
   over the ABI. The engine re-checks on every scan page that the index exists and serves the
   query snapshot, so a stale JVM view fails a query and never returns wrong rows.

## Delivered, task by task

Each task is one commit on the 2.3.0 branch, with its tests and documentation.

| # | Task | Main pieces |
|---|---|---|
| 1 | Refit cost weights | `Cost.hashRows`, fitted `io` / `hash_row`, `scripts/fit-cost-weights.py` |
| 2 | Order-preserving key encoding | `adb_core::key_encoding` |
| 3 | Data format 2 | `RowId::from_primary_key`, `format.meta`, `DbError::IncompatibleFormat` |
| 4 | Migration | `adb-wal::rewrite_log`, `adb-engine::migrate`, `adb-migrate` CLI |
| 5 | Byte-key B+Tree | `ByteTree`, `BPlusTree::get_many` |
| 6 | Index definitions and entries | crate `adb-index` |
| 7 | Index projection | `IndexProjection`, `IndexBounds` |
| 8 | Engine index catalog | `indexes/catalog.meta`, checkpointed |
| 9 | Maintenance, recovery, rebuild | `Projection` trait, commit apply, vacuum, verify |
| 10 | Index lifecycle | `create_index`, `drop_index`, crash-safe builds and drops |
| 11 | Index statistics | statistics format 2, `IndexCollector` |
| 12 | Index execution | `IndexAccess`, `IndexScan`, `RowFetch` |
| 13 | Plan wire v3 | `index_scan`, `row_fetch`, cross-language fixture |
| 14 | C ABI 6 | three index functions, status 12, Java binding |
| 15 | SQL DDL | `CREATE INDEX`, `DROP INDEX`, `SHOW INDEXES`, JVM `IndexCatalog` |
| 16 | JVM index statistics | codec, `indexSelectivity` |
| 17 | Predicate matching | `IndexPredicateMatcher`, `AccessPath`, `PhysicalProperties` |
| 18 | Cost-based access paths | `AccessPathEnumerator`, `CostModel.accessCost`, `CostBasedPolicy.chooseAccess` |
| 19 | Sort elision | ordered access paths replace Sort and TopK |
| 20 | EXPLAIN for access paths | index names, bounds by column, rejected paths, `Indexes:` section, profile summary |
| 21 | Hardening | bound validation, fuzz tests of encoder, entries, plan wire v3, JVM listing |
| 22 | Benchmarks | workloads C–E, index builds, selectivity sweep, write overhead, `AS OF`, calibration |
| 23 | Demo | tour phase 2.3.0 |
| 24 | Documentation and version | this document, version 2.3.0, `build-milestone-2.3.0.sh` |

## Changes to the proposal

1. **No separate index WAL records** (the proposal left this open). Maintenance from the
   commit's row images was enough, because recovery replays the same images into every
   projection.
2. **`pk_locality` instead of a correlation coefficient.** A rank correlation between index
   order and primary-key order says little about I/O. What matters is whether consecutive
   fetches land on the same leaf, so the statistic is the share of consecutive entries whose
   rows are at most 64 primary keys apart.
3. **The random-access weight was measured, not assumed.** The selectivity sweep produces
   calibration points of index plans. The cost model's crossover between index and sequential
   scan matches the measured one ([examples/results/README.md](../examples/results/README.md)).
4. **Asynchronous projections** stay a short section of the roadmap (4.x HTAP), as discussed
   before implementation; 2.3.0 only keeps the projection interface ready for them.
5. **One range per scan.** Index intersection, `IN` lists, `OR` and index nested-loop joins
   are left for later milestones. None of them needs a format change.

## Definition of done

| Item | Proven by |
|---|---|
| encoded keys order like values, in both directions | `key_encoding::tests::byte_order_matches_sql_order`, `key_encoding_fuzz::adversarial_values_keep_their_order` |
| primary-key order equals storage order; old directories refused | `format::*`, `ffi index::older_data_formats_are_reported` |
| migration keeps commits, LSNs and offsets | `migrate::migration_rewrites_keys_and_keeps_everything_else`, `adb-migrate cli::migrates_into_a_target_directory` |
| index scans equal a model at every snapshot | `projection::scans_at_any_snapshot_match_the_model` |
| commits, recovery, rebuild and vacuum maintain indexes | `index_maintenance::*` |
| builds and drops are crash-safe | `index_lifecycle::crash_during_build_leaves_no_index`, `::crash_after_drop_checkpoint_removes_leftover_files` |
| indexes serve exactly their snapshot range | `index_queries::unusable_snapshots_and_dropped_indexes_fail` |
| index statistics | `adb-stats index::counts_prefixes_and_order`, `StatisticsCodecTest.readsFormatTwo`, `CardinalityEstimatorTest.indexSelectivityUsesPrefixStatistics` |
| plan wire v3 contract | `adb-plan-wire jvm_contract`, `PlanJsonEncoderTest`, `fuzz_v3::*` |
| SQL DDL and catalog | `IndexSqlParserTest`, `IndexBinderTest`, `IndexCatalogTest`, `ffi index::create_list_drop` |
| covering scans, row fetches and residuals planned by cost | `AccessPathPlanningTest.selectiveCoveredEqualityUsesTheIndex`, `.selectiveRangeFetchesRowsAndPlacesResiduals`, `.unselectivePredicatesScanTheTable` |
| Sort and TopK elided only when the order is produced | `AccessPathPlanningTest.indexOrderReplacesTopK`, `.indexOrderReplacesSort`, `.primaryKeyAndConstantColumns` |
| EXPLAIN names indexes, bounds and alternatives | `AccessPathPlanningTest.explainNamesIndexesAndAlternatives`, `CostBasedGatewayTest.explainNamesIndexes`, `ProfileRendererTest.summarizesIndexAccess` |
| identical results on every path and plan | equal checksums in `examples/results/2.3.0-*.json`, checked by `scripts/check-benchmarks.py` |
| cost mode picks the faster access path | `selectivity_sweep` in `examples/results/2.3.0-ffi*.json` |

## Supported SQL (2.3.0 additions)

```sql
CREATE INDEX idx_owner ON account (owner) INCLUDE (balance);
CREATE INDEX idx_region_balance ON account (region, balance DESC);
SHOW INDEXES;                 -- or SHOW INDEXES ON account
DROP INDEX idx_owner;
EXPLAIN SELECT owner, balance FROM account WHERE owner = 'x';   -- covering IndexScan
SELECT id FROM account WHERE region = 2 ORDER BY balance DESC LIMIT 10;  -- no TopK
```

Everything from [milestone-2.2.3.md](milestone-2.2.3.md#supported-sql-223-additions) still
applies.

## Out of scope

Unique, partial and expression indexes; online builds; index intersection, `IN` / `OR` and
index nested-loop joins; bulk-loaded builds; spilling (a 2.1.x follow-up); asynchronous projections
(4.x).
