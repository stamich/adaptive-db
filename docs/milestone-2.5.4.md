# Milestone 2.5.4 — AUTO_SAFE actuator

2.5.3 recommended. 2.5.4 **acts on the recommendation, measures the result, and keeps or undoes it**,
within limits a person sets. It adds no engine function: the C ABI stays 9, plan wire v4 and data
format 4 are unchanged, and no data migration is needed. The work is in the JVM, in a new module
`adb-actuator`, in the telemetry and advisor modules (write counters) and in the gateway.

The default is unchanged: `SET adaptive = off`. Nothing is ever done unless a person asks for
`auto_safe`. Design and details: [actuator.md](actuator.md).

## Numbering

2.5.4 has three components, so the Cargo crates are `2.5.4` (they follow the milestone, as in
2.5.3). The engine version string, the JVM build, the scripts and the reports say `2.5.4`.

## What was planned, and what was done

| Item | Result |
|---|---|
| `SET adaptive = off \| advisory \| auto_safe` | **done** — `AdaptiveMode`, binder, gateway; `SET advisor` maps onto it |
| Policy gate with reasons | **done** — `PolicyGate`, 11 `RejectReason`s, fixed order, pure function |
| Policy and budget | **done** — `Policy`, `Budget`, every default configurable |
| Actions: `CREATE INDEX`, `ANALYZE`, `DROP INDEX` only | **done** — `Action`, `Actuator`, `GatewayActuator` |
| Ownership: never drop a user's index | **done** — `OwnershipRegistry.mayDrop` plus `OwnedOnlyActuator` |
| Effectiveness measurement | **done** — `BaselineCapture`, `AfterPeriod`, `EffectivenessVerifier` on rows read per execution |
| Write cost of an index | **done** — per-entity write counters and `WriteCostCheck` |
| Rollback | **done** — `RollbackPlanner`, `FlapGuard` (doubling cooldown, capped at 8x) |
| Idle indexes | **done** — `IdleDetector` |
| Action log, `\actions`, `\actions log` | **done** — bounded, sequence-numbered |
| Build safety | **done** — `BuildGuard`, `maxBuildRows` (50 000), one build at a time |
| Adoption of `adv_*` indexes after a restart | **done** — never verified, idle rule only |
| Reference-machine benchmark runs | **not done** — see below |
| Persisting the registry and the log | **not done, on purpose** — the `adv_` prefix is the memory |
| Online index build | **not done** — an engine feature; the 50 000-row ceiling bounds the stall instead |
| Other actions (projections, partitions, settings) | **not done, on purpose** |

## Tasks

Each is one commit.

| # | Task | Result |
|---|---|---|
| 1 | Version 2.5.4 in the engine string, crates, JVM build, scripts, workflows, banners | `build-milestone-2.5.4.sh` |
| 2 | Freeze the 2.5.3 advisor contract with a structural test | `ContractTest` |
| 3 | Count writes per entity while telemetry is on | `WriteCounters`, `WriteStats` |
| 4 | Carry write figures through the windows and the physical design | `WorkloadModel`, `EntityDesign.writes` |
| 5 | Hold the write path to an allocation budget | benchmark `write_*` keys, `check-benchmarks.py` |
| 6 | Module `adb-actuator` and the action model | `Action`, `ActionKind`, `ActionState` |
| 7 | `Policy` and `Budget` with the proposal's defaults | `Policy.scala` |
| 8 | `PolicyGate` | 11 reasons, pure function |
| 9 | Ownership registry | `OwnershipRegistry` |
| 10 | Re-adopt `adv_*` indexes after a restart | `OwnershipRegistry.adopt` |
| 11 | `Actuator` interface and a recording fake | `FakeActuator` |
| 12 | `GatewayActuator` | runs actions under the DDL lock |
| 13 | `BuildGuard` | bounded builds, none after shutdown |
| 14 | Baseline from the workload windows | `BaselineCapture` |
| 15 | `EffectivenessVerifier` | rows read, time as a veto |
| 16 | Write cost of an index | `WriteCostCheck` |
| 17 | Idle advisor indexes | `IdleDetector` |
| 18 | Rollback as `DROP INDEX` of owned indexes, second guard | `RollbackPlanner`, `OwnedOnlyActuator` |
| 19 | `FlapGuard` | doubling rollback cooldown |
| 20 | `ActionLog` | bounded, sequence-numbered |
| 21 | `AdaptiveController` | one tick evaluates, gates, acts, verifies, rolls back |
| 22 | Reuse the advisor timer, map `SET advisor` onto the modes | `AdvisorScheduler` runs any tick |
| 23 | `SET adaptive` in the binder and the gateway | `BoundSetAdaptive`, `BinderTest` |
| 24 | `\actions`, `\actions log` | shell |
| — | Lower the automatic build ceiling to 50 000 rows | found by measuring the build stall (30 µs/row) |
| 25 | End-to-end smoke test | `examples/jvm-actuator-smoke` |
| 26 | Benchmark section and its check | `actuator` in the JVM report, `check_actuator` |
| 27 | Feature tour phase | `demo/sql/2.5.4-actuator.sql` |
| 28 | Complete the Scaladoc of the module | audit of every public item |
| 29 | Documentation | `actuator.md`, this note, README, CHANGELOG, roadmap, invariants 74 to 82, architecture, telemetry, results README |
| 30 | Validation records and reference results | `VALIDATION-2.5.4.txt`, `MILESTONE-2.5.4.json` |

