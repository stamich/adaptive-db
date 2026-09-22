# Adaptive DB Milestone 2.0.1 — Hardening

Milestone 2.0.1 is a compatibility-oriented hardening patch over historical Milestone 2.0.
It deliberately preserves the Milestone 2.0 feature scope: SQL subset, Scala control plane,
logical/physical planning, JVM FFM adapter, Rust execution engine, MVCC, persistent Current/Version
stores, segmented WAL and ABI v2.

## Inherited from Milestone 1.7.1 hardened

- bounded segmented-WAL frames,
- crash-tail truncation before append,
- missing/interior segment validation,
- page structural validation and CRC32 hardened pages,
- partial trailing-page normalization,
- panic-free current and temporal B+Tree node decoding,
- B+Tree depth/cycle defenses,
- crash-safe framed root/checkpoint metadata,
- bounded ADB Batch v1 encoding,
- bounded physical-plan JSON at the C ABI,
- deterministic FFI output-handle initialization,
- query/batch memory limits introduced in the 1.7.1 execution layer.

## New Milestone 2.0-specific hardening

### SQL/JVM control plane

- SQL input <= 1 MiB,
- <= 100,000 tokens,
- identifiers <= 256 chars,
- string literals <= 64 Ki chars,
- parenthesis nesting <= 256,
- EXPLAIN nesting <= 64 and implemented iteratively rather than recursively,
- LIMIT checked before Long -> Int narrowing,
- gateway materialized result <= 1,000,000 rows.

### Catalog

- catalog file <= 8 MiB,
- entity count <= 10,000,
- fields/entity <= 4,096,
- bounded entity/field names,
- validated entity/field id uniqueness,
- validated primary key references and version/next-id frontiers,
- canonical SHA-256 integrity digest for hardened catalog snapshots,
- legacy Milestone 2.0 catalog files without digest remain readable,
- file force -> atomic rename -> parent-directory force publication,
- in-memory state is reloaded from the last durable state if publication fails.

### FFI/JVM native adapter

- INSERT/UPDATE JSON <= 8 MiB before Rust slice construction,
- output commit timestamps/handles initialized before native work,
- Java path/plan/mutation lengths mirrored before FFI calls,
- Java native wrappers reject calls after close,
- native batch length capped at 64 MiB before `reinterpret`,
- BatchDecoder validates row/column counts, header/body sizes, bitmap length, fixed widths,
  variable offsets and trailing bytes,
- native last-error length capped before Java memory reinterpretation.

### Build reproducibility fixes

- Java toolchain remains JDK 22 for finalized FFM,
- Scala upgraded from 3.3.3 to 3.3.8,
- Scala 3.3.8 and Java both target JDK 22 bytecode,
- JUnit Platform launcher 1.11.0 is explicit for Gradle 9.x test workers.

## Compatibility

- milestone label: 2.0.1,
- Cargo/JVM version: `0.2.0+hardening.1`,
- native ABI remains 2,
- no 2.1 joins/aggregates or later statistics/index/schema-evolution functionality is imported.
