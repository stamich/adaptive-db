# C ABI v9 (Milestone 2.5.2.1)

The JVM uses the JDK 22+ Foreign Function & Memory API; JNI is not required. The authoritative
declarations are in `include/adb.h`.

## Conventions

* Every function validates pointers and lengths, initializes its output slots first, contains
  panics, and reports failures as an `AdbStatus` plus a thread-local message
  (`adb_last_error_ptr` / `adb_last_error_len`).
* Memory allocated by the library is released by the library: database and query handles, record
  batches and JSON buffers (`adb_batch_release`).

## Functions

| Area | Functions |
|---|---|
| Lifecycle | `adb_open`, `adb_close` (checkpoints, always releases the handle) |
| Queries | `adb_execute_plan_json`, `adb_execute_plan_json_at` (plan wire v4, see [plan-wire-format.md](plan-wire-format.md)), `adb_query_next_batch` (batch format v2, see [batch-format.md](batch-format.md)), `adb_query_cancel`, `adb_query_close` |
| Query counters *(new in 9)* | `adb_query_telemetry`: writes an `AdbQueryTelemetry` (rows returned, rows read from storage, batches, peak memory: four `uint64_t`, 32 bytes) into caller-owned memory, without building or serializing a profile. Complete once `adb_query_next_batch` returned `END_OF_STREAM`; `out` is zeroed first and null arguments give `INVALID_ARGUMENT`. The JVM reads it from a reused native segment (`NativeQueryTelemetry`), which is how query telemetry avoids the JSON profile; see [telemetry.md](telemetry.md) |
| Profiles *(4)* | `adb_query_profile_json`: the query's per-operator runtime profile, each operator with its pre-order `node_id` *(5)* (see [execution.md](execution.md#runtime-profile)) |
| Commit order *(new in 8)* | `adb_latest_commit_seq` (newest published commit; 0 before the first), `adb_insert_row_commit_json`, `adb_update_fields_commit_json`, `adb_delete_row_commit` (the mutations, also reporting the commit's timestamp and sequence number; the old functions remain and delegate to them); see [change-log.md](change-log.md) |
| Change-log cursors *(8)* | `adb_change_cursor_open` (after a commit number; `LOG_TRUNCATED` if that history was pruned), `adb_change_cursor_next_batch` (a JSON batch of commits in an `AdbBatchHandle`, `END_OF_STREAM` when caught up; `max_commits` 1..=10000, `max_bytes` 1..=64 MiB), `adb_change_cursor_position`, `adb_change_cursor_close` |
| Projections *(8)* | `adb_projection_register_json` (an external consumer; returns its id), `adb_projection_list_json`, `adb_projection_get_json` (state, frontier, head, lag, rate), `adb_projection_advance`, `adb_projection_set_state` (0 building, 1 ready, 2 needs rebuild, 3 failed), `adb_projection_wait_until` (at most 60 s; outcome 0 reached, 1 timed out, 2 not readable, 3 unknown); see [consistency.md](consistency.md) |
| Schemas *(7)* | `adb_create_entity_json` (`CREATE TABLE`, returns version 1), `adb_alter_entity_json` (one `add_field`, `rename_field` or `drop_field` change, returns the new version), `adb_drop_entity`, `adb_list_schemas_json` (catalog epoch and every live schema), `adb_schema_history_json` (every version, also of a dropped entity); see [schema.md](schema.md) |
| Indexes *(6)* | `adb_create_index_json` (builds the index over the existing rows, publishes it with a checkpoint and returns its status), `adb_drop_index` (`NOT_FOUND` if there is none), `adb_list_indexes_json` (every status, in id order; see [indexes.md](indexes.md)) |
| Statistics *(5)* | `adb_analyze_entity_json` (ANALYZE one entity, returns the document), `adb_statistics_json` (`NOT_FOUND` if never analyzed), `adb_statistics_generation` (changes with every published document; 0 = none), `adb_modifications_since_analyze` (see [statistics.md](statistics.md)) |
| Buffers | `adb_batch_data`, `adb_batch_len`, `adb_batch_release` |
| Mutations | `adb_insert_row_json`, `adb_update_fields_json`, `adb_delete_row` (each one serializable transaction) |
| Metadata | `adb_latest_committed_ts`, `adb_latest_commit_seq` *(8)*, `adb_current_schema_epoch` *(7)*, `adb_abi_version`, `adb_engine_version` |
| Maintenance | `adb_checkpoint`, `adb_vacuum` |
| Change feed | `adb_read_changes_json`, `adb_change_feed_end`, `adb_commit_consumer_offset`, `adb_consumer_offset` (see [cdc.md](cdc.md)) |

## Status codes

| Code | Name | Meaning |
|---|---|---|
| 0 | `OK` | |
| 1 | `END_OF_STREAM` | query exhausted |
| 2 | `INVALID_ARGUMENT` | bad pointer, length, cursor or JSON; unsupported plan wire version; invalid plan; expression type error; invalid `ANALYZE` options; invalid index spec, taken index name, too many indexes, a row an index cannot hold; invalid or refused schema definition or change, a row that does not fit its entity's schema *(7)* |
| 3 | `CONFLICT` | transaction conflict (retry) or duplicate primary key |
| 4 | `IO_ERROR` | filesystem failure |
| 5 | `CORRUPTION` | persisted bytes failed validation; projections can be rebuilt from the log |
| 6 | `CANCELLED` | query cancelled or past its deadline |
| 7 | `NOT_FOUND` | row, consumer offset, statistics document, index or entity not found |
| 8 | `POISONED` | instance unusable or commit outcome unknown: close and reopen |
| 9 | `LOG_TRUNCATED` | change-log position no longer retained |
| 10 | `RESOURCE_LIMIT` | query exceeded its memory budget, materialized-row cap, join fanout or nested-loop comparison limit, or `ANALYZE` exceeded its row, working-set or time limit, or a schema limit was reached *(7)*; the database is unaffected |
| 11 | `ARITHMETIC_OVERFLOW` | an exact computation overflowed (e.g. an INT64 `SUM`) |
| 12 | `INCOMPATIBLE_FORMAT` *(6)* | `adb_open` of a directory in an older data format (before 2.3.0); migrate it with `adb-migrate` ([indexes.md](indexes.md#data-format-2)) |
| 13 | `SCHEMA_CHANGED` *(7)* | the plan was made at another catalog epoch: refresh the catalog and plan again ([plan-wire-format.md](plan-wire-format.md)) |
| 255 | `INTERNAL` | unexpected failure or contained panic |

## Versioning

`adb_abi_version()` returns `8` and `adb_engine_version()` returns the release version.
`io.adb.ffm.NativeLibrary` refuses any other ABI version.

Changes from ABI 7: the commit sequence number (`latest_commit_seq`, and the `*_commit` mutation
functions, `insertCommit`/`updateCommit`/`deleteCommit` returning a `NativeCommitResult` in Java),
the change-log cursor functions (`openChangeCursor` → `NativeChangeCursor`) and the projection
functions (`registerProjection`, `projectionsJson`, `projectionJson`, `advanceProjection`,
`setProjectionState`, `waitForProjection` → `NativeWaitOutcome`). The change feed's JSON events
gained `commit_seq`. Data format 4 (format 3 is upgraded on open). Plan wire v4, batch format v2
and the statistics functions are unchanged. `waitForProjection` runs outside the object monitor so
a waiting reader never blocks the writes that make its projection progress.

Changes from ABI 6: the five schema functions (`NativeDatabase.createEntityJson`,
`alterEntityJson`, `dropEntity`, `listSchemasJson`, `schemaHistoryJson` in Java),
`adb_current_schema_epoch` (`currentSchemaEpoch`), status 13 `SCHEMA_CHANGED`, plan wire v4 (the
envelope carries `schema_epoch`; v3 documents are refused) and statistics format 3. Rows written
through the mutation functions to an entity with a schema are validated and completed with
defaults; entities without a schema behave as before. Batch format v2 is unchanged.

Changes from ABI 5: the three index functions (`NativeDatabase.createIndexJson`, `dropIndex`,
`listIndexesJson` in Java), status 12 `INCOMPATIBLE_FORMAT`, and plan wire v3 (`index_scan`,
`row_fetch`; v2 documents are refused). Row ids of BIGINT primary keys use data format 2
(`RowId::from_primary_key`: the key with its sign bit flipped), so a client that composes row ids
itself must flip the sign bit. The mutation functions take the primary key and do this for you.
Batch format v2 and the statistics functions are unchanged; statistics documents may now be
format 2.

Changes from ABI 4: the four statistics functions (`NativeDatabase.analyzeJson`,
`statisticsJson`, `statisticsGeneration`, `modificationsSinceAnalyze` in Java) and the `node_id` of every profile
operator. Plan wire v2 and batch format v2 are unchanged: node ids are positions in a pre-order
walk of the plan, so the plan needs no id field.

Changes from ABI 3 to 4: plan wire v2 only,
batch format v2, `adb_query_profile_json`, statuses 10 and 11, consistent mapping of execution
errors (invalid plans and expression errors are `INVALID_ARGUMENT`, deadlines `CANCELLED`).

## Verification

`examples/jvm-cdc-smoke/run.sh` builds the library, compiles the Java binding with plain `javac`
and exercises the Java → C ABI → engine path; `crates/adb-ffi/tests/relational.rs` covers joins,
aggregates, profiles and the new statuses over the ABI; `crates/adb-ffi/tests/statistics.rs` covers
ANALYZE, documents, staleness and their statuses; `crates/adb-ffi/tests/index.rs` covers creating,
listing, scanning and dropping indexes, invalid specs and older data formats;
`crates/adb-ffi/tests/{commit_seq,change_cursor,projection}.rs` cover the 2.5.0 functions, their
limits and error codes; `examples/jvm-consistency-smoke/run.sh` drives sessions, tokens and
`SET SESSION CONSISTENCY` through the real library.

## ABI 9 (Milestone 2.5.2.1)

ABI 9 is **additive**: it adds `adb_query_telemetry` and the `AdbQueryTelemetry` struct and changes
nothing else (no function, struct, status code, plan wire v4 or data format 4). The version number
still changes because bindings compare it exactly (`NativeLibrary.EXPECTED_ABI`), which is what
lets a binding refuse a library that lacks the function instead of failing at the first call.
