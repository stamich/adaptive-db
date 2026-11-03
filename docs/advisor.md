# The workload advisor (2.5.3)

The advisor reads what telemetry (2.5.2) captured, finds problems the physical design could solve,
and **says so**. It runs no statement, creates no index and changes no setting. A person (or, later,
an actuator with its own safeguards) carries the advice out.

```
telemetry counters ──► WorkloadModel ──► Detectors ──► Hysteresis ──► AdvisorReport
 (immutable stats)     (time windows)   (4 kinds)     (3 in / 3 out)   \advice
                                            ▲                              │
              PhysicalDesign (names, rows, statistics, indexes) ───────────┘
                                                                     AuditLog  \advice log
```

Module `jvm/adb-advisor`, package `io.adb.advisor`. It depends on the model, statistics, catalog and
telemetry modules only; the gateway wires it to a database.

## Rules the advisor follows

1. **It only recommends.** Nothing in the module can change the database: it has no reference to the
   native handle, the catalog or the gateway.
2. **It reads immutable values only.** Per-shape counters (`ShapeStats`), shape definitions
   (`QueryShape`) and a `PhysicalDesign` the caller builds. It never reads the telemetry queue or a
   live structure, so an evaluation cannot slow a query or race with one.
3. **It is off by default and costs nothing while off.** `SET advisor = on` starts one daemon
   thread; `off` stops it. Without it, `\advice` evaluates once per call, on the caller's thread.
4. **It needs telemetry.** The advisor turns nothing on. With `SET telemetry = off` it sees no queries
   and `\advice` says so.
5. **Advice is stable.** A finding appears only after three evaluations in a row and leaves only after
   three misses in a row, so a workload that wobbles around a threshold does not make it flicker.

## The workload model

`WorkloadModel` keeps a bounded ring of **windows** (default 60). Each evaluation closes one window
by subtracting the previous cumulative counters from the current ones, so a window describes only
its own interval and a workload that changed an hour ago stops looking hot.

- The first evaluation only sets the baseline (the counters hold everything since the process
  started, which is not one interval).
- A counter that went backwards (telemetry was reset) is read as a fresh start.
- Idle shapes are absent from a window.
- `WorkloadSnapshot` is the immutable view the detectors get: the retained windows and per-shape
  totals (`ShapeDelta`: executions, time, rows returned, rows read, worst estimate error).

## Detectors

| Detector | Finds | Result |
|---|---|---|
| `hot-query` | a shape that takes at least 20 % of the workload's time | information |
| `scan-amplification` | a single-table shape reading at least 10 rows per row returned, filtering on a column that leads no index | **`CREATE INDEX`** if the cost-benefit ratio clears the bar |
| `hot-join` | a joining shape that takes at least 20 % of the time | information (names the join columns without a leading index; a join projection would be the next step) |
| `cardinality-error` | a shape whose estimate was off by 10x or more | **`ANALYZE`** if the table was never analyzed or more than 20 % of its rows changed since; information otherwise |

A shape needs at least 20 captured executions before it counts as evidence. Thresholds are
`DetectorConfig`.

Only `CREATE INDEX` and `ANALYZE` map to statements the engine has. Join-projection and partition
candidates stay informational until their actuators exist (2.6 and later).

For scan amplification, evidence from several shapes that want the same index is added up before the
cost-benefit gate. The candidate is an index on the shape's first unindexed filter column; if any
filter column of the shape already leads an index the shape counts as served.

## Cost-benefit scoring

A heuristic, not a measurement. `CostBenefitModel` (all parameters are constructor arguments):

- **Benefit** of an index for one shape: `totalNanos × (rowsRead − rowsReturned) / rowsRead`, scaled
  from the observed span to the horizon (default one hour). That is the time the shape spent reading
  rows it did not need, if it keeps running at the rate seen.