## Invariants

1. **Off is the default and off is absent.** No thread, no evaluation, no state change on any query.
2. **Advisory never acts.** Only `auto_safe` runs a statement.
3. **Three statements only:** `CREATE INDEX`, `ANALYZE`, `DROP INDEX`.
4. **Only what it created can be dropped.** Two independent guards, the registry and the wrapper.
5. **At most one action per tick and one build at a time.**
6. **Every action is measured and every decision logged.** An index is kept only on evidence of
   fewer rows read; time can only veto.
7. **A rollback is remembered:** the same target waits longer each time (up to 8x).
8. **A person's decision stands:** a dropped advisor index is not rebuilt.
9. **Results never depend on the mode:** the same query returns the same rows in every mode.

## Findings while building it

- **`CREATE INDEX` stalls the database for the whole build**, reads and writes alike, about 30 µs per
  row. That made a ceiling on the size of tables the actuator builds on its own necessary: 50 000
  rows (about 1.5 s). The first draft had 1 000 000 rows, which would have frozen a database for
  half a minute from a timer thread.
- **Time cannot prove an improvement on a shared machine.** Rows read per execution is the engine's
  exact counter, so it is the evidence; time only vetoes a clear regression (2x).
- **A window that straddles the build is poison for the "after" figure.** `AfterPeriod` counts only
  windows that started at or after the moment the action was applied.
- **A write-heavy table may not deserve an index** even if reads improve; hence the write counters
  and `WriteCostCheck`, which compares the lost write time with the saved read time.
- **A quiet weekend is not an idle index.** `IdleDetector` ignores windows in which nothing ran.

## Measured

Everything below is a count or an exact engine counter, so it does not depend on the machine
(`scripts/check-benchmarks.py`, `actuator` section of the JVM report): in off and advisory mode,
0 actions; of two candidates, one index kept and one rolled back, the rolled-back target refused
during its cooldown, none failed; rows read per execution on the helped query fell from 5 000 to 250;
the user's own indexes were the same before and after. The demo's 2 001-row table shows the same
effect, 2 002 times fewer rows read.

The write counters add 0 bytes of allocation beyond the existing statement path in the benchmark
(budget 64). On the reference machine (Ryzen 9 5950X, `examples/results/2.5.4-*.json`) the checks
agree, the counts are identical to the VM's, a write allocates the same with and without the counters,
and the read and planning paths are unchanged against 2.5.3 (median -0.5 % over 15 timings). One controller tick (evaluation, gate, verification) took about 1 ms on the validation
VM and 0.46 ms on the reference machine; it runs every 10 s.

## Compatibility

- No engine, ABI, wire or data change. A 2.5.3 library works with 2.5.4 JVM modules except for the
  engine version string.
- New SQL: `SET adaptive = off | advisory | auto_safe`. New shell commands: `\actions`, `\actions log`.
- `SET advisor = on | off` is kept and means `advisory` | `off`.
- New public JVM API: package `io.adb.actuator`; `AdaptiveDatabase.controller`, `setAdaptive`,
  `adaptiveMode`, `showActions`, `showActionsLog`; `WriteCounters` in `io.adb.telemetry`;
  `EntityDesign.writes` and `WorkloadSnapshot` writes in `io.adb.advisor`.
- `AdvisorScheduler` takes any tick function (it ran only the advisor before).

## Definition of done

- [x] `off` is the default; `advisory` never acts; `auto_safe` creates, measures, keeps or undoes
- [x] Only `CREATE INDEX`, `ANALYZE` and `DROP INDEX` can run; only owned indexes can be dropped
- [x] Every rejection has a reason, logged once per target and reason
- [x] `SET adaptive`, `\actions`, `\actions log` covered by `BinderTest`, the smoke test and the demo
- [x] Scaladoc on every public item
- [x] `check-benchmarks.py` checks the actuator section exactly
- [x] JVM tests (405) and the three smoke tests pass against the real engine
- [x] Reference-machine run of `scripts/benchmark-db.sh` and `scripts/benchmark-ffi.sh`
      (`examples/results/2.5.4-*.json`)
- [x] Write rates explained: with the CPU governor set to `performance`, 2.5.4 writes as fast as the
      quiet 2.5.2.1 run (42.0k rows/s without index, 14.1k with 3 indexes); the lower rates of the first
      run came from the default governor (`examples/results/2.5.4-performance-*.json`)
- [ ] Reference-machine run of the JVM `BenchmarkMain` and `compare-write-path.sh` (carried over)
- [ ] Tuning of `Policy` and `Budget` defaults on real workloads

## Not verified

- Gradle (Maven Central is not reachable from the validation environment; CI runs it). The JVM
  modules were compiled with `scalac` 3.3.8 and `javac` 22 and the tests run with a JUnit stand-in.
- `auto_safe` under concurrent load for hours. The loop, the locks and the guards are covered by
  unit tests, a scripted rig and a short smoke test only.
- The Rust crates were re-versioned and checked, not changed.
