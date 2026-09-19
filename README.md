# Adaptive DB — Milestone 1.6.2 Demo + Benchmark

Milestone 1.6.2 is the demonstration/performance-baseline packaging release built on top of
Milestone 1.6.1 Hardened.

The Milestone 1.6 engine scope is unchanged: persistent Current Store, persistent temporal Version
Store, temporal reads/history, segmented WAL, checkpoint v2, recovery, storage statistics and
integrity verification. 1.6.2 adds Rust-only examples and benchmarks.

One build-metadata correction is included: `crates/adb-wal/Cargo.toml` declares
`tempfile.workspace = true` under `[dev-dependencies]`, because the existing segmented-WAL
integration test uses `tempfile`.

## Demo

```bash
cargo run --release -p adb-demo-1-6-2
```

The demo covers persistent temporal versions, temporal delete, restart durability, RYOW,
write/write conflicts, `storage_stats()`, `verify()`, direct PersistentVersionStore and
VersionBTree use, segmented-WAL rotation and WAL retention.

## Benchmark

```bash
cargo run --release -p adb-benchmark-1-6-2 -- \
  --rows 10000 \
  --batch-size 100 \
  --lookups 100000 \
  --versions-per-row 4 \
  --temporal-rows 1000 \
  --output examples/results/1.6.2-local.json
```

The benchmark is a milestone-to-milestone baseline. It intentionally does not invent reference
numbers; results must be captured on the target machine.

## Full validation

```bash
./build-milestone1.6.2.sh
```

See `TASKS-1.6.2.md` for the implementation sequence from 1.5.2 to 1.6.x.
