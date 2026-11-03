# Milestone 2.5.3 — workload model and advisor

2.5.2 and 2.5.2.1 made it cheap to know what the workload does. 2.5.3 uses that knowledge: it
turns telemetry into **advice**. It adds no engine function: the C ABI stays 9, plan wire v4 and
data format 4 are unchanged, and no data migration is needed. Everything is in the JVM, in a new
module `adb-advisor`, wired into the gateway.

The advisor **only recommends**. It never runs a statement, creates an index or changes a setting;
that is a rule of this milestone, not a missing feature. Design and details: [advisor.md](advisor.md).

## Numbering

2.5.3 has three components, so the Cargo crates are `2.5.3` again (they stayed at `2.5.2` during
2.5.2.1). The engine version string, the JVM build, the scripts and the reports say `2.5.3`.

## What was planned, and what was done

The roadmap (and the milestone notes of 2.5.2 and 2.5.2.1) listed for 2.5.3: time-windowed
aggregation, detectors for hot query, scan amplification, hot join and cardinality error,
candidates, heuristic cost-benefit scoring with hysteresis, an audit log and `\advice`.

| Item | Result |
|---|---|
| Time-windowed aggregation | **done** — `WorkloadModel`, a bounded ring of delta windows |
| Detectors: hot query, scan amplification, hot join, cardinality error | **done** — four pure functions of an immutable context |
| Candidates | **done** — `CreateIndex`, `RunAnalyze`, `Informational` |
| Heuristic cost-benefit scoring | **done** — `CostBenefitModel`, every parameter configurable |
| Hysteresis | **done** — 3 evaluations in, 3 out; carried-out advice withdrawn at once |
| Audit log | **done** — bounded, sequence-numbered |
| `\advice` | **done** — plus `\advice log` and `SET advisor = on \| off` |
| "Advisor on snapshots only" (moved here from 2.5.2.1) | **done** — it reads `ShapeStats` values, shape definitions and a `PhysicalDesign`; never the queue or a live structure |
| Double-buffered aggregation in telemetry | **not done** — the advisor reads each shape's counters under that shape's own lock, once per tick; no contention was measured, so there is nothing to fix |
| Join-projection and partition candidates as actions | **not done, on purpose** — informational until their actuators exist (2.6 and later) |
| Acting on the advice automatically | **not done, on purpose** — recommend only |

## Tasks

Each is one commit.

| # | Task | Result |
|---|---|---|
| 1 | Version 2.5.3 in the engine string, crates, JVM build, scripts, workflows, banners | `build-milestone-2.5.3.sh` |
| 2 | Module `adb-advisor` and the physical design view | `PhysicalDesign`, `EntityDesign` |
| 3 | Windowed workload model | `WorkloadModel`, `WorkloadSnapshot`, `ShapeDelta` |
| 4 | Recommendation types and cost-benefit scoring | `Recommendation`, `CostBenefitModel`, `Score` |
| 5 | Four detectors | `Detectors`, `DetectorConfig` |
| 6 | Hysteresis and audit log | `Hysteresis`, `AuditLog` |
| 7 | `Advisor` and `AdvisorReport` | `evaluate(design)` |
| 8 | `AdvisorScheduler` | one daemon thread, only while on |
| 9 | Size a never-analyzed table by the rows its scans read | found while wiring the gateway |
| 10 | Gateway wiring and `SET advisor` in the binder | `BoundSetAdvisor`, `BinderTest` |
| 11 | `\advice` and `\advice log` | shell |
| 12 | End-to-end smoke test | `examples/jvm-advisor-smoke` |
| 13 | Milestone script runs it | `build-milestone-2.5.3.sh` |
| 14 | Benchmark section and its check | `advisor` in the JVM report, `check-benchmarks.py` |
| 15 | Feature tour phase | `demo/sql/2.5.3-advisor.sql` |
| 16 | `docs/advisor.md` | design document |
| 17 | This note | `docs/milestone-2.5.3.md` |
| 18 | Complete the Scaladoc of the module | parameters of `Advisor`, `AdvisorScheduler.close` |
| 19 | README, CHANGELOG, roadmap, invariants 69 to 73, architecture, telemetry note, results README | documentation |
| 20 | Validation records | `VALIDATION-2.5.3.txt`, `MILESTONE-2.5.3.json` |

