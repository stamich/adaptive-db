/*
 * Adaptive DB native C ABI, version 7 (Milestone 2.4.0).
 *
 * ABI 7 changes (vs. 6): entity schemas — adb_create_entity_json, adb_alter_entity_json,
 * adb_drop_entity, adb_list_schemas_json, adb_schema_history_json (see docs/schema.md); writes
 * to an entity with a schema are validated against it; plans are plan wire v4 documents
 * ({"wire_version":4,"schema_epoch":N,...}) and a plan of another catalog epoch is refused with
 * ADB_SCHEMA_CHANGED; adb_current_schema_epoch; statistics documents are format 3 (they record
 * the schema version; formats 1 and 2 still read); data format 3 (format 2 upgraded on open).
 *
 * ABI 6 changes (vs. 5): secondary indexes — adb_create_index_json, adb_drop_index,
 * adb_list_indexes_json (see docs/indexes.md); plans are plan wire v3 documents
 * ({"wire_version":3,...}, adding index_scan and row_fetch); statistics documents are format 2
 * (index statistics; format 1 still read); primary keys passed to the row functions are stored
 * order-preserving (data format 2) and the change feed reports them as signed decimal strings;
 * a directory in an older data format is refused with ADB_INCOMPATIBLE_FORMAT (migrate it with
 * adb-migrate).
 *
 * ABI 5 changes (vs. 4): optimizer statistics — adb_analyze_entity_json, adb_statistics_json,
 * adb_statistics_generation, adb_modifications_since_analyze (see docs/statistics.md); every
 * operator of a query profile carries a pre-order "node_id". Plan wire v2 and batch format v2
 * are unchanged.
 *
 * ABI 4 changes (vs. 3): plans are plan wire v2 documents ({"wire_version":2,"plan":...},
 * see docs/plan-wire-format.md); batches are batch format v2 (optional row ids, column ids are
 * output slots, see docs/batch-format.md); adb_query_profile_json; ADB_RESOURCE_LIMIT and
 * ADB_ARITHMETIC_OVERFLOW statuses.
 *
 * Conventions: every function validates its pointers and lengths, initializes its output slots
 * before doing any work, never unwinds a panic across the boundary, and on failure stores a
 * UTF-8 message readable with adb_last_error_ptr/len on the calling thread.
 * All memory allocated by the library is released by the library (handles, batches, buffers).
 */
#ifndef ADB_H
#define ADB_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AdbDatabaseHandle AdbDatabaseHandle;
typedef struct AdbQueryHandle AdbQueryHandle;
typedef struct AdbBatchHandle AdbBatchHandle;

typedef enum AdbStatus {
    ADB_OK = 0,
    ADB_END_OF_STREAM = 1,
    ADB_INVALID_ARGUMENT = 2,
    ADB_CONFLICT = 3,
    ADB_IO_ERROR = 4,
    ADB_CORRUPTION = 5,
    ADB_CANCELLED = 6,
    ADB_NOT_FOUND = 7,
    ADB_POISONED = 8,       /* instance unusable or commit outcome unknown: close and reopen */
    ADB_LOG_TRUNCATED = 9,  /* change-log position no longer retained */
    ADB_RESOURCE_LIMIT = 10,      /* query exceeded memory, row, fanout or comparison limits */
    ADB_ARITHMETIC_OVERFLOW = 11, /* exact computation (e.g. INT64 SUM) overflowed */
    ADB_INCOMPATIBLE_FORMAT = 12, /* data directory in another data format: run adb-migrate (ABI 6) */
    ADB_SCHEMA_CHANGED = 13, /* plan made at another catalog epoch: plan it again (ABI 7) */
    ADB_INTERNAL = 255
} AdbStatus;

#define ADB_ABI_VERSION 7

uint32_t adb_abi_version(void);
const uint8_t* adb_engine_version(size_t* out_len);
const uint8_t* adb_last_error_ptr(void);
size_t adb_last_error_len(void);

AdbStatus adb_open(const uint8_t* path_ptr, size_t path_len, AdbDatabaseHandle** out_db);
/* Checkpoints (best effort) and always releases the handle. */
AdbStatus adb_close(AdbDatabaseHandle* db);
AdbStatus adb_latest_committed_ts(AdbDatabaseHandle* db, uint64_t* out_commit_ts);
/* Current catalog epoch. Plans are plan wire v4 documents carrying the epoch they were made at
 * ({"wire_version":4,"schema_epoch":N,"plan":{...}}); a plan of another epoch is refused before
 * it runs (a table or index it was planned against may have changed: plan it again). */
AdbStatus adb_current_schema_epoch(AdbDatabaseHandle* db, uint64_t* out_epoch);

AdbStatus adb_execute_plan_json(
    AdbDatabaseHandle* db,
    const uint8_t* plan_ptr,
    size_t plan_len,
    AdbQueryHandle** out_query);

AdbStatus adb_execute_plan_json_at(
    AdbDatabaseHandle* db,
    const uint8_t* plan_ptr,
    size_t plan_len,
    uint64_t snapshot_ts,
    AdbQueryHandle** out_query);

