# Benchmark results

Store machine-specific benchmark JSON outputs here. Do not commit fabricated baseline
numbers. For useful comparisons record CPU, memory, storage device/filesystem, OS/kernel,
Rust version and whether the filesystem/cache state was warm or cold.

## Milestone 2.5.3 workload advisor

```bash
./scripts/benchmark-db.sh && ./scripts/benchmark-ffi.sh                       # -> 2.5.3-database.json, 2.5.3-ffi.json
python3 scripts/check-benchmarks.py examples/results/2.5.3-database.json examples/results/2.5.3-ffi.json
```

The JVM report has a new `advisor` section (workload E: an unindexed filter reading every row to
return one). `check-benchmarks.py` checks its counts exactly: no advice after two evaluations, an
index on `ix_account(region)` after three, an unchanged index count, a non-empty audit log. 
Recorded by the author on the AMD Ryzen 9 5950X (release build, scale 1);
`check-benchmarks.py` reports "benchmark results agree (scale 1)".

* **Advisor section:** no advice after two evaluations, `CREATE INDEX adv_ix_account_region ON
  ix_account (region)` after three, 2 indexes before and after, 2 audit entries. One evaluation,
  including building the design from the engine, took 0.76 ms (smallest of four).
* **Telemetry allocation unchanged:** skipped +0 bytes, captured +144 bytes, 10 rows read.
* **Read, planning and engine paths unchanged against 2.5.2.1:** the 45 timings of the ffi report
  have a median change of -0.4 % (range -7.2 % to +8.3 %), as expected for an engine that only
  changed its version string.
* **Write path, again the disk:** writes without index 6 924 rows/s, three indexes 4 044, load
  48 020, single-row commit 72/s, against 41 439, 14 111, 60 819 and 205/s in the quiet 2.5.2.1 run.
  The engine is the same code, so this is the disk state of this run (see the 2.5.2.1 notes below),
  not a regression; compare disk-bound figures only between runs on a quiet disk.

## Milestone 2.5.2.1 telemetry capture path

```bash
./scripts/benchmark-db.sh && ./scripts/benchmark-ffi.sh                       # -> 2.5.2.1-database.json, 2.5.2.1-ffi.json
python3 scripts/check-benchmarks.py examples/results/2.5.2.1-database.json examples/results/2.5.2.1-ffi.json
```

Recorded by the author on the same AMD Ryzen 9 5950X (release build, scale 1) after closing other
applications; `scripts/check-benchmarks.py` reports "benchmark results agree (scale 1)". The kept
files are the **second** run, made on a quiet disk. The first run (busy disk, same build) is not
kept; it is why the write-path numbers below must be read with the disk in mind.

* **Telemetry allocation (new `telemetry` section of the ffi report):** a skipped query allocates
  +0 bytes more than an unobserved one and a captured query +144 bytes (the event); a capture
  reports 10 rows read for workload C. Allocation does not depend on the machine, so these equal
  the figures measured on the validation VM. 2.5.2 measured +24 and about +2 340 bytes.
* **Read, planning and engine paths are unchanged against 2.5.1:** the 31 timings of the ffi report
  have a median change of -0.8 % (range -3.7 % to +3.0 %), the read workloads of the database report
  +0.1 %, and `rust_native` point lookup is 1 000 ns against 1 250 ns. C through the full SQL path:
  0.0270 -> 0.0260 ms.
* **Write path, same code, different disk state.** Between the first and the second run of this
  build, with nothing changed but the machine, writes without index went from 6 395 to 41 439
  rows/s, with three indexes from 3 762 to 14 111, schema-validated writes from 6 358 to 44 198,
  load from 50 419 to 60 819 rows/s and the single-row commit of the Rust micro-benchmark from 74
  to 136 (one thread) and 177 to 343 (eight threads) commits/s. The 2.5.2 and the first 2.5.2.1
  reports (about half the 2.5.1 write rates) therefore did not measure a regression: no code on
  those paths changed between 2.5.1 and 2.5.2.1 apart from version strings.
* **Commit workload I is not a baseline.** The 659 single-row commits/s of the 2.5.1 report is the
  outlier: that run's own single-row commit test gave 73/s, 205/s in this run (136/s in the
  micro-benchmark), and workload I and the micro-benchmark do the same thing (one fsync per commit).
  Compare disk-bound figures only from runs on the same disk in the same state; the commit rate is
  set by the device's flush latency.
* The `telemetry` timing comparison (off, skipped, captured) belongs to the JVM `BenchmarkMain`,
  which is not part of these three reports and has not been run on this machine.

## Milestone 2.5.1 plan cache

```bash
./scripts/benchmark-db.sh && ./scripts/benchmark-ffi.sh                       # -> 2.5.1-database.json, 2.5.1-ffi.json
python3 scripts/check-benchmarks.py examples/results/2.5.1-database.json examples/results/2.5.1-ffi.json
```

Recorded by the author on the same AMD Ryzen 9 5950X as the 2.5.0 files (release build, scale 1);
`scripts/check-benchmarks.py` reports "benchmark results agree (scale 1)". The engine and the
workloads are unchanged, so only the paths that go through the SQL gateway should move.

* **Full SQL path (`scala_cbo_ffi_rust`, ffi report), 2.5.0 -> 2.5.1:** C (covering index point
  read) 0.101 -> 0.028 ms (3.6x faster), D (index range, order, limit) 0.226 -> 0.123 ms (1.8x),
  A 0.622 -> 0.526 ms (-15 %), B 0.651 -> 0.539 ms (-17 %), E (sequential scan, 48 ms) unchanged.
  The cached statement no longer pays parse, bind, plan and metadata work, so the gain is
  largest where the engine is fastest and vanishes where the engine dominates.
