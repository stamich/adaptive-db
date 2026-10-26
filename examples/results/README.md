# Benchmark results

Store machine-specific benchmark JSON outputs here. Do not commit fabricated baseline
numbers. For useful comparisons record CPU, memory, storage device/filesystem, OS/kernel,
Rust version and whether the filesystem/cache state was warm or cold.

## Milestone 2.3.0 workloads

```bash
./scripts/benchmark-db.sh && ./scripts/benchmark-ffi.sh                       # scale 1
ADB_BENCH_SCALE=10 ./scripts/benchmark-db.sh                                   # -> 2.3.0-database-sf10.json
ADB_BENCH_SCALE=10 ADB_BENCH_ITERATIONS=20 ./scripts/benchmark-ffi.sh          # -> 2.3.0-ffi-sf10.json
python3 scripts/check-benchmarks.py examples/results/2.3.0-database.json examples/results/2.3.0-ffi.json
python3 scripts/fit-cost-weights.py examples/results/2.3.0-ffi.json examples/results/2.3.0-ffi-sf10.json
```

2.3.0 keeps the 2.2.3 dataset and sections below and adds the table `ix_account` (100,000 rows ×
scale): `region` takes 100 values and has no index; `balance` is `id · 48,271 mod 1,000,000,007`,
which is unique, so `ORDER BY balance` has no ties, and spread like random values; `owner` takes
10,000 values; `note` is a payload column that no index covers. Indexes:
`idx_ix_owner (owner) INCLUDE (balance)` and `idx_ix_balance (balance)`.

New sections:

* **index_build**: `CREATE INDEX` over the loaded rows. Rust also counts the entries with a full
  index scan, which must equal the rows.
* **workloads C, D, E**: C is a covering equality scan (`owner = 'owner-42'`). D is an ordered
  range with row fetch and `LIMIT 100`, so no sort runs. E is the sequential control
  (`region = 7`, no index). Each runs on every path, with the access path cost mode chose.
* **selectivity_sweep** (JVM): a non-covering `balance < x` range selecting 0.01% to 50% of the
  rows, run as an index plan (planned with sequential pages made prohibitively expensive), a
  sequential plan (rule mode) and the cost-mode choice. `regret` is the chosen plan's time over
  the faster plan's (1.0 is a perfect choice). Every plan is also a calibration point with
  random accesses.
* **as_of_index_read** (Rust): workload D before and after 1% of the balances change, both
  through the index.
* **write_overhead** (Rust): 20,000 inserts, 100 per commit, into fresh databases with 0, 1 and
  3 indexes.
* Every timing now also reports `min_ms`, `max_ms`, `stddev_ms` and `round_medians_ms` (the
  medians of 7 consecutive rounds, which show the run-to-run spread).

Recorded on the 2-vCPU cloud VM (p50 of 100 iterations at scale 1 and of 30 (Rust) or 20 (JVM)
at scale 10):

| | scale 1 | scale 10 |
|---|---|---|
| C covering index equality: rust_native / ffi_prepared_plan / full SQL | 0.025 / 0.040 / 0.105 ms | 0.083 / 0.148 / 0.49 ms |
| D index range + row fetch + LIMIT 100 | 0.155 / 0.174 / 0.372 ms | 0.35 / 0.41 / 0.83 ms |
| E sequential scan control | 57 / 88 / 83 ms | 1,703 / 912 / 936 ms |
| index build (`CREATE INDEX`, per index) | 3.8–4.0 s | 39–43 s (about 25,000 rows/s) |
| inserts with 1 / 3 indexes, relative to none | 3.0× / 5.5× slower | 3.1× / 6.7× slower |
| D through the index: latest / `AS OF` before 1% changed | 0.47 / 0.27 ms | 0.34 / 0.48 ms |

| Selectivity sweep (rows) | index plan | sequential plan | cost mode chose | regret |
|---|---|---|---|---|
| 0.01% (10 / 101) | 0.14 / 0.93 ms | 87 / 990 ms | index / index | 1.00 / 1.00 |
| 0.1% (103 / 1,015) | 0.25 / 1.6 ms | 88 / 936 ms | index / index | 1.00 / 1.00 |
| 1% (1,035 / 10,151) | 1.2 / 19 ms | 87 / 936 ms | index / index | 1.00 / 1.00 |
| 5% (5,178 / 50,754) | 7.3 / 105 ms | 87 / 928 ms | index / index | 1.00 / 1.00 |
| 20% (20,716 / 203,020) | 31 / 397 ms | 89 / 964 ms | index / index | 1.00 / 1.00 |
| 50% (51,790 / 502,807) | 87 / 1,064 ms | 91 / 975 ms | seq / seq | 1.06 / 1.00 |

(scale 1 / scale 10 in each cell). Cost mode chose the faster plan at every point, except at 50%
on scale 1, where both plans take the same time within run-to-run noise. The rust_native time
of E at scale 10 is higher than the FFI paths. That run measured E right after building two
1,000,000-row indexes in the same process, and the JVM run, in its own process, is the better
indicator. Writes are the price of indexes: every index adds a tree insert per row, and inserts
with three indexes ran 5.5–6.7× slower. Builds are single-threaded tree inserts. Both are listed
as 2.3.x follow-ups in [docs/roadmap.md](../../docs/roadmap.md).

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
