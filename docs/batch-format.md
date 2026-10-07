# ADB batch format v2

Query results cross the C ABI as columnar batches (`adb_query_next_batch`). All integers are
little-endian.

```text
u32 magic   = 0x41444242 ("ADBB")
u16 version = 2
u16 flags              bit 0 (FLAG_ROW_IDS): row ids follow the header; other bits must be 0
u32 row_count
u32 column_count

row_ids[row_count] : u128      only when FLAG_ROW_IDS is set

repeat column_count:
  u32 column_id                the output slot of the column
  u8  physical_type
  u8[3] reserved
  u32 null_bitmap_len          = ceil(row_count / 8)
  u32 payload_len
  bytes null_bitmap            bit = 1 means NULL
  bytes payload
```

Physical types:

| Tag | Type | Payload |
|---|---|---|
| 1 | BOOL | one byte per row |
| 2 | INT64 | 8 bytes per row |
| 3 | FLOAT64 | 8 bytes per row (IEEE 754) |
| 4 | UTF-8 STRING | `u32 offsets[row_count + 1]` + concatenated bytes |
| 5 | BYTES | as STRING |

Rules:

* Columns appear in the plan's output order. A column that is NULL in every row of a batch is
  omitted; readers treat a missing column as NULL (`NativeRecordBatch.column(slot)` is empty).
* Row ids are present when every row came straight from storage (scans, lookups and operators
  above them that keep rows, such as `Filter`, `Sort`, `TopK`). Join and aggregate rows have no
  storage identity, so their batches carry none.
* An encoded batch is at most 64 MiB, at most 1,000,000 rows and 4096 columns (the Java decoder
  enforces the same bounds and rejects trailing bytes, unknown flags and other versions).

## History

* v1 (until 2.0.3): the flags word was reserved, row ids were always present, and `column_id` was
  a storage field id.
* v2 (2.1): flags word, optional row ids, `column_id` is an output slot.
