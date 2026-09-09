# Adaptive DB — Milestone 1.5.2

Milestone 1.5.2 is the demo/benchmark packaging release over **Milestone 1.5.1 Hardened**.
It does not add later-milestone database features.

The inherited 1.5.1 engine contains:
- snapshot-isolated MVCC transaction API,
- logical WAL with durable commit,
- fixed 16 KiB persistent pages,
- BufferPool,
- slotted heap pages / persistent Current heap,
- primary B+Tree (`RowId -> RowLocation`),
- checkpoint + WAL recovery,
- checksummed/hardened page and metadata formats.

## Demo

```bash
cargo run --release -p adb-demo-1-5-2
```

## Benchmark

```bash
cargo run --release -p adb-benchmark-1-5-2 -- \
  --rows 10000 \
  --batch-size 100 \
  --lookups 100000 \
  --historical-versions 1000 \
  --output examples/results/1.5.2-local.json
```

## Full validation

```bash
./build-milestone1.5.2.sh
```

See `TASKS-1.5.2.md`, `HARDENING-1.5.1.md`, `SECURITY-REVIEW.md`, `docs/architecture.md` and `docs/invariants.md`.
