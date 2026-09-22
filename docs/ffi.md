# FFI ABI v2 — Milestone 2

The JVM uses JDK 22+ Foreign Function & Memory API. JNI is not required.

## Lifecycle
- `adb_open` / `adb_close`
- `adb_execute_plan_json` / `adb_execute_plan_json_at`
- `adb_query_next_batch` / `adb_query_cancel` / `adb_query_close`
- `adb_batch_data` / `adb_batch_len` / `adb_batch_release`

## Statement mutations
- `adb_insert_row_json`
- `adb_update_fields_json`
- `adb_delete_row`

Each mutation is an atomic Rust transaction. UPDATE reads the row inside the same MVCC transaction before applying field assignments, so write-write conflicts remain enforced by the Rust transaction engine.

## Ownership
All handles and batch memory allocated by Rust are released by Rust ABI calls. JVM never calls `free()` on Rust memory.

## ABI
`adb_abi_version()` returns `2`. Java validates this before opening a database.
