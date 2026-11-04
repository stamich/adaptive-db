# Benchmarks (Milestone 2.5.4.1)

Why this exists: on the reference machine (Ryzen 9 5950X) the same 2.5.4 code wrote 5.1k rows/s with
the default CPU governor and 42.0k rows/s with `performance`. A number without the state of the
machine cannot be compared. The harness in `crates/adb-bench` measures, and records, both.

## Before you measure

1. Set the governor: `sudo cpupower frequency-set -g performance` (check
   `/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor`).
2. Use a real disk under the data directory, not `tmpfs`/`ramfs`: durable writes are `fsync`-bound.
3. Keep the machine quiet; the harness records load and temperature, but cannot remove them.
4. Compare runs only when the governor, storage and toolchain are the same.

## What the harness provides (`adb-bench`)

| Module | What it does |
|---|---|
| `plan` | `MeasurementPlan`: warm-up, calibration, batches x samples. `release()` is 5 warm-ups, samples of at least 50 ms, 3 batches of 7. `durable()` is one invocation per sample, 5 x 5, because an `fsync` is slow and noisy. `quick()` and `durable_quick()` are for CI. `ADB_BENCH_QUICK=1` selects the quick plans; `ADB_BENCH_MIN_SAMPLE_MS` changes the sample length of CPU-bound plans only |
| `runner` | `measure(plan, f)`: warm-up, calibration by doubling, then the batches; `black_box` keeps the work from being optimised away |
| `stats` | `SampleStats`: **median of medians** is the headline; **batch MAD %** is the stability input; outliers (beyond 3 x 1.4826 x MAD) are *reported, never removed*; p95/p99 by nearest rank. `Stability` is `stable`, `unstable` or `skipped` (quick plans do not judge) |
| `environment` | `Environment` (OS, kernel, CPU, governor, max frequency, rustc, profile) and `RuntimeSnapshot` (frequency, hottest thermal zone, load average). A governor other than `performance` makes a result *diagnostic*, never a pass |
| `storage` | `StorageInfo`: filesystem, mount point, device and free space under the data directory; `tmpfs`, `ramfs` and `devtmpfs` are flagged `memory_backed` |
| `probe` | `FsyncProbe`: a fixed 4 KiB write + `fdatasync`, p50/p95. Run before and after a durable family; `drift_percent` shows whether the machine changed meanwhile |

Unknown values are `null` in the JSON, never a guess.

## Limits are provisional

`DEFAULT_STABLE_BATCH_MAD_PERCENT = 5.0` was chosen without the reference machine. It will be
calibrated from A/A and `--repeat` runs on the Ryzen (task 24) and then replaced; until that is done,
any status derived from it is provisional.

## Still to come in 2.5.4.1

Report schema 3 and `scripts/bench_validate.py`; the harness in the Rust and Scala benchmarks;
families (`durable-write`, `memory`, `scaling`, `recovery`, `sql-path`, `telemetry`, `actuator`,
`ffi`); `--repeat N` with deterministic and spread gates; interleaved A/B against a previous git ref
with an A/A control; gate exit codes 0/1/3/4; `scripts/benchmark.sh`. This page is extended as each
arrives. See [milestone-2.5.4.1.md](milestone-2.5.4.1.md).

## Running the harness tests

```bash
cargo test -p adb-bench
cargo clippy -p adb-bench --all-targets -- -D warnings -W missing_docs -W clippy::missing_docs_in_private_items
RUSTDOCFLAGS="-D warnings" cargo doc -p adb-bench --no-deps
```
