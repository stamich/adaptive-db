# Benchmark results

Store machine-specific Milestone 1.6.2 JSON outputs here. Do not commit fabricated baseline
numbers. For useful comparisons record CPU, memory, storage device/filesystem, OS/kernel,
Rust version and whether the filesystem/cache state was warm or cold.

## Milestone 2.2.3 workloads

```bash
ADB_BENCH_SCALE=10 ./scripts/benchmark-db.sh    # rust_native       -> 2.2.3-database-sf10.json
ADB_BENCH_SCALE=10 ./scripts/benchmark-ffi.sh   # JVM / FFI paths   -> 2.2.3-ffi-sf10.json
python3 scripts/check-benchmarks.py examples/results/2.2.3-database-sf10.json examples/results/2.2.3-ffi-sf10.json
```

The dataset is defined once, in `examples/rust-benchmark/src/bin/workloads/dataset.rs`. The Rust
binary measures `rust_native` on it, or prepares a database directory (`--prepare`) that the JVM
benchmark opens with a matching catalog. Loading through SQL would cost one commit per row.

| Table | Rows at scale 1 | Scaled | Role |
|---|---|---|---|
| `bench_customer` / `bench_orders` | 100 / 1,000 | yes | workloads A and B |
| `jo_c` / `jo_b` / `jo_a` | 20 / 2,000 / 20,000 | b, a | chain join order |
| `st_region` / `st_dim` / `st_fact` | 100 / 1,000 / 100,000 | dim, fact | star join order; SQL order builds the fact table |
| `sk_events` | 100,000 | yes | Zipf `kind` (kind 0 ≈ 26%), `region = kind + 100` (perfectly correlated) |
| `sk_stale` | 100,000 | yes | analyzed uniform, then half the rows moved to kind 0 |

Sections of the reports:

* **workloads** — A (hash join + TopK) and B (+ aggregate) on every path: `rust_native` (Rust),
  `planning_only` (parse, bind, plan, encode), `ffi_prepared_plan` (boundary + engine) and
  `scala_cbo_ffi_rust` (the whole SQL path). The p50, p95 and p99 come after a time-based warm-up
  (200 ms in Rust, 2 s on the JVM, for the JIT). `rust_native` also reports peak operator memory
  and input rows per second.
* **analyze** — time per table, total rows per second, and the HyperLogLog check on 100,000 ×
  scale unique ids.
* **join_order** — chain and star queries in cost and rule mode. A mode that exceeds an engine
  limit reports its error instead of a timing (the star in rule mode at scale 10).
* **estimation** — estimated versus actual rows and the q-error for a most common value, a rare
  value, a perfectly correlated predicate pair (independence assumed), a range, and stale
  statistics before and after `ANALYZE`.
* **planner** — planning time of chain and star queries over 2–12 relations (DP up to 10, greedy
  above).
* **calibration** — engine milliseconds per 1,000 estimated cost units for every plan, and their
  spread.

`scripts/check-benchmarks.py` checks what must hold on every machine; timings are never checked.
Every path must return the same checksum. Cost and rule mode must return the same rows, and cost
mode must not pick a plan it estimates as dearer. Fresh statistics must estimate the plain
workloads within q ≤ 1.5. Stale statistics must be flagged and repaired by `ANALYZE`.

Recorded files: `2.2.3-database.json` / `2.2.3-ffi.json` (scale 1) and `-sf10` (scale 10), from a
2-vCPU cloud VM (Intel Xeon 2.8 GHz, warm cache, release build, JDK 22). Single-row commit
throughput in `2.2.3-rust-local.json` depends mostly on how fast the disk completes `fsync`, not on
the CPU.
