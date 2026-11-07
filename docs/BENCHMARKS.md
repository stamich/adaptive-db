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

Every limit below was chosen without the reference machine and is **provisional** until task 24
calibrates it from A/A and `--repeat` runs on the Ryzen 9 5950X; any status derived from one says so
(`limit_is_provisional` in the documents).

| Limit | Value | Where |
|---|---|---|
| Batch MAD % that makes a case `unstable` | 5.0 | `DEFAULT_STABLE_BATCH_MAD_PERCENT`, `Stability` in the JVM |
| Run-to-run spread for `--repeat` | 10 % | `scripts/bench_repeat.py` |
| A/B and compare: change that counts | 5 % (and above the A/A control) | `bench_ab.py`, `bench_compare.py` |

## The heavy plan

A durable case whose single invocation takes a second or more (loading the dataset, the thousand
commits of workload I, the write-overhead loads, the single-thread commits of `scaling`) uses the
**heavy plan**: one warm-up and 3 batches of 3 samples, ten invocations instead of the 27 of the
durable plan. On a disk where `fsync` costs 7 ms, the durable plan made the `workloads` family run
for close to an hour; with the heavy plan it takes 10-20 minutes. Families print one progress line per
section to stderr.

## Calibration on the reference machine

First reference run (Ryzen 9 5950X, `performance` governor, data on ext4, `fsync` p50 2.2-5.4 ms),
86 timed cases in `rust-local`, `workloads` (Rust) and `workloads` (JVM):

| Family | Cases | Median batch MAD | 90th percentile | Highest |
|---|---:|---:|---:|---:|
| rust-local | 17 | 0.08 % | 2.0 % | 4.3 % (`commit_single_row.n_threads`) |
| workloads (Rust) | 38 | 0.15 % | 1.4 % | 3.3 % (`analyze.jo_b`); L excluded, see below |
| workloads (JVM) | 31 | 0.08 % | 0.8 % | 6.3 % (`A planning_only`, measured while compiling) |

* The **5 % batch MAD limit holds**: no case that was warm and not a poll-bound latency exceeded it.
* `L_projection_wait_latency` measured 49.9 %: its consumer polls every 200 µs, so the spread is the poll
  interval. Its stability is no longer judged (`stability_skipped_reason`).
* The JVM cases above 1 % were measured while the JIT was compiling; the runner now settles the JIT for
  at least 2 s.
* Not calibrated yet: the run-to-run spread of `--repeat` (10 %) and the A/B threshold (5 %), which need
  `--repeat 3` and A/A runs on the same machine.

## Report schema 3

Every family writes one JSON document: `schema` (3), `milestone`, `family`, `environment`,
`runtime_before`, `runtime_after`, `plan`, usually `storage` and `fsync_probe`, and the cases. A timed
case keeps every key the 2.5.4 report had and adds a `timing` object: `median_of_medians_ms`,
`batch_medians_ms`, `batch_mad_percent`, `outlier_count`, `outlier_indices`, `stability`, `batches`,
`samples_per_batch`, `iterations_per_sample`, `quick`. A case that is not a timing (a byte count, a row
count) is a *plain case* with a `value` or a `skipped_reason`. JVM reports also carry `jit` (did the
code warm up? how much GC ran?) and a `fresh_fork` flag (was the family alone in its JVM?).
`scripts/bench_validate.py` implements the contract (`crates/adb-bench/src/report.rs` `validate` is the
Rust side; both are tested against each other). Reports from 2.5.x and earlier are classified `legacy`
and are never rejected, but they are always *diagnostic* when compared.

## Families

| Family | Language | What it measures | Plan |
|---|---|---|---|
| `rust-local` | Rust | scans, point lookups, load, durable commits (1 and N threads), CDC read, recovery, relational operators | release + durable |
| `workloads` | Rust | workloads A-H and the consistency workloads I-O on the engine catalog | release + durable |
| `memory` | Rust | bytes and calls allocated per operation (a counting allocator; deterministic) | none (counts) |
| `scaling` | Rust | read scaling over threads, write amplification, recovery time over log size | release + durable |
| `sql-path` | JVM | SQL text to plan, plan cache on and off, `sql_to_rust` | JVM plan |
| `telemetry` | JVM | cost of capturing a query at `off`, `basic` and `full` | JVM plan |
| `actuator` | JVM | controller tick and the stall of a `CREATE INDEX` | JVM plan |
| `ffi` | JVM | native calls, prepared plans, profile fetch | JVM plan |

