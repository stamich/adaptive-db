# Milestone 2.5.4.1 — a trustworthy benchmark harness

**Status: in progress.** 6 of 31 tasks are done. Pull request: #118 (draft).

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
| 7-8 | Report schema 3 and validator | open |
| 9-13 | Rust families (rust-local, workloads, durable-write, memory, scaling, recovery) | open |
| 14-16 | Scala harness, `WorkloadBenchmark`, `sql-path` | open |
| 17-19 | `telemetry`, `actuator`, `ffi` families | open |
| 20-23 | Governor and storage gates, `--repeat`, interleaved A/B, gate exit codes | open |
| 24 | Calibrate limits on the reference machine (needs the user's Ryzen run) | open |
| 25-26 | `bench_report.py`, `bench_compare.py`, `scripts/benchmark.sh` | open |
| 27-28 | Tests, CI quick plan | open |
| 29-30 | Documentation, API-doc audit | open |
| 31 | Release: reference run, baseline, validation files (needs the user's Ryzen run) | open |

## Not claimed

No 2.5.4.1 reference-machine numbers exist yet. Limits set before task 24 are provisional. Details:
[BENCHMARKS.md](BENCHMARKS.md).
