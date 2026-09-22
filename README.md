# Adaptive DB — Milestone 2.0

Milestone 2 wprowadza pierwszą warstwę JVM/Scala nad Rust data-plane 1.7.

## JVM

- Scala 3 semantic model
- persistent file catalog
- SQL tokenizer/parser
- binder + type checking
- LogicalPlan
- rule optimizer (w tym PK equality -> PointLookup)
- PhysicalPlanner
- versioned JSON plan wire adapter
- Java 22+ FFM / Project Panama adapter
- SQL gateway
- CLI

## Rust additions

- ABI v2
- execute-at-snapshot przez FFI
- latest committed timestamp
- atomic INSERT / UPDATE-fields / DELETE FFI helpers
- wielotabelowy physical RowId: `(EntityId << 64) | BIGINT_PK`

## Obsługiwany SQL 2.0

```sql
CREATE TABLE account (
  id BIGINT PRIMARY KEY,
  balance BIGINT NOT NULL,
  owner STRING
);

INSERT INTO account VALUES (1, 100, 'Alice');
SELECT id, balance FROM account WHERE balance > 50 LIMIT 10;
SELECT * FROM account AS OF VERSION 3 WHERE id = 1;
UPDATE account SET balance = 200 WHERE id = 1;
DELETE FROM account WHERE id = 1;
EXPLAIN SELECT * FROM account WHERE id = 1;
```

UPDATE/DELETE wymagają w Milestone 2 predykatu `WHERE primary_key = BIGINT`.

## Build Rust

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
cargo build --release -p adb-ffi
```

## Build JVM

Wymagany JDK 22+.

```bash
cd jvm
gradle clean test
```

## CLI

```bash
cd jvm
gradle :adb-cli:run \\
  -Dadb.native.library=../target/release/libadb_ffi.so \\
  -Dadb.data=../demo-data
```

## Granica architektoniczna

```text
SQL -> AST -> Binder -> LogicalPlan -> RuleOptimizer -> PhysicalPlan
                                                     |
                                                     v
                                              Java FFM/Panama
                                                     |
                                                     v
                                                  adb-ffi
                                                     |
                                                     v
                                             Rust execution/storage
```

`proto/physical_plan.proto` jest docelowym kontraktem transportowym. Milestone 2.0 zachowuje JSON wire v1 jako działający bootstrap; core planner nie zależy od JSON.

## Demo: 1.0.1 → 2.0.1

A runnable chronological feature tour is available under `demo/`. It demonstrates WAL/recovery, persistent current state and B+Tree lookup, historical `AS OF VERSION`, native RecordBatch execution, and finally the complete Scala SQL/control-plane path.

```bash
./demo/run-demo.sh
```

See `demo/README.md` and `demo/FEATURE-MAP.md`.


## Corrected demo build

- Scala: **3.3.8**
- JVM/Scala target: **JDK 22**
- Changelog: `CHANGELOG.md`
- Demo: `demo/run-demo.sh`

The physical-plan point-lookup wire encodes the 128-bit RowId as a decimal JSON string to avoid
precision/range loss for `(entityId << 64) | primaryKey`.

## Milestone 2.0.2 benchmark/demo package

`2.0.2` preserves the hardened `2.0.1-with-demo-buildfix` feature set and adds measurement tooling.
See `TASKS-2.0.2.md` and `examples/README.md`. No Milestone 2.1+ query features are included.
