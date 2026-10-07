# Milestone 2 architecture

## Responsibility split

### Scala 3
- semantic schema and catalog model
- SQL syntax
- binding and type checking
- logical planning
- deterministic optimization
- physical access path and strategy selection (with explained decisions)

### Java 22+
- Foreign Function & Memory API
- lifetime-safe wrappers around Rust-owned handles
- ADB batch format v2 decoding (slot columns, optional row ids)

### Rust
- transactions and MVCC
- WAL/recovery
- storage
- physical execution (joins, aggregation, sorting) under memory and work budgets
- statement mutation atomics

## Reserved physical field

`FieldId(0)` is `_entity_id` and never appears in user schema. Since Milestone 2.0.3 a table scan
is physically a native key-range scan of the entity:

```text
EntityScan(EntityId)        -- reads [EntityId << 64, (EntityId + 1) << 64)
```

(Milestones 2.0–2.0.2 used `Filter(Column(0) == EntityId)` over a scan of every table.)

Primary-key point lookup uses:

```text
u128 RowId = (EntityId << 64) | unsigned(BIGINT primary key)
```

This lets Milestone 2 support multiple SQL tables without changing the Rust page/B+Tree layout.