/* plan_ptr/plan_len: a plan wire v2 JSON document of at most 8 MiB, validated before execution. */

AdbStatus adb_query_next_batch(AdbQueryHandle* query, AdbBatchHandle** out_batch);

/*
 * Per-operator runtime profile of the query (rows, batches, time, counters, peak memory) as a
 * UTF-8 JSON document returned as an AdbBatchHandle. Complete once next_batch returned
 * ADB_END_OF_STREAM. (ABI 4)
 */
AdbStatus adb_query_profile_json(AdbQueryHandle* query, AdbBatchHandle** out_json);
AdbStatus adb_query_cancel(AdbQueryHandle* query);
AdbStatus adb_query_close(AdbQueryHandle* query);
const uint8_t* adb_batch_data(const AdbBatchHandle* batch);
size_t adb_batch_len(const AdbBatchHandle* batch);
AdbStatus adb_batch_release(AdbBatchHandle* batch);

AdbStatus adb_insert_row_json(
    AdbDatabaseHandle* db,
    uint64_t entity_id,
    uint64_t primary_key,
    const uint8_t* row_ptr,
    size_t row_len,
    uint64_t* out_commit_ts);

AdbStatus adb_update_fields_json(
    AdbDatabaseHandle* db,
    uint64_t entity_id,
    uint64_t primary_key,
    const uint8_t* assignments_ptr,
    size_t assignments_len,
    uint64_t* out_commit_ts);

AdbStatus adb_delete_row(
    AdbDatabaseHandle* db,
    uint64_t entity_id,
    uint64_t primary_key,
    uint64_t* out_commit_ts);

/* ---- maintenance (ABI 3) ---- */

/* Persists in-memory projection changes; also runs automatically and on adb_close. */
AdbStatus adb_checkpoint(AdbDatabaseHandle* db);

/* Removes delete tombstones no live transaction can still conflict with. */
AdbStatus adb_vacuum(AdbDatabaseHandle* db, uint64_t* out_removed);

/* ---- change data capture (ABI 3) ---- */

/*
 * Reads up to max_events (1..=10000) committed transactions after `cursor` (0 = beginning of
 * the log). entities_len == 0 means all entities. The result is a UTF-8 JSON document returned
 * as an AdbBatchHandle (read with adb_batch_data/len, free with adb_batch_release):
 *   {"next_cursor":"<u64>","events":[{"tx_id":N,"commit_ts":N,"commit_lsn":"<u64>",
 *     "cursor_after":"<u64>","changes":[{"entity_id":N,"primary_key":"<i64>",
 *     "kind":"insert|update|delete","before":Row|null,"after":Row|null}]}]}
 * Only the durable prefix of the log is visible.
 */
AdbStatus adb_read_changes_json(
    AdbDatabaseHandle* db,
    uint64_t cursor,
    uint32_t max_events,
    const uint64_t* entities_ptr,
    size_t entities_len,
    AdbBatchHandle** out_json);

/* Cursor at the durable end of the log. */
AdbStatus adb_change_feed_end(AdbDatabaseHandle* db, uint64_t* out_cursor);

/* Durably stores the cursor of a named consumer (name: 1..=256 UTF-8 bytes). */
AdbStatus adb_commit_consumer_offset(
    AdbDatabaseHandle* db,
    const uint8_t* name_ptr,
    size_t name_len,
    uint64_t cursor);

/* Reads a consumer's stored cursor; ADB_NOT_FOUND if it has none. */
AdbStatus adb_consumer_offset(
    AdbDatabaseHandle* db,
    const uint8_t* name_ptr,
    size_t name_len,
    uint64_t* out_cursor);

/* ---- optimizer statistics (ABI 5) ---- */

/*
 * Runs ANALYZE on one entity: scans the latest snapshot without blocking commits, persists the
 * statistics document and returns it as a UTF-8 JSON buffer (an AdbBatchHandle; read with
 * adb_batch_data/len, free with adb_batch_release). options_len == 0 uses the default options
 * (options_ptr may then be NULL); otherwise options is a JSON object with any of
 *   fields (array of field ids), sample_rows, histogram_buckets, most_common_values, max_rows,
 *   max_working_set_bytes, deadline_ms   (at most 64 KiB; unknown keys are rejected).
 * Errors: ADB_INVALID_ARGUMENT for bad options, ADB_RESOURCE_LIMIT when a row, memory or time
 * limit is exceeded (nothing is persisted then).
 */
AdbStatus adb_analyze_entity_json(
    AdbDatabaseHandle* db,
    uint64_t entity_id,
    const uint8_t* options_ptr,
    size_t options_len,
    AdbBatchHandle** out_json);

/* The persisted statistics document of one entity; ADB_NOT_FOUND if it was never analyzed. */
AdbStatus adb_statistics_json(AdbDatabaseHandle* db, uint64_t entity_id, AdbBatchHandle** out_json);

/*
 * Generation of an entity's statistics document (0 if there is none); it changes whenever
 * ANALYZE publishes a new document, so clients can cache decoded documents by it.
 */
