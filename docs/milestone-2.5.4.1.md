# Milestone 2.5.4.1 — a trustworthy benchmark harness

**Status: in progress.** 29 of 31 tasks are done; 24 and 31 need a run on the reference machine. Pull request: #118 (draft).

## Why

The 2.5.4 reports mostly held single numbers and said nothing about the machine. The same engine code
wrote 5.1k or 42.0k rows/s depending on the CPU governor (`crates/` was unchanged between 2.5.2.1 and
2.5.4 apart from version strings), and the reports could not tell that apart from a regression. The
design follows the harness of ACE (`ace-bench`), adapted to a database that is bound by `fsync`.

## Decisions

- Version **2.5.4.1**; Cargo crates stay `2.5.4` (Cargo versions have three components). Engine
  string, JVM build, scripts and workflows say 2.5.4.1.
- No engine change: C ABI 9, plan wire v4, data format 4.
- Tag `v2.5.4` on the merge of PR #117 (`c42f358`) is the A/B baseline.
- All 31 tasks, not only the core ones. CI runs a quick plan and the validator on every PR; timings
  never gate CI. This milestone comes before 2.5.5.
- Durable writes use their own plan (one invocation per sample) and an fsync control probe.

## Tasks

| # | Task | State |
|---|---|---|
| 1 | Version 2.5.4.1 in engine, JVM, scripts, workflows | done |
| 2 | Crate `adb-bench`: plan and runner | done |
| 3 | Robust statistics | done |
| 4 | Environment record | done |
| 5 | Storage record | done |
| 6 | fsync control probe | done |
| 7-8 | Report schema 3 and validator (`report.rs`, `bench_validate.py`) | done |
| 9-13 | Rust families (rust-local, workloads, durable-write, memory, scaling, recovery) | done |
| 14-16 | Scala harness (`adb-bench` module), `WorkloadBenchmark`, `sql-path` | done |
| 17-19 | `telemetry`, `actuator`, `ffi` families | done |
| 20 | Governor and storage gates, statuses and exit codes (`bench_gate.py`) | done |
| 21 | `--repeat` with deterministic and spread gates (`bench_repeat.py`) | done |
| 22 | Interleaved A/B with an A/A control (`bench_ab.py`) | done |
| 23 | Gate exit codes 0/1/3/4 wired together, remeasure on unstable (`bench_run.py`, `check-benchmarks.py`) | done |
| 24 | Calibrate limits on the reference machine: batch MAD limit confirmed by the first Ryzen run; `--repeat` and A/B limits still need repeat and A/A runs | in progress |
| 25 | `bench_report.py`, `bench_compare.py` | done |
| 26 | `scripts/benchmark.sh`, `bench-jvm-family.sh`, updated `benchmark-*.sh` and build script | done |
| 27 | Tests (script tests, launcher, committed reports, harness tests) | done |
| 28 | CI: quick plan and validator on every PR; timings never gate | done |
| 29 | Documentation | done |
| 30 | Rustdoc / Scaladoc / Javadoc audit (`clippy::missing_docs_in_private_items`, `-D warnings` rustdoc, `check-scala-docs.py`) | done |
| 31 | Release: reference run, baseline, validation files (needs the user's Ryzen run) | open |

## Not claimed

No 2.5.4.1 reference-machine numbers exist yet. Limits set before task 24 are provisional. Details:
[BENCHMARKS.md](BENCHMARKS.md).
