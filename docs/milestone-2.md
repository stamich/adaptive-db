# Milestone 2 architecture

## Responsibility split

### Scala 3
- semantic schema and catalog model
- SQL syntax
- binding and type checking
- logical planning
- deterministic optimization
- physical access path selection

### Java 22+
- Foreign Function & Memory API
- lifetime-safe wrappers around Rust-owned handles
- ADB Batch Format v1 decoding

### Rust
- transactions and MVCC
- WAL/recovery
- storage
- physical execution
- statement mutation atomics

## Reserved physical field

`FieldId(0)` is `_entity_id` and never appears in user schema. General table scans are physically:

```text
Filter(Column(0) == EntityId)
  Scan
```

Primary-key point lookup uses:

```text
u128 RowId = (EntityId << 64) | unsigned(BIGINT primary key)
```

This lets Milestone 2 support multiple SQL tables without changing the Rust page/B+Tree layout.