AdbStatus adb_statistics_generation(AdbDatabaseHandle* db, uint64_t entity_id, uint64_t* out_generation);

/*
 * Row mutations committed to an entity since its last ANALYZE (since creation if never; for a
 * database created before 2.2.3, since its first open by 2.2.3).
 */
AdbStatus adb_modifications_since_analyze(
    AdbDatabaseHandle* db,
    uint64_t entity_id,
    uint64_t* out_count);

/* ---- secondary indexes (ABI 6) ---- */

/*
 * Creates and builds an index. spec is a UTF-8 JSON object (at most 64 KiB):
 *   {"name":"idx_balance","entity_id":3,"columns":[{"field_id":2,"direction":"asc"}],"include":[3]}
 * (name [A-Za-z_][A-Za-z0-9_]* up to 128 bytes, unique case-insensitively; 1..16 key columns,
 * direction "asc" (default) or "desc"; up to 32 covered "include" fields; no field twice).
 * The build blocks commits, then a checkpoint publishes the index. Returns the index status as a
 * JSON buffer (an AdbBatchHandle):
 *   {"id":N,"name":"...","entity_id":N,"columns":[...],"include":[...],"state":"active",
 *    "created_at_ts":N,"usable_from_ts":N,"watermark_ts":N}
 * The index serves snapshots usable_from_ts..=watermark_ts.
 * Errors: ADB_INVALID_ARGUMENT for a malformed or invalid spec, a taken name, too many indexes,
 * or a row whose encoded key (512 bytes) or covered values (1 KiB) exceed the index limits.
 */
AdbStatus adb_create_index_json(
    AdbDatabaseHandle* db,
    const uint8_t* spec_ptr,
    size_t spec_len,
    AdbBatchHandle** out_json);

/* Drops the index named by a UTF-8 string (at most 128 bytes); ADB_NOT_FOUND if there is none. */
AdbStatus adb_drop_index(AdbDatabaseHandle* db, const uint8_t* name_ptr, size_t name_len);

/* Every index as a JSON buffer {"indexes":[status, ...]} in id order (status as above). */
AdbStatus adb_list_indexes_json(AdbDatabaseHandle* db, AdbBatchHandle** out_json);

/* ---- entity schemas (ABI 7) ---- */

/*
 * Creates an entity (CREATE TABLE). spec is a UTF-8 JSON object (at most 1 MiB):
 *   {"name":"account","entity_id":3,"primary_key":"id","fields":[
 *     {"name":"id","value_type":"Int64","nullable":false},
 *     {"name":"balance","value_type":"Int64","nullable":false,"default":{"Int64":0}}]}
 * entity_id is optional (the engine assigns one). Returns version 1 of the schema as a JSON
 * buffer: {"entity_id","name","version","parent","epoch","primary_key","next_field_id",
 * "fields":[{"id","name","value_type","nullable","default","introduced_in","dropped_in",
 * "references"}, ...]}. Errors: ADB_INVALID_ARGUMENT (malformed or invalid definition, taken
 * name or id), ADB_RESOURCE_LIMIT (schema limits).
 */
AdbStatus adb_create_entity_json(
    AdbDatabaseHandle* db,
    const uint8_t* spec_ptr,
    size_t spec_len,
    AdbBatchHandle** out_json);

/*
 * Applies one change to an entity: {"entity_id":3,"change":C} where C is
 *   {"op":"add_field","field":{"name":..,"value_type":..,"nullable":..,"default":..}}
 *   {"op":"rename_field","from":..,"to":..}
 *   {"op":"drop_field","name":..}
 * Returns the new schema version (same form as above). Existing rows are not rewritten: they
 * are adapted on read and migrated when next written. Errors: ADB_NOT_FOUND (no such live
 * entity), ADB_INVALID_ARGUMENT (malformed or refused change), ADB_RESOURCE_LIMIT.
 */
AdbStatus adb_alter_entity_json(
    AdbDatabaseHandle* db,
    const uint8_t* request_ptr,
    size_t request_len,
    AdbBatchHandle** out_json);

/* Drops an entity (DROP TABLE). ADB_NOT_FOUND if it is not live; ADB_INVALID_ARGUMENT while
 * indexes of the entity exist. Its history stays readable. */
AdbStatus adb_drop_entity(AdbDatabaseHandle* db, uint64_t entity_id);

/* The catalog as a JSON buffer {"schema_epoch":N,"entities":[schema, ...]}: the latest schema
 * of every live entity in id order, consistent with the epoch. */
AdbStatus adb_list_schemas_json(AdbDatabaseHandle* db, AdbBatchHandle** out_json);

/* Every schema version of an entity (also a dropped one), oldest first, as a JSON buffer
 * {"entity_id":N,"versions":[schema, ...]}. ADB_NOT_FOUND if it never had a schema. */
AdbStatus adb_schema_history_json(
    AdbDatabaseHandle* db,
    uint64_t entity_id,
    AdbBatchHandle** out_json);

#ifdef __cplusplus
}
#endif

#endif
