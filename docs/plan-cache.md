# Plan cache (Milestone 2.5.1)

The SQL gateway keeps the physical plan of a `SELECT` and reuses it when the same statement
comes back and nothing the plan depends on has changed. A hit skips parsing, binding, logical
planning, optimization, physical planning and plan encoding; it still runs in the engine, so a
cached query returns exactly what a freshly planned one would.

## What is cached

Only `SELECT` statements. The entry is the encoded plan JSON, the result columns `(name, slot)`,
the optimizer mode and the per-operator estimates (for the feedback log) — no syntax trees.

A plan is **not** cached when
- it uses `AS OF VERSION` (the snapshot is part of the statement's meaning),
- it reads an index whose projection is asynchronous, not `Ready`, or whose freshness resolution
  is not `Direct` (such a plan depends on the session's level and the projection's progress).
  In 2.5.x every index is synchronous, so this is a guard for 2.6,
- the statement does not lex, is empty, or has more than 2048 tokens,
- the statement does not begin with `SELECT` (a leading comment opts a statement out).

`EXPLAIN` never reads or fills the cache and `EXPLAIN ANALYZE` always plans afresh. EXPLAIN ends
with a `Plan cache:` line saying whether the SELECT would be cached and showing the counters.

## The key

The key is the statement's **token stream** from the SQL lexer plus the optimizer mode
(`SET optimizer`). Layout and comments do not matter; literals and keyword case do (a different
literal is a different plan; literal parameterisation is out of scope). Two different statements can
never share a key; the worst case is two spellings of one statement occupying two slots.

## Validity

Every entry carries a stamp read from the engine **before** the plan was made:
- the catalog **schema epoch** (every DDL advances it, `CREATE INDEX` and `DROP INDEX` included), and
- the **statistics generation** of each table the query reads (`ANALYZE` from any client advances
  it).

On every lookup the stamp is compared with the engine's current values (one native call for the
epoch plus one per table read). A mismatch drops the entry and the statement is planned again.
Because the stamp is read before planning, DDL or `ANALYZE` that lands *while* the planner runs
leaves the entry older than the world, so it is refused at its first lookup.

DML does not invalidate: a plan is about shapes and statistics, and statistics change only on
`ANALYZE`. If the engine itself refuses a plan as outdated (`SCHEMA_CHANGED`, DDL from another
client between the check and the run) the entry is dropped and the normal path re-plans.

Generations come from the engine, not from JVM counters, so DDL or `ANALYZE` through another
handle on the same database invalidates this handle's entries too (the smoke test checks it).

Consistency: a hit still refuses a session token beyond the database's head (the normal path
reports it). Cacheable plans read canonical rows and synchronous ready indexes only, which are at
the head, so they are correct for every session level.

## Memory and admission

- Budget: `planCacheBytes` (default 32 MiB), approximate bytes, split over 8 lock shards, LRU per
  shard. An entry larger than a shard's share is rejected rather than flushing the shard.
- Admission: a statement is stored on its **second** sighting (a bounded memory of 4096 recent
  key hashes), so one-off statements do not fill the cache.

## Controls

- `SET plan_cache = on | off` (default on; off for a budget of 0).
- Shell: `\cache` lists the counters (hits, misses, stale, inserts, deferred, rejected, evictions,
  entries, bytes, hit ratio); `\cache clear` empties the cache.
- API: `AdaptiveDatabase.planCacheStats`, `showPlanCache()`, `clearPlanCache()`.

## Not done

- A separate prepared-statement cache for the parser: parsing is ~6–9 µs, small next to the
  cost the plan cache removes.
- Literal parameterisation (`WHERE id = 42` and `= 43` share no entry).
- Caching plans that read asynchronous projections (2.6, with plan families keyed on freshness).
- Changing the engine: the plan wire, the C ABI (8) and the data format (4) are unchanged.

## Measured

`BenchmarkMain` with a live engine (sandbox, one run each, noisy; repeat on your hardware):
point lookup by primary key about 120–155 µs per statement with the cache off (the 2.5.0 path)
and 13–14 µs with a hit; the join/aggregate query 13.6–14.5 ms off and 11.8–12.5 ms on, because the
engine dominates. The cache-off figure is far above the ~23 µs the planner stages add up to, which
points to roughly 100 µs of per-SELECT gateway overhead besides planning (the index and projection
metadata fetched and decoded for every SELECT); a hit skips it, an uncached statement still pays it.
Building the key costs about 0.8 µs for the point lookup and 2.5 µs for the relational query.

**Author's run on the Ryzen** (`examples/results/2.5.1-*.json`, summary in `examples/results/README.md`): the full SQL path of workload C fell from 0.101 to 0.028 ms, D from 0.226 to 0.123 ms, A and B by about 15 %, E unchanged; engine paths stayed within 1–4 % of 2.5.0.

Tests: `jvm/adb-plan-cache` (key, LRU, admission, validation), `BinderTest` (the setting), and
`examples/jvm-plan-cache-smoke/run.sh` against the real engine.