- **Cost**: `rows × buildNanosPerRow` (default 2 µs) to build the index, times `1 + writePenalty`
  (default 0.5) for keeping it up to date. `rows` is the analyzed row count, or, for a table never
  analyzed, the most rows one execution scanned.
- A candidate is recommended when `benefit / cost ≥ minRatio` (default 2).

The figures only rank candidates and gate them. They are tuned on the 2.5.2 benchmarks and are meant
to be changed per deployment (`AdvisorConfig.model`). Projecting a short observation to an hour
over-promises for bursts; that is why a finding must hold for three evaluations.

## Hysteresis

`Hysteresis(confirmAfter = 3, withdrawAfter = 3)`:

- a finding made in `confirmAfter` evaluations in a row is **confirmed** (shown);
- a candidate missed once before that starts over;
- a shown finding missed `withdrawAfter` times in a row is **withdrawn**;
- a `CREATE INDEX` advice whose index now exists is withdrawn at once as **applied**.

The shown text follows the latest evaluation (the ratio is refreshed), but only state changes are
logged.

## The audit log

`AuditLog` is a bounded (default 200), sequence-numbered record of every confirmed, withdrawn and
applied recommendation, with the SQL it carried. Sequence numbers are never reused, so a reader can
tell that entries were dropped. `\advice log` prints it.

## SQL and shell

```sql
SET telemetry = sampled;   -- the advisor reads telemetry
SET advisor = on;          -- evaluate every 10 s on a daemon thread
SET advisor = off;
```

```
\advice        kind | advice | sql   — what is recommended now
\advice log    seq | event | advice | sql   — confirmed / withdrawn / applied, oldest first
```

With the timer off, every `\advice` is one evaluation (so it closes one window and counts one step of
hysteresis). With the timer on it shows the latest evaluation.

## Programming interface

```scala
val db = AdaptiveDatabase.open(native)
db.execute("SET telemetry = sampled")
// ... run queries ...
val report = db.advisor.evaluate(db.physicalDesign())   // AdvisorReport
report.recommendations.foreach(r => println(r.summary + r.sql.fold("")(" -> " + _)))
db.advisor.auditLog.entries
```

`AdvisorConfig` sets the number of windows, the hysteresis counts, the audit capacity, the
`CostBenefitModel` and the `DetectorConfig`. `Advisor` takes its counters and shape lookup as
functions, so it can be driven by a scripted workload (the unit tests do).

## What is measured and checked

- 42 unit tests: the model's windows and resets, scoring, each detector, hysteresis and the audit
  log, the advisor over a scripted counter stream, the scheduler.
- `examples/jvm-advisor-smoke` runs the whole path against the real engine: off by default,
  hysteresis over three windows, the index advice for a wasteful scan, no index created, the advice
  leaving once the index exists, the audit log, `SET advisor`, and an unchanged result throughout.
- The JVM workload report has an `advisor` section (workload E: an unindexed filter that reads every
  row to return one). `scripts/check-benchmarks.py` checks it exactly: no advice after two
  evaluations, an index on `ix_account(region)` after three, an unchanged index count, a non-empty
  audit log. These are counts, so they do not depend on the machine.
- `advisor_evaluate_50_shapes` times one evaluation over 50 shapes on 5 tables, for information.

## Limits

- Evidence is **sampled** telemetry: in `sampled` mode a known, fast query is mostly skipped, so
  executions are undercounted; the benefit is a floor for the share of time and an underestimate of
  the rate. `full` mode gives the exact picture at a cost.
- Writes are not observed (telemetry covers SELECT), so the write cost of an index is the flat
  `writePenalty` and not a measurement.
- An index candidate is a single leading column. Composite and covering candidates, and dropping
  unused indexes, are not generated.
- Statistics and index definitions are read when `evaluate` runs; between evaluations the advice can
  describe a design that has changed. The `applied` check covers the common case.
- State lives in memory: recommendations and the audit log do not survive a restart.
