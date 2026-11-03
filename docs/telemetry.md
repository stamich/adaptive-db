# Query telemetry (Milestone 2.5.2)

2.5.2 lets the SQL gateway observe its own workload at almost no cost. It collects **facts**:
which shapes of query run, how often, how long they take, how many rows they return and read, and
how far the planner's estimate was from reality. It does not decide anything yet: the workload
model and the physical-design advisor are 2.5.3. Nothing below the gateway changes (C ABI 8, plan
wire v4, data format 4).

## The contract

| State | Work done by a query thread |
|---|---|
| `off` (default) | one volatile read. No clock, no counter, no allocation, and the query profile is not fetched |
| `sampled`, query skipped | two `nanoTime` calls and the sampling decision (a counter test; about 0.1 µs) |
| `sampled`, query captured | the profile is fetched and scanned (about 1 µs), a small event is built and offered to a queue |
| `full` | every query is captured (tests, demos, short diagnostic runs) |

The queue is bounded and `offer` never blocks: when it is full the event is dropped and counted
(`dropped`). Everything else (draining, aggregation) happens in a daemon worker thread.

## Pieces

All in module `adb-telemetry` unless noted.

- **`QueryShape`**: the semantic shape of a SELECT: entities read, filter columns, equality join
  edges, order columns with direction, "has aggregate", "has limit". No SQL text, no literal. Two
  statements that differ in layout, case or literals have the same shape and the same 64-bit id
  (FNV-1a over the content). `QueryShapeExtractor` (in `adb-gateway`) derives it from the bound
  statement.
- **Computed once per plan.** The shape is computed when a statement is planned and stored in the
  cached plan (`CachedQuery.shape`). A cache hit reads it; it never rebuilds it. A miss builds it
  only when telemetry is on.
- **`TelemetrySamplingPolicy`**: captures a query that failed, ran at least `slowQueryNanos`
  (default 10 ms), has a shape not yet registered, or is picked by a deterministic
  1-in-`fastSampleEvery` counter (default 128, a power of two so the test is a mask). The decision
  uses only values the gateway already has; the result is an enum singleton, so nothing is
  allocated.
- **`QueryTelemetry`**: the event: shape id, schema epoch, estimated rows, actual rows, rows read,
  elapsed nanoseconds, cache-hit and failed flags. The shape is not copied into events; the
  `QueryShapeRegistry` (bounded, default 4096 shapes) holds it once.
- **`TelemetryRuntime`**: mode, sampler, queue, registry, counters and the worker. The worker folds
  events into bounded per-shape `ShapeStats` (executions, cache hits, failures, mean and max time,
  rows returned and read, worst q-error).
- **Rows read.** `ProfileRenderer.rowsScanned` sums `rows_out` of the scan operators in the
  engine's query profile. It is a single-pass text scan, not a JSON parse (16.5 µs for a small
  profile against 1.2 µs); `rowsScannedExact` is the parsing reference the tests compare with.

## Controls

- `SET telemetry = off | sampled | full`.
- Shell: `\telemetry` (counters: mode, skipped, captured, accepted, dropped, drained, queue
  size and high watermark, shapes) and `\telemetry shapes` (busiest shapes with mean and max
  time, rows returned and read per execution and the worst q-error).
- API: `AdaptiveDatabase.telemetry` (`mode`, `policy`, `metrics`, `allStats`, `drainNow`,
  `registry`), `showTelemetry()`, `showTelemetryShapes()`.

## What is and is not recorded

- No SQL text and no literal values are kept, in events, shapes or statistics.
- Statements that fail while planning or executing a bound SELECT are captured as failed events.
  Statements that fail earlier (parse or bind errors) have no shape yet and are not recorded.
- `AS OF`, DDL, DML and `EXPLAIN` are not observed in 2.5.2; only SELECT.
- Statistics live in memory and are lost on restart. Persistence belongs to the workload model
  (2.5.3).

## A worker that sleeps

The first worker waited on the queue (`queue.poll(timeout)`). A consumer parked there has to be
woken by the producer's `offer`, an unpark paid by the **query thread**. In the benchmark this made
a captured cached point lookup about 2.6 times slower (+25 µs, more than the query itself). The
worker now drains and **sleeps** (5 ms while events arrive, 50 ms when idle), so `offer` finds
nobody to wake. The cost is that statistics lag by at most the sleep; `drainNow()` folds the queue
on demand.

## Measured

`BenchmarkMain` with a live engine in the validation VM (2 vCPU, noisy; the telemetry comparison
is the median of seven alternating rounds):

| Piece | Time |
|---|---|
| sampling policy alone | 14–18 ns |
| `TelemetryRuntime.decide` (counter, registry lookup, counters) | about 109 ns |
| shape extraction (once per plan) | 2.3–5 µs |
| cached shape id | 12–17 ns |
| event construction | 33–38 ns |
| queue offer (benchmark also reads the queue size) | 0.32–0.41 µs |
| profile scan for rows read | 1.1–1.2 µs (the JSON parse: 16.5 µs) |

Cached point lookup (about 14.5–18 µs per statement): telemetry on but skipped −4 % to +4 %
(within noise), captured +12 % to +18 % (before the worker change: +162 % to +176 %). In sampled
mode the default policy captures about 1 query in 128, so the average cost is a fraction of a
percent. Run `scripts/benchmark-ffi.sh` and the JVM benchmark on your hardware for reference
numbers; the sampler decision (109 ns) is slightly above the 100 ns target set for it.

Tests: `jvm/adb-telemetry` (shape ids, sampling rules, event q-error, registry bounds, queue
drop, aggregation, worker), `QueryShapeExtractorTest`, `ProfileRendererTest`, the `telemetry`
setting in `BinderTest`, and `examples/jvm-telemetry-smoke/run.sh` against the real engine.

## Not done (2.5.3 and later)

Workload model with time windows and persisted snapshots, detectors (hot query, scan
amplification, hot join, cardinality error), candidate generation, cost-benefit scoring, the
advisor and its audit log; any automatic action; telemetry for DML and DDL; failed events for
statements that fail before a shape exists; a cached-plan hit/miss rate per shape beyond the
`cache_hits` count.
