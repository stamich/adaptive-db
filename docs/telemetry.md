# Query telemetry (Milestone 2.5.2, capture path reworked in 2.5.2.1)

2.5.2 lets the SQL gateway observe its own workload at almost no cost. It collects **facts**:
which shapes of query run, how often, how long they take, how many rows they return and read, and
how far the planner's estimate was from reality. It does not decide anything yet: the workload
model and the physical-design advisor are 2.5.3. In 2.5.2 nothing below the gateway changed; 2.5.2.1
added one engine function, `adb_query_telemetry` (C ABI 9, additive), so that a captured query does
not need the JSON profile. Plan wire v4 and data format 4 are unchanged.

## The contract

| State | Work done by a query thread |
|---|---|
| `off` (default) | one volatile read. No clock, no counter, no allocation, and the query profile is not fetched |
| `sampled`, query skipped | two `nanoTime` calls and the sampling decision (a counter test and an allocation-free id lookup; about 0.1 µs); **0 bytes allocated** since 2.5.2.1 (2.5.2 boxed the shape id: 24 bytes) |
| `sampled`, query captured | one native call fills four integers (rows returned and read, batches, peak memory), a small event is built and offered to a queue (2.5.2: the whole JSON profile was fetched, decoded and scanned) |
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
- **Rows read** *(2.5.2.1)*. The engine counts the rows its plan's leaf operators produced and
  reports them with the other counters through `adb_query_telemetry` into a reusable native
  segment (`NativeQueryTelemetry`, one per thread), read with `NativeQuery.readTelemetry`. In 2.5.2
  the gateway fetched the whole profile document, decoded it into a `String` and scanned it
  (`ProfileRenderer.rowsScanned`, 1.1–2.7 µs and about 2 KB of allocation); the document is now
  fetched only for EXPLAIN ANALYZE and the feedback log. `rowsScanned` stays for callers that only
  have a profile; `rowsScannedExact` is the parsing reference its tests compare with.
- **Known-shape test** *(2.5.2.1)*. The sampler asks whether a shape is registered for every query.
  `ConcurrentHashMap[Long, _].containsKey` boxes the key, so the registry keeps the ids also in a
  `LongIdSet` (open addressing over an `AtomicLongArray`) that answers without touching an object.

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

All figures come from the validation VM (2 vCPU, shared, noisy). **Allocation is exact; time is
not**, and 2.5.2.1 is judged on allocation here. Time needs the reference machine.

Allocation per executed cached point lookup (bytes of the calling thread, `BenchmarkMain`, 20 000
executions per mode):

| | 2.5.2 | 2.5.2.1 |
|---|---|---|
| telemetry off | 8 640 B | 8 456 B (the baseline moves with the JIT) |
| skipped by the sampler | +24 B | **+0 B** |
| captured | +2 344 B | **+144 B** (the event) |

On the reference machine (Ryzen 9 5950X, workload C, JVM workload report) the same allocation was
measured: skipped +0 bytes, captured +144 bytes, 10 rows read per capture. The timing comparison of
the modes is not in that report.

`scripts/check-benchmarks.py` holds the workload report to a budget of 64 B (skipped) and 512 B
(captured); `examples/jvm-telemetry-smoke` checks the same end to end.

Pieces of the capture path in 2.5.2 (before the rework, nanoseconds per call, noisy):

| Piece | Time |
|---|---|
| sampling policy alone | 27–127 ns |
| `TelemetryRuntime.decide` | 86–315 ns |
| shape extraction (once per plan) | 2.0–8.6 µs |
| cached shape id | 22–52 ns |
| event construction | 44–116 ns |
| queue offer (benchmark also drains) | 0.45–0.76 µs |
| profile fetch (native JSON + copy + `String`) | 0.5–3.6 µs |
| profile scan for rows read | 1.1–2.7 µs |

The spread of each line is the noise of the machine, not a range of behaviours: two runs of the
same build differ by that much. Overhead on the cached point lookup (about 17–20 µs per
statement), fastest of nine rounds in rotating order: skipped −1.4 % to +2.3 %, captured +5 % to
+11 % (2.5.2: +12 % to +18 % by the median of seven fixed-order rounds). The medians of the same
runs moved by ten points between runs, so these percentages cannot confirm or refute a gain of a
few microseconds. Run the JVM benchmark and `scripts/benchmark-ffi.sh` on the reference machine.

An observation for later: a native call costs 300–500 ns in the benchmark
(`native_call_schema_epoch` and the others), far more than the work behind it, because
`NativeLibrary.invokeInt` goes through `MethodHandle.invokeWithArguments`, which boxes its
arguments. `adb_query_telemetry` uses `invokeExact` and allocates nothing; the other calls were not
changed in 2.5.2.1.

Tests: `jvm/adb-telemetry` (shape ids, sampling rules, event q-error, registry bounds, `LongIdSet`,
queue drop, aggregation, worker), `NativeQueryTelemetryTest` (the layout that must match
`adb.h`), `QueryShapeExtractorTest`, `ProfileRendererTest`, the `telemetry` setting in
`BinderTest`, the Rust tests of `adb_query_telemetry` (`crates/adb-ffi/tests/query_telemetry.rs`,
the cursor counters in `crates/adb-execution/tests/profile.rs`), and
`examples/jvm-telemetry-smoke/run.sh` against the real engine.

## Not done

**Considered and left out in 2.5.2.1:** a preallocated MPSC ring buffer in place of
`ArrayBlockingQueue`, thread-local counters and false-sharing padding. The measurements do not
support them: a captured query now allocates 144 bytes, the queue offer is 0.3–0.5 µs of a 17–20 µs
statement, and the first slowdown in this subsystem (2.5.2) was a parked worker, not the queue. They
are worth revisiting if the reference-machine run shows contention under many query threads.

**2.5.3 and later:**

Workload model with time windows and persisted snapshots, detectors (hot query, scan
amplification, hot join, cardinality error), candidate generation, cost-benefit scoring, the
advisor and its audit log; any automatic action; telemetry for DML and DDL; failed events for
statements that fail before a shape exists; a cached-plan hit/miss rate per shape beyond the
`cache_hits` count.