JVM families are launched one per JVM (`ADB_BENCH_FORK=1`), so the JIT profile of one cannot colour
another; the JVM harness measures like the Rust one (warm-up, calibration, batches) and records whether
the JIT was still compiling and how much GC ran during the samples.

## Gates and exit codes

`scripts/bench_gate.py report.json...` evaluates reports:

| Gate | Result |
|---|---|
| governor | `performance` -> pass; anything else, or unreadable -> **diagnostic** |
| storage | `tmpfs`, `ramfs`, `devtmpfs` under a durable family (`rust-local`, `workloads`, `scaling`) -> **fail**; otherwise pass or skipped |
| stability | a case with `stability: unstable` -> **unstable**; quick plans are skipped |
| jit | a JVM case measured while compiling (`warm: false`) -> **unstable** |
| schema | a schema-3 report that breaks the contract -> **fail**; a legacy report -> diagnostic |

Exit codes of all tools: **0** pass or skipped, **1** fail, **3** unstable, **4** diagnostic; **2** is a
usage or tool error. When several apply: fail, then diagnostic, then unstable. *Timings never gate CI*:
hosted runners have no `performance` governor, so CI accepts 3 and 4 and fails only on 1 and 2.

## Running benchmarks

```bash
scripts/benchmark.sh all                       # every family, one JVM per JVM family
scripts/benchmark.sh rust-local --quick        # a smoke run (not release grade)
scripts/benchmark.sh workloads --repeat 3      # three fresh processes, one median document
scripts/benchmark.sh rust-local --ab v2.5.4    # interleaved A/B against a git ref
```

* `--repeat N` (`scripts/bench_repeat.py`) runs a family N times as fresh processes. Deterministic
  values (`checksum`, `rows_out`, `rows`, `entries`, `table_rows`, `expected_changes`, `changed_rows`,
  and `value` of untimed cases) must be identical in every run, else **fail**; the spread of the
  per-run medians above the limit is **unstable**.
* Without `--repeat`, `scripts/bench_run.py` measures a family again (up to twice) while it is
  `unstable`; `diagnostic` and `fail` are not retried because measuring again does not help.
* `--ab REF` (`scripts/bench_ab.py`) builds a git worktree of REF, copies the *current* harness
  (`crates/adb-bench`, `examples/rust-benchmark`) over it so the same binaries measure both engines, and
  runs A (the old engine), B (the working tree) and A2 (a second copy of A, the A/A control) in a
  rotating order. A change is reported only when it exceeds both the limit and the control noise; a
  noisy control makes the comparison `unstable`. Rust families only.
* `scripts/bench_compare.py old.json new.json` compares two reports; it is conservative (legacy reports
  and non-`performance` governors give `diagnostic`). For a verdict use `--ab`.
* `scripts/bench_report.py report.json` renders a report as Markdown.
* `scripts/benchmark-db.sh` and `scripts/benchmark-ffi.sh` stay as shortcuts for the `workloads`
  family of each language. `ADB_BENCH_ITERATIONS`, `ADB_BENCH_WARMUP_MS` and `ADB_BENCH_DATA` are
  ignored since 2.5.4.1: the measurement plan decides and every run gets a fresh data directory.

## Running the tests

```bash
cargo test -p adb-bench
cargo clippy -p adb-bench --all-targets -- -D warnings -W missing_docs -W clippy::missing_docs_in_private_items
RUSTDOCFLAGS="-D warnings" cargo doc -p adb-bench --no-deps
python3 -m unittest discover -s scripts/tests         # gates, repeat, A/B, compare, launcher, Scaladoc check
python3 scripts/check-scala-docs.py $(find jvm/adb-bench*/src/main -name '*.scala')
(cd jvm && ./gradlew :adb-bench:test)                 # the Scala harness
```

## What is not claimed

No 2.5.4.1 reference-machine numbers exist yet: the committed reports of this milestone come from a
2-CPU virtual machine without a CPU governor, and every number from it is diagnostic. The release
numbers (task 31) and the calibration of the limits (task 24) need a run on the reference machine; see
[milestone-2.5.4.1.md](milestone-2.5.4.1.md).
