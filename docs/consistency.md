# Semantic consistency (Milestone 2.5.0)

Until 2.4.0 every structure a query could read — the current rows and the secondary indexes — was
updated inside the commit, so a query always saw every committed write. 2.5.0 does not change
that: indexes are still synchronous. What it adds is the vocabulary and the machinery that lets a
later milestone make some projections **asynchronous** without making reads wrong: a global
commit order, session tokens, consistency levels and a planner that knows how fresh each
projection is.

## Session tokens

A **token** is the commit sequence number (`CommitSeq`, see [change-log.md](change-log.md)) of the
newest commit a session made or observed. It lives in the session, not in the engine.

* After `INSERT`, `UPDATE` or `DELETE` the session's token moves to the number of that commit
  (the statement's message says `commitSeq=<n>`; `QueryResult.commitSeq` carries it).
* After DDL (`CREATE/ALTER/DROP TABLE`, `CREATE/DROP INDEX`) the token moves to the head read
  right after the statement. Under concurrency that may include a few later commits, which can only
  make the token stricter, never weaker.
* A token only moves forward. `AdaptiveSession.observe(token)` adopts a token from another
  connection or service (for example one passed along in an HTTP header).
* A new session starts at token 0. A token larger than the database's head is **refused** (the
  token comes from another database, or from a commit that did not survive a crash); waiting could
  never make it true.

Sessions are created with `AdaptiveDatabase.session()`; `AdaptiveDatabase.execute` uses one
default session. The shell has one session.

## Levels

`SET SESSION CONSISTENCY ...` chooses what a read must see:

| Level | SQL | A projection may serve the read when |
|---|---|---|
| strong | `STRONG` | it has consumed the head (everything committed when the query was planned) |
| read your writes (default) | `READ_YOUR_WRITES` | it has consumed the session's token |
| bounded staleness | `BOUNDED <n> MS` or `BOUNDED <n> COMMITS` | it trails the head by at most that much |
| eventual | `EVENTUAL` | it is readable, whatever its lag; the read never waits |

Levels only change *freshness*; isolation (snapshot/serializable) is unchanged.

**Interpretation of `BOUNDED n MS`.** The engine reports a projection's lag in commits and its
smoothed consumption rate in commits per second. The time bound is estimated as
`lag_commits / rate`. A projection that is fully caught up always satisfies it; one whose rate was
never observed cannot prove a time bound. `BOUNDED n COMMITS` compares the lag directly and needs no
estimate. The estimate is not a measurement of wall-clock age.

## Deciding per projection

`FreshnessResolver.resolve(context, projection, fallbackAvailable)` (module `adb-consistency`) is
a pure function. Its result is one of:

| Result | Meaning |
|---|---|
| `Direct` | the projection already satisfies the read |
| `WaitUntil(commit, estimatedMillis)` | wait until it has consumed `commit` (the estimate comes from the rate) |
| `AllowStale(lag)` | it is behind, but the level tolerates that |
| `Fallback(reason)` | do not use it; read the canonical rows |
| `Infeasible(reason)` | cannot be satisfied: the token is beyond the head, or there is no fallback |

Rules, in order: a projection that is not `Ready` (`Building`, `NeedsRebuild`, `Failed`) is never
read. A token beyond the head is infeasible (except for eventual reads, which ignore the token).
Eventual reads accept the projection as it is. Bounded reads accept it within the bound, otherwise
fall back or wait. Strong and read-your-writes reads need the projection at the required commit:
direct when it is there; a wait when the estimated time fits the session's wait limit (default 2 s,
at most 60 s); otherwise a fallback; with an unknown rate and a fallback available, the fallback.

## In the planner and the gateway

For a `SELECT`, the gateway (`AdaptiveDatabase.planFresh`):

1. builds the read context (level, token, head, wait limit) from the session;
2. asks the engine for the projection reports and resolves every index
   (`SemanticFreshnessPlanner.resolveIndexes`); an index without a report is treated as a fresh
   synchronous index, as in 2.4.0;
3. plans with the indexes that were not turned away;
4. for the indexes the chosen plan reads, waits for those with a `WaitUntil` decision
   (`adb_projection_wait_until`, outside the database lock);
5. if a wait times out, plans again without that index, so the read falls back to the canonical row
   path. A lagging projection therefore delays a read at most once, by the session's wait limit.

A sequential scan reads the canonical rows and is always allowed. A transaction that has
uncommitted writes must read its own writes from the canonical row path: projections contain only
committed data and cannot serve such a read. (Nothing in 2.5.0 can violate this, because indexes
are still maintained inside the commit; the rule is recorded for the asynchronous projections of
later milestones.)

`EXPLAIN` ends with a **Read context** section: level, token, head, wait limit, and per index the
decision with the projection's state, frontier and lag.

```
Read context:
- consistency=READ_YOUR_WRITES token=2679 head=2679 max wait 2000 ms
- index idx_ledger_account: usable (projection index:idx_ledger_account Ready, at 2679, lag 0)
```

## Shell

| Command | Shows |
|---|---|
| `\session` | the session's level, token, head and wait limit |
| `\projections` | every projection: kind, mode, state, frontier, head, lag, rate |
| `\changes [after] [max]` | commits of the change log after commit `after` (default 0, at most `max`, default 20) |

## Not in 2.5.0

Asynchronous index maintenance, `DELTA_REPAIR`, delta caches, coalesced row deltas, read leases,
adaptive plan families and a plan cache are 2.6+ topics. 2.5.0 supplies what they need: the commit
order, the change log, the projection registry, and the freshness decision.

## Tests

`jvm/adb-consistency` (`ConsistencyModelTest`, `FreshnessResolverTest`, `ProjectionJsonCodecTest`),
`jvm/adb-optimizer` (`SemanticFreshnessPlannerTest`), `jvm/adb-gateway` (`AdaptiveSessionTest`,
`FreshnessAcceptanceTest`, `ConsistencyReportTest`), `jvm/adb-sql` (`parsesSessionConsistency`),
`jvm/adb-logical-plan` (`ConsistencyBinderTest`), and the end-to-end
`examples/jvm-consistency-smoke/run.sh`, which drives the real engine through SQL.
