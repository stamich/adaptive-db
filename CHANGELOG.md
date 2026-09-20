# Changelog

## 1.7.2
- Based on Milestone 1.7.1 Hardened.
- Added Rust demo for execution pipeline, ADB Batch v1 and C ABI.
- Added execution/FFI benchmark baseline and JSON result export.
- Added implementation task map and release validation script.
- Added missing `adb-wal` test-only `tempfile.workspace = true` dependency required by segmented-WAL hardening tests.
- No 2.0 functionality introduced.

## 1.7.1
- Execution/FFI hardening: bounded path/plan inputs, output-handle clearing, status mapping, cardinality checks, checked wire conversions, 64 MiB batch ceiling, memory-limit enforcement, panic-path removal and explicit C pointer/handle contract.

## 1.7
- Added PhysicalPlan execution, batched operators and initial C ABI.

### 1.7.2 build-fix — FFI point lookup JSON decoding

- fixed `adb_execute_plan_json` returning `InvalidArgument` for the point-lookup plan used by `crates/adb-ffi/tests/ffi.rs`,
- introduced an FFI-only `WirePhysicalPlan` decoder instead of deserializing transport JSON directly into engine-native `PhysicalPlan`,
- accepts numeric `row_id` values through `u64` and decimal-string `row_id` values through the full `u128` range,
- leaves `adb_core::RowId` serialization and all persisted Rust engine formats unchanged,
- added regression tests for numeric and full-range decimal-string point lookup IDs.

## 1.6.2

- Added Rust-only temporal demo under `examples/demo`.
- Added Rust-only temporal/storage/segmented-WAL benchmark under `examples/benchmark`.
- Added machine-readable JSON benchmark output.
- Added `TASKS-1.6.2.md` describing the 1.5.2 -> 1.6.x implementation order.
- Added release validation/build script and release metadata.
- Fixed the existing `adb-wal` integration-test dependency by declaring `tempfile` as a workspace
  dev-dependency.
- No production Rust source behavior was changed relative to 1.6.1 Hardened.

## 1.6.1

- Hardened Milestone 1.6 persistent Current + Version stores, temporal B+Tree, segmented WAL and
  checkpoint v2.
- Added bounded WAL payload handling, crash-tail normalization, segment-gap detection, checked LSN
  packing, directory fsync on segment metadata changes and logical recovery validation.
- Hardened page/B+Tree/checkpoint codecs and corruption handling.

## 1.6

- Added persistent temporal Version Store and VersionBTree.
- Added explicit WAL before-images for independent history recovery.
- Added segmented WAL and packed segment/offset LSN.
- Added checkpoint v2 with independent Current/Version replay frontiers.
- Added temporal history API, storage statistics, integrity verification and WAL retention.

## 1.5.2
- Based on Milestone 1.5.1 Hardened; no new database-engine functionality.
- Added Rust-only `examples/demo` covering MVCC, persistence, checkpoint/recovery, B+Tree persistence and page CRC validation.
- Added Rust-only `examples/benchmark` baseline for B+Tree, durable commits, persistent Current reads, historical reads and reopen/recovery.
- Added machine-readable benchmark JSON output and `examples/results` convention.
- Added detailed `TASKS-1.5.2.md` implementation sequence from the 1.0.2 contract to 1.5.x physical storage.

## 1.5.1
- Hardened the Milestone 1.5 persistent-storage architecture.
- Added page CRC/version validation and structural checks.
- Hardened B+Tree decoding and root metadata.
- Hardened checkpoint publication and page-file crash-tail behavior.
- Retained bounded WAL and WAL crash-tail handling inherited from 1.0.1.

## 1.5
- Added persistent fixed pages, BufferPool, persistent Current heap, primary B+Tree and checkpoint + WAL recovery.

## 1.0.2
- Added Rust-only demo and benchmark baseline over the 1.0.1 hardened transactional/WAL engine.
