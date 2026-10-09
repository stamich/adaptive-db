# Optimizer statistics (Milestone 2.2.3)

Statistics are **derived data**. The canonical log stays the only source of truth. `ANALYZE`
rebuilds statistics, they may be deleted at any time, and a query never fails because they are
missing; it only plans with less confidence.

## Who does what

| Step | Where | What |
|---|---|---|
| Collect | Rust, `adb-stats::analyze` | one snapshot scan of one entity |
| Persist | Rust, `adb-engine::statistics` | `stats/entity-<id>.stats`, atomic replace |
| Count changes | Rust, commit pipeline | per-entity modification counters, checkpointed |
| Expose | C ABI 5 | `adb_analyze_entity_json`, `adb_statistics_json`, `adb_statistics_generation`, `adb_modifications_since_analyze` |
| Decode, judge freshness | JVM, `adb-statistics` | `StatisticsCodec`, `EntityStatistics`, `StatisticsProvider` |
| Use | JVM, `adb-optimizer` | cardinality estimation, cost model, join order |

Statistics live next to the data rather than in the JVM catalog. Only the commit pipeline sees
every mutation, so only the engine can say how stale a document is. Several JVM clients share
one engine, and crash-safe publication already exists there (the `adb-journal` envelope).

## `ANALYZE`

```sql
ANALYZE orders;   -- one table
ANALYZE;          -- every table of the catalog
```

`Database::analyze(entity_id, &AnalyzeOptions)` scans the entity's key range at the latest
published snapshot, in pages of 1,024 rows, without holding any lock that blocks commits. The
Java binding runs it outside the `NativeDatabase` monitor too, so other calls proceed during a
long `ANALYZE`; only `close()` waits for it. The engine has no catalog, so analyzing an entity
id without rows publishes an empty document. A
single pass collects the following:

| Statistic | How | Exact? |
|---|---|---|
| row count, average row size | every row | yes |
| per field: NULL count, min, max, average width | every row | yes |
| distinct values | hash set up to 10,000 values, then HyperLogLog (p = 12, 4,096 registers, ≈1.6% standard error) | `distinct_exact` says which |
| equi-depth histogram (≤ 64 buckets) | seeded reservoir sample (Algorithm R, 30,000 rows) | exact when the sample holds every row |
| most common values (≤ 32) | the same sample: values at least 1.25× more frequent than the average, seen at least twice | scaled from the sample |

Sampled counts are scaled to the full row count. The working set charges sampled rows at their
real in-memory size (40 bytes per field entry plus string payload and vector overhead), so
`max_working_set_bytes` bounds the actual footprint. Histogram buckets use cumulative rounding, so
their rows always add up to the non-NULL count. The reservoir is seeded with the entity id, so
two runs over the same data produce the same document. A field whose values change type gets
counts but no min/max, histogram or common values. NaN and infinite floats are counted but take
no part in min/max, histograms or common values (they have no order or no JSON form). Stored
values longer than 256 bytes are truncated.

### Options and limits

`AnalyzeOptions` (JSON over the ABI; unknown keys are rejected):

| Option | Default | Bound |
|---|---|---|
| `fields` | all fields | ≤ 1,024 fields |
| `sample_rows` | 30,000 | 1..=1,000,000 |
| `histogram_buckets` | 64 | ≤ 256 |
| `most_common_values` | 32 | ≤ 256 |
| `max_rows` | 100,000,000 | rows scanned |
| `max_working_set_bytes` | 64 MiB | sample + distinct sets |
| `deadline_ms` | none | checked per page |

An exceeded limit fails with `StatsError::Limit` (`ADB_RESOURCE_LIMIT` over the ABI) and
persists nothing; the previous document stays in force.

## The document

`stats/entity-<id>.stats` is a checksummed envelope (`ADBSTA01`) around a JSON document (at
most 4 MiB, `deny_unknown_fields`):

```json
{"format_version":1,"entity_id":2,"row_count":1000,"avg_row_bytes":40.0,"analyzed_at_ts":1105,
 "modifications_at_analyze":1000,"sampled_rows":1000,"exact":true,
 "columns":[{"field_id":2,"null_count":0,"distinct_count":100,"distinct_exact":true,
   "min":{"Int64":1},"max":{"Int64":100},"avg_width_bytes":8.0,
   "histogram":[{"lower":{"Int64":1},"upper":{"Int64":2},"rows":20,"distinct":2}],
   "most_common":[]}]}
```

It is written with `replace_file` (write to `.tmp`, `fsync`, rename, directory `fsync`). Publication is
serialized, and a document of an older snapshot never replaces a newer one, so concurrent
`ANALYZE`s of one entity leave the latest snapshot's document. Every published document gets a
**generation** (`adb_statistics_generation`), which clients use to cache decoded documents. On open,
an unreadable document or a stray `.tmp` file is ignored: the entity counts as never analyzed.
Values use the engine's JSON form (`"Null"`, `{"Int64": 5}`, `{"String": "x"}`, ...).

## Modification counters and freshness

The commit apply step adds every row mutation (insert, update or delete) to its entity's
counter, under the commit lock. The counters are written by the **same journal commit** as the
projections (`stats/modifications.meta`), and after a crash they are brought forward by the same
log replay. They therefore always count exactly the committed mutations
(`statistics::modification_counters_track_commits`). `rebuild_projections` deletes the file and
recounts from the complete log. Unlike a statistics document, a damaged counters file is
reported as corruption, like every other checkpointed file.

Two edge cases make the counter a staleness signal rather than an exact delta:

* A commit is counted when it is applied, before it is durable. If the process crashes before
  durability, the commit is lost but an `ANALYZE` that ran in between already recorded it.
  The delta then under-counts by those few mutations.
* A database created before 2.2.3 starts counting at its first open by 2.2.3. A full
  `rebuild_projections` recounts from the beginning of the log.

A document records the counter at its snapshot (`modifications_at_analyze`), so

```text
modifications since ANALYZE = counter − modifications_at_analyze
```

The JVM's `EntityStatistics` reports statistics as **stale** when more than 20% of the analyzed
rows changed (`changeRatio > 0.2`). Confidence is 1.0 for an exact analysis, 0.9 for a sampled
one, and half of that when stale. EXPLAIN shows the freshness of every table a query reads and
warns about stale or missing statistics. `ANALYZE` stays manual in 2.2.3; the counters are what
an automatic trigger will use later.

## JVM

* `StatisticsCodec.decode` is strict: unknown format versions, missing or mistyped fields and
  impossible counts (more NULLs than rows, more distinct values than non-NULL rows) are rejected.
* `EngineStatisticsProvider` caches decoded documents by generation, so an `ANALYZE` by any
  client is picked up at the next query. Each planning step reads the generation and the
  modification count, which is two cheap native calls per entity and query. A document that
  cannot be decoded counts as missing, so a query never fails because of statistics.
* `StatisticsProvider.of(...)` serves fixed statistics for tests, benchmarks and what-if planning.

`AS OF VERSION` queries are estimated with the current statistics. Version-store statistics are
not collected.