## Invariants

1. **The advisor never changes the database.** It has no handle to the engine, the catalog or the
   gateway; `AdaptiveDatabase.physicalDesign()` hands it copies.
2. **Evaluations never touch the query path.** They read folded counters and shape definitions, not
   the telemetry queue; the only thread is the optional timer.
3. **Off means absent.** With `SET advisor = off` (the default) there is no thread and no state
   change on any query.
4. **Advice is deterministic and stable:** a pure function of the windows and the design, held back
   by hysteresis.
5. **Advice that was carried out is withdrawn**, not repeated.

## Findings while building it

- A table that was never analyzed reports 0 rows, which made an index look free to build and any
  scan worth fixing. The cost now uses the larger of the analyzed row count and the most rows one
  execution scanned (task 9). Running `ANALYZE` first gives better advice.
- The first evaluation cannot be a window: the telemetry counters hold everything since the process
  started. It only sets the baseline, so the first advice needs one evaluation more than the
  hysteresis count.
- A shape that dominates the workload is also reported as hot, so a wasteful scan normally produces
  two lines: the index advice and the hot-query information.

## Measured

The behaviour is checked by counts, which do not depend on the machine (`check-benchmarks.py`):
workload E (an unindexed filter that reads 100 000 rows to return one) gives no advice after two
evaluations, `CREATE INDEX adv_ix_account_region ON ix_account (region)` after three, and the number
of indexes is the same before and after.

One evaluation, including building the `PhysicalDesign` from the engine for the 11 tables of the
benchmark, took about 6 ms (smallest of four, VM). The advisor runs every 10 s when on, so this is
a tick, not a per-query cost. The reference-machine figures are not in this note.

## Compatibility

- No engine, ABI, wire or data change. A 2.5.2.1 library works with 2.5.3 JVM modules except for the
  engine version string.
- New SQL: `SET advisor = on | off`. New shell commands: `\advice`, `\advice log`.
- New public JVM API: package `io.adb.advisor`; `AdaptiveDatabase.advisor`, `physicalDesign()`,
  `startAdvisor`, `stopAdvisor`, `advisorRunning`, `showAdvice`, `showAdviceLog`.
- The unknown-setting message lists `advisor`.

## Definition of done

- [x] The advisor recommends and never acts (smoke test: the index count is unchanged by it)
- [x] Advice appears only after three evaluations and leaves when applied
- [x] `CREATE INDEX` and `ANALYZE` advice carries the statement; other findings do not
- [x] Audit log of confirmed, withdrawn and applied advice
- [x] `\advice`, `\advice log`, `SET advisor` covered by BinderTest, the smoke test and the demo
- [x] Scaladoc on every public item; Rust unchanged and still strict about `missing_docs`
- [x] `check-benchmarks.py` checks the advisor section exactly
- [ ] Reference-machine run of the JVM `BenchmarkMain` (carried over from 2.5.2.1) and `compare-write-path.sh`
- [ ] Tuning of `CostBenefitModel` defaults on the reference machine's workloads

## Not verified

- Gradle (Maven Central is not reachable from the validation environment; CI runs it). The JVM
  modules were compiled with `scalac` 3.3.8 and `javac` 22 and the tests run with a JUnit stand-in.
- The quality of the advice on real workloads. The cost model is a heuristic; its defaults come from
  the 2.5.2 benchmarks and one synthetic scenario.
- The advisor under concurrent query load with `SET advisor = on` for hours; the timer, the
  evaluation and the audit log are covered by unit tests and a short smoke test only.
