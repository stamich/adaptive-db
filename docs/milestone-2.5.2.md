# Milestone 2.5.2: telemetry hot path

2.5.2 adds the observation half of an adaptive loop to the SQL gateway: a sampled, non-blocking
telemetry pipeline that records the semantic shape and the cost of the queries that run. It adds
no decisions and no automatic changes. The advisor that reads these facts is 2.5.3. The engine,
the data format (4), the C ABI (8) and the plan wire (v4) are unchanged. Behaviour and measurements
are in [telemetry.md](telemetry.md); this document records how the proposal was implemented and
adapted.

## Version numbering

The concept documents called this stage 2.5.4 (telemetry and advisor) and 2.5.4.1 (hot-path
hardening) in a different lineage. In ours it is **2.5.2**, with the advisor split off as **2.5.3**.
As for 2.5.1: our rule keeps patch numbers for fixes only, and the roadmap placed this under later
milestones; the numbering follows the author's request.

## Why this is a milestone of its own

The concept built telemetry and advisor together, then needed a hardening release because
its first version extracted the query shape before deciding to sample and slowed the 13 µs cached
query to 27 µs. 2.5.2 builds the cheap path first and measures it, so that the advisor
(2.5.3) is built on a pipeline whose cost is known.

## Decisions taken before implementation

1. **Sample first.** The sampler decides from values at hand (failure, elapsed time, whether the
   shape is registered, a counter). The shape, the event and the profile are touched only for
   captured queries.
2. **The shape is a plan-time artefact.** It is computed when a statement is planned and stored in
   the cached plan, so the execution path of a cached statement reads it.
3. **Events are compact.** A shape id and a few numbers; the shape lives once in a bounded
   registry.
4. **Never block.** `ArrayBlockingQueue.offer`; a full queue drops and counts.
5. **Off means off.** One volatile read. The query profile, which 2.5.1 fetched on every
   execution even with no feedback log, is now fetched only when telemetry captures the query, a
   feedback log needs it, or EXPLAIN ANALYZE asks.
6. **No external dependencies.** Plain Scala and `java.util.concurrent` (no Cats Effect, no actor
   system): Gradle cannot resolve new dependencies in our environment and the queue is enough.
7. **Absolute budgets, not percentages.** On a 14 µs query 1 % is 140 ns, below this machine's
   noise, so the pieces are timed separately in nanoseconds and the full path is compared as a
   median over alternating rounds.
8. **No literals, no SQL text** in anything telemetry keeps.

## Delivered, task by task

| # | Task | Result |
|---|---|---|
| 1 | Baseline | native calls of a cache hit timed (about 0.3 µs each) |
| 2 | adb-telemetry model | `QueryShape`, `SamplingDecision`, `TelemetryMode`, `TelemetrySamplingPolicy`, `QueryTelemetry`, `QueryShapeRegistry` |
| 3 | Runtime | `TelemetrySink`, `TelemetryRuntime` (queue, counters, worker, aggregation), `TelemetryMetrics`, `ShapeStats` |
| 4 | Shape at plan time | `QueryShapeExtractor`, `CachedQuery.shape`, `ProfileRenderer.rowsScanned` |
| 5 | Gateway | sample-first hot path, conditional profile, failed events, `SET telemetry`, `\telemetry` |
| 6 | Smoke test | `examples/jvm-telemetry-smoke/run.sh` against the real engine |
| 7 | Profile scan | rows read with a scanner (16.5 µs → 1.2 µs) |
| 8 | Worker | sleeps between drains; removes the unpark cost from the query thread |
| 9 | Benchmark | parts of the hot path and the off/skip/capture comparison |
| 10 | Version | workspace, Gradle, engine version string, ABI test, shell banner |
| 11 | Scripts | scripts, workflows and reports at 2.5.2; telemetry smoke in the validation script |
| 12 | Demo | telemetry phase in the feature tour |
| 13 | Documentation | this document, `telemetry.md`, README, changelog, roadmap, invariants, architecture |

## What the measurements changed

- The first capture path made a cached point lookup 2.6 times slower. The cause was not the work
  of building the event but waking the worker on every `offer`; found by removing parts one at a
  time (no profile fetch: still slow; no worker wake-ups: +12 %). Task 8 is that fix.
- Parsing the profile as JSON cost more than the query (16.5 µs); the scanner is task 7.
- The skipped path is indistinguishable from telemetry off at this resolution (−4 % to +4 %).

## Changes to the proposal

- No `AUTO_SAFE` mode and no adaptive mode enum yet: there is no advisor to configure. They come
  with 2.5.3.
- No separate "telemetry plan key" or registry bound to the plan cache's lifecycle: the shape is a
  function of the statement and the schema; it lives in the cached plan and in the shape registry,
  and a registry that is full simply stops adding shapes.
- No commit sequence in events (it costs a native call per query); the schema epoch is enough
  to relate an event to a plan.
- Failure events cover statements that fail while a bound SELECT is planned or executed.

## Definition of done

- JVM tests: 229 pass (214 before). New: `TelemetryModelTest`, `TelemetryRuntimeTest`,
  `QueryShapeExtractorTest`, scanner tests in `ProfileRendererTest`, the setting in `BinderTest`.
- Rust: format, clippy with the documentation lints and the 413 tests pass; rustdoc builds.
- `examples/jvm-telemetry-smoke/run.sh` and `examples/jvm-plan-cache-smoke/run.sh` pass against the
  real library.
- Documentation (Scaladoc) on every new Scala item.

## Not verified

- The Gradle build and test run (Maven Central is not reachable from the validation VM).
- A Ryzen run of the 2.5.2 benchmarks; the figures above come from the validation VM.
- `scripts/check-benchmarks.py` has no check for telemetry; the telemetry benchmark prints to the
  console.
