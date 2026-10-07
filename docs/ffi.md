# C ABI v3 — Milestone 2.0.3

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
| Queries | `adb_execute_plan_json`, `adb_execute_plan_json_at`, `adb_query_next_batch`, `adb_query_cancel`, `adb_query_close` |
| Buffers | `adb_batch_data`, `adb_batch_len`, `adb_batch_release` |
| Mutations | `adb_insert_row_json`, `adb_update_fields_json`, `adb_delete_row` (each one serializable transaction) |
| Metadata | `adb_latest_committed_ts`, `adb_abi_version`, `adb_engine_version` |
| Maintenance *(new)* | `adb_checkpoint`, `adb_vacuum` |
| Change feed *(new)* | `adb_read_changes_json`, `adb_change_feed_end`, `adb_commit_consumer_offset`, `adb_consumer_offset` — see [cdc.md](cdc.md) |

## Status codes

| Code | Name | Meaning |
|---|---|---|
| 0 | `OK` | |
| 1 | `END_OF_STREAM` | query exhausted |
| 2 | `INVALID_ARGUMENT` | bad pointer, length, cursor or JSON |
| 3 | `CONFLICT` | transaction conflict (retry) or duplicate primary key |
| 4 | `IO_ERROR` | filesystem failure |
| 5 | `CORRUPTION` | persisted bytes failed validation; projections can be rebuilt from the log |
| 6 | `CANCELLED` | query cancelled |
| 7 | `NOT_FOUND` | row or consumer offset not found |
| 8 | `POISONED` *(new)* | instance unusable or commit outcome unknown: close and reopen |
| 9 | `LOG_TRUNCATED` *(new)* | change-log position no longer retained |
| 255 | `INTERNAL` | unexpected failure or contained panic |

## Versioning

`adb_abi_version()` returns `3`. `io.adb.ffm.NativeLibrary` refuses any other value. Changes from
ABI 2: new functions and status codes above, `adb_close` checkpoints, and the plan wire format
accepts `entity_scan`.

## Verification

`examples/jvm-cdc-smoke/run.sh` builds the library, compiles the Java binding with plain `javac`
and exercises the full Java → C ABI → engine path.
