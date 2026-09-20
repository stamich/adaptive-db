# ADB Batch Format v1

Little-endian.

```text
u32 magic = ADBB
u16 version = 1
u16 reserved
u32 row_count
u32 column_count

row_ids[row_count] : u128

repeat column_count:
  u32 field_id
  u8  physical_type
  u8[3] reserved
  u32 null_bitmap_len
  u32 payload_len
  bytes null_bitmap
  bytes payload
```

Physical types:

```text
1 BOOL
2 INT64
3 FLOAT64
4 UTF8 STRING
5 BYTES
```

String/Bytes payload:

```text
u32 offsets[row_count + 1]
bytes concatenated_data
```

Null bitmap: bit=1 oznacza NULL.
