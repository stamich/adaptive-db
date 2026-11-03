# Milestone 2.5.1: plan cache

2.5.1 adds a plan cache to the SQL gateway on top of 2.5.0. A repeated `SELECT` no longer goes
through parsing, binding, logical planning, optimization, physical planning and plan encoding on
every execution: the encoded physical plan is kept and sent to the engine again. The engine, the
data format (4), the C ABI (8) and the plan wire (v4) are unchanged; only the version string moved.
Behaviour, limits and measurements are in [plan-cache.md](plan-cache.md); this document records
how the proposal was implemented and adapted.

## Version numbering

The concept documents called this milestone 2.5.1 (and 2.5.3 / 2.5.3.1 in their drafts). Our rule
since 2.3.0 is that the minor number is the milestone and the patch number is for fixes only, and
the roadmap put a plan cache under 2.6+. It is numbered **2.5.1** because the author asked for
that. The next feature milestone remains 2.6 (asynchronous projections).

## Decisions taken before implementation

1. **Validity comes from the engine.** The concept used JVM-side generation counters. A counter
   inside one process cannot see DDL or `ANALYZE` done through another handle on the same database,
   so the stamp of an entry is the engine's own **schema epoch** (`currentSchemaEpoch`; every DDL,
   `CREATE INDEX` and `DROP INDEX` included, advances it) and the **statistics generation** of each
   table the plan reads (`adb_statistics_generation`).
2. **The stamp is read before planning.** If something changes while the planner runs, the entry
   is already older than the world and is refused at its first lookup.
3. **The optimizer mode is part of the key.** `SET optimizer = rule | cost` yields different
   plans for one text.
4. **The key is the token stream of the existing lexer**, not a text normalisation. Layout and
   comments are free; two different statements cannot collide.
5. **Only plans that are correct for every session are cached.** A plan that reads an
   asynchronous projection, or an index whose freshness resolution is not `Direct`, depends on the
   session's level and the projection's progress. In 2.5.x every index is synchronous, so the
   rule costs nothing today and keeps 2.6 safe.
6. **A token beyond the head is still refused on a hit.**
7. **`AS OF` is never cached**; `EXPLAIN` never reads or fills the cache; `EXPLAIN ANALYZE`
   plans afresh.
8. **Bounded memory and admission.** A byte budget (32 MiB by default) over 8 lock shards, LRU
   per shard, and storage only on the second sighting, instead of an entry count that lets one-off
   statements push out useful plans.
9. **The engine's refusal is a safety net, not the mechanism.** If the engine still reports
   `SCHEMA_CHANGED` for a cached plan, the entry is dropped and the normal path re-plans.

## Delivered, task by task

| # | Task | Result |
|---|---|---|
| 1 | Statement key | module `adb-plan-cache`, `StatementKey` from the lexer's tokens plus the mode, with a size limit |
| 2 | LRU and admission | `BoundedLru` (byte weight, shards), `SeenFilter` (second sighting) |
| 3 | PlanCache | `PlanStamp`, `PlanCache` with `lookup` / `offer` / `invalidate` / `clear`, counters |
| 4 | Gateway | hit path before parsing, stamp read before planning, cacheability rule, `SET plan_cache`, `planCacheStats`, `showPlanCache`, `clearPlanCache`, `run` split so a cached plan runs without syntax trees |
| 5 | EXPLAIN and shell | the `Plan cache:` line, `\cache`, `\cache clear` |
| 6 | Smoke test | `examples/jvm-plan-cache-smoke/run.sh` against the real library |
| 7 | Benchmark | cache off / on and key-building timings in the JVM benchmark |
| 8 | Version | workspace, Gradle, engine version string, ABI test, shell banner |
| 9 | Documentation | this document, `plan-cache.md`, changelog, roadmap, invariants 62–63, architecture, README |
| 10 | Scripts, benchmarks, demo | scripts, workflows and benchmark reports moved to 2.5.1, `build-milestone-2.5.1.sh` runs the plan-cache smoke test, demo phase 2.5.1 |

## Changes to the proposal

- No separate prepared-statement (parse) cache: parsing is 6–9 µs, small beside the planning and
  metadata cost the plan cache removes.
- No literal parameterisation: `WHERE id = 42` and `= 43` are different statements for now.
- No JVM-local generation counters and no plan-cache invalidation hook in the catalog; see
  decision 1.
- The statistics stamp covers the tables the query reads, not a database-wide counter, so
  `ANALYZE` of an unrelated table keeps the entry.
- Counters are exposed in SQL-shell form (`\cache`) and through the API rather than as a metrics
  endpoint.

## Definition of done

- JVM tests: 214 pass (201 before). New: `StatementKeyTest`, `BoundedLruTest`, `PlanCacheTest`,
  and the `plan_cache` setting in `BinderTest`.
- Rust: format, clippy with the documentation lints, 413 tests and rustdoc pass.
- `examples/jvm-plan-cache-smoke/run.sh` passes against the real library: admission, hits, no
  invalidation by DML, invalidation by ALTER, `ANALYZE`, `CREATE INDEX` and DDL from another
  handle, one entry per mode, `AS OF` not stored, refusal of a token beyond the head on a hit,
  `SET plan_cache`.
- Documentation of every new Scala item (Scaladoc).

## Measured

See [plan-cache.md](plan-cache.md#measured). On the validation VM (one noisy run each) a point
lookup by key took about 120–155 µs with the cache off and 13–14 µs on a hit, a join/aggregate
about 14 ms against 12 ms. The cache-off path costs much more than the planner stages alone
(about 23 µs), which suggests about 100 µs of per-SELECT index and projection metadata work in the
gateway; hits skip it and uncached statements still pay it. Reducing it is a follow-up.

The author's Ryzen run (`examples/results/2.5.1-*.json`) agrees: through the full SQL path the
covering-index point read (workload C) fell from 0.101 to 0.028 ms and the index range query (D)
from 0.226 to 0.123 ms, A and B by about 15 %, and the 48 ms scan (E) did not move; engine paths
stayed within 1–4 % of 2.5.0. Disk-bound figures differ widely between runs and say nothing about
the cache.

## Not verified

- The Gradle build and test run (Maven Central is not reachable from the validation VM).
- A `check-benchmarks.py` check for the plan cache: the cache benchmark of `BenchmarkMain` prints to
  the console. The Ryzen workload run (`examples/results/2.5.1-*.json`) passes the existing checks.

## Out of scope (2.6 and later)

Plans that depend on projection freshness (plan families keyed on freshness), literal
parameterisation, a prepared-statement API, cache sharing between processes.
