# Native C ABI / Java FFM boundary

ABI version: `1`.

Główne funkcje:

```text
adb_abi_version
adb_engine_version
adb_open
adb_close
adb_execute_plan_json
adb_query_next_batch
adb_query_cancel
adb_query_close
adb_batch_data
adb_batch_len
adb_batch_release
adb_last_error_ptr
adb_last_error_len
```

Ownership:

- Rust alokuje database/query/batch handles.
- Caller zwalnia je wyłącznie odpowiednimi `adb_*_close/release`.
- Pointer batch data jest ważny do `adb_batch_release`.
- Panic nie może przekroczyć C ABI; eksportowane operacje mutujące/otwierające są chronione przez `catch_unwind`.

Milestone 2.0 powinien dodać cienki adapter Java 22+ oparty o `java.lang.foreign.Linker`, `MemorySegment` i `SymbolLookup`.