* **Engine paths are unchanged:** `rust_native` workloads A-E are within 1-4 % of 2.5.0 (A 0.468 ->
  0.475 ms, C 0.0197 -> 0.0197 ms, D 0.104 -> 0.101 ms, E 35.3 -> 33.9 ms) and `ffi_prepared_plan`
  within 3 %; `planning_only` is unchanged (it measures the planner, not the cache).
* **Not comparable, disk-bound:** commit throughput (I: 63 -> 659 single-row commits/s, 60 -> 170
  commits/s of 100 rows) is set by the disk's fsync, which differs between runs; `write_overhead`
  (15.9k -> 9.6k rows/s), `load` and `analyze` also moved by -45 % to -28 %, and the schema-evolution
  write validation by +90 %, in directions that have nothing to do with a JVM cache (those paths
  do not use the gateway). They show the run-to-run noise of this machine's storage, not a
  regression or a gain of 2.5.1.
* Change log, cursors, projection wait (L: 0.11 -> 0.17 ms p50, still sub-millisecond) and
  rebuild workloads J-O are otherwise as in 2.5.0.

## Milestone 2.5.0 consistency

```bash
./scripts/benchmark-db.sh && ./scripts/benchmark-ffi.sh                       # -> 2.5.0-database.json, 2.5.0-ffi.json
python3 scripts/check-benchmarks.py examples/results/2.5.0-database.json examples/results/2.5.0-ffi.json
```

The `consistency` section of the database report (workloads I–O, see [../README.md](../README.md))
measures the commit order, the change log and the projection runtime. The committed 2.5.0 files
were recorded on an AMD Ryzen 9 5950X (32 threads, release build), scale 1, 100 iterations;
`scripts/check-benchmarks.py` reports "benchmark results agree".

* Workloads A–E (`rust_native`) are within 1–2 % of 2.4.0 on the same machine (A 0.468 vs 0.460 ms,
  C 0.019 vs 0.018 ms, D 0.104 vs 0.101 ms, E 35.4 vs 34.8 ms); the schema-evolution scan G is
  unchanged (21.3 ms).
* I: 63 single-row commits/s and 60 commits/s of 100 rows, 188 change-log bytes per single-row
  commit. Commit throughput here is bound by the disk's fsync, which differs between runs and
  machines (the 2.4.0 write-overhead figures were 7.4k rows/s without an index, 2.5.0's 15.9k on
  the same machine), so it says nothing about the cost of the change log by itself.
* J: the change log streams 511,000 commits/s (685 MiB/s). K: opening a cursor and reading its
  first batch takes 0.69 ms at the start, 0.71 ms in the middle and 5 µs at the end of a 28-segment log.
* L: a consumer that follows the log is reached within 0.11 ms (p50) and 0.29 ms (p99); no wait
  timed out. M: catching up on 5,000 commits runs at 1.64 million commits/s.
* N: opening 5,000 commits takes 20 ms with an intact change log and 69 ms when it is rebuilt from
  the WAL; both reach the same head.
* O: pruning a consumed 28-segment log removes 27 segments in 5.8 ms; a byte limit against a
  slow consumer removes 14 segments in 8.1 ms and marks the consumer as needing a rebuild.

## Milestone 2.4.0 schema evolution

(These runs used the 2.4.0 file names; the scripts now write `2.5.1-*.json` unless
`ADB_BENCH_OUT` says otherwise.)

```bash
./scripts/benchmark-db.sh && ./scripts/benchmark-ffi.sh                       # -> 2.4.0-database.json, 2.4.0-ffi.json
python3 scripts/check-benchmarks.py examples/results/2.4.0-database.json examples/results/2.4.0-ffi.json
```

The workloads A–E and the JVM sections are unchanged, but the JVM benchmark now creates its
tables in the engine catalog (every row is read through its schema). The Rust report gains a
`schema_evolution` section, measured on a database of its own: four `sc_account` tables of
50,000 rows × scale (`id`, `balance`, `owner`), three loaded at schema version 1 and then given
`ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR'` with 0 %, 50 % and 100 % of their rows
rewritten (migrated to version 2), and an unversioned control table that stores `currency` in
every row as 2.3.0 would.

* **F_virtual_default_point_read**: a point lookup of a row that predates `currency` (the
  default is filled in on read) and of a migrated row; both must read `EUR`.
* **G_schema_adapted_scan**: `COUNT(*), SUM(balance) WHERE currency = 'EUR'` over the three
  mixes and the control table; all four return the same checksum.
* **H_stale_schema_epoch_reject**: `execute_planned` of a point lookup planned one epoch ago
  (every call is refused) and of the same plan at the current epoch.
* **alter_latency**: `ADD COLUMN` on a 1,000-row and a 50,000 × scale-row table; the log bytes
  are the size of one catalog record either way, because no row is rewritten.
* **write_validation**: 20,000 inserts, 100 per commit, into a table with a schema (stamping,
  defaults, type checks) and one without.

The committed 2.4.0 files were recorded on an AMD Ryzen 9 5950X (32 threads, release build,
JDK 22; see each report's `host`), the machine of the 2.3.0 comparison runs. Scale-10 runs
(`ADB_BENCH_SCALE=10`) are not included. Against 2.3.0 on the same machine, the rust_native path
is unchanged; the JVM paths, whose tables now have a schema, take about 10 µs more planning per
query and 7–12 % more on large scans (the per-row adapter lookup, also visible in workload G:
19.8 ms for the unversioned control, 21.3 ms with a schema and no row to adapt).

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
