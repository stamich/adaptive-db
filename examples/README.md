# Adaptive DB Milestone 1.0.2 — Rust demo and benchmark

Milestone 1.0.2 keeps the Milestone 1.0.1 hardened engine unchanged and adds
Rust-only executable examples. No JVM layer, SQL layer, schema/catalog layer,
indexes, or later-milestone storage features are introduced.

## Layout

- `demo/` — deterministic functional demonstration of the engine.
- `benchmark/` — zero-extra-dependency baseline benchmark harness.
- `results/` — intentionally empty placeholder for locally generated JSON results.

## Demo

```bash
cargo run --release -p adb-demo
```

The demo exercises:

1. transaction-local read after `put`, before commit,
2. durable commit,
3. MVCC snapshot isolation,
4. historical `get_at`,
5. write-write conflict detection,
6. delete,
7. close/reopen WAL recovery.

## Benchmark

```bash
cargo run --release -p adb-benchmark -- \
  --rows 10000 \
  --batch-size 100 \
  --lookups 100000 \
  --warmup-lookups 10000 \
  --output examples/results/1.0.2-local.json
```

The baseline reports:

- `durable_single_row_commit` — one row per transaction, including WAL sync;
- `durable_batched_insert_rows` — batched rows with one WAL sync per transaction;
- `current_point_get` — current in-memory MVCC point reads;
- `historical_get_at` — historical version lookup;
- `wal_recovery_open` — time required to reconstruct stores from the WAL.

For latency-bearing workloads the harness prints p50/p95/p99. JSON output is
available for later milestone-to-milestone comparisons.

## Benchmark discipline

Always compare versions using:

- the same physical machine,
- the same Rust toolchain,
- `--release`,
- the same filesystem/device,
- the same workload arguments,
- otherwise idle system where practical.

The durable commit benchmark intentionally includes the engine's WAL sync. It
therefore measures a meaningful database durability path, not only in-memory
mutation speed.

This early harness deliberately avoids Criterion or other new dependencies so
that Milestone 1.0.2 remains as close as possible to 1.0.1. A Criterion suite can
be introduced later when microbenchmarks become numerous enough to justify a
benchmark framework.
