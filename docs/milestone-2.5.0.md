# Milestone 2.5.0: semantic consistency and the change log

2.5.0 implements Milestone 2.5 on top of 2.4.0. It adds a global commit order, a durable change
log derived from the canonical log, a registry of projections with their freshness, and the
session-level consistency vocabulary (tokens, levels, `SET SESSION CONSISTENCY`) with a planner
that honours it. Indexes are still maintained inside the commit; asynchronous maintenance of
the real projections is the next step. This document records how the proposal was implemented and
adapted. The design is described in [change-log.md](change-log.md) and
[consistency.md](consistency.md).

## Version numbering

Earlier proposals numbered this milestone 2.5, 2.5.1 and 2.5.2. From 2.3.0 the minor number is the
milestone and the patch number is for fixes only, so the milestone is **2.5.0**.

## Decisions taken before implementation

1. **The canonical log stays the source of truth.** The change log is derived from it and
   projections consume the change log. Nothing that matters for durability moved.
2. **One commit order.** Data and catalog commits share `CommitSeq`, assigned with the commit
   timestamp and stored in the WAL record (data format 4). It is the unit sessions, cursors and
   projections use.
3. **The change log is appended before the sequence number is published but fsynced lazily** (at
   checkpoint and close). The WAL makes a commit durable; open repairs a missing or surplus change
   log tail from the WAL.
4. **`Database::commit` returns a `CommitResult`** (timestamp and sequence number) so callers —
   and, over the ABI, the `*_commit` mutation functions — can build tokens without a second call.
5. **Staleness bounds are expressed in SQL, in two units.** `BOUNDED n COMMITS` compares the lag
   directly. `BOUNDED n MS` is estimated from the lag and the projection's observed consumption
   rate; a projection with no observed rate cannot prove a time bound.
6. **A token beyond the head is an error** instead of a silent read of old data.
7. **A missing projection report means "fresh synchronous index"**, so planning against an
   engine that reports nothing behaves exactly as in 2.4.0.

## Delivered, task by task

Each task is one commit on the 2.5.0 branch, with its tests and documentation comments.

| # | Phase | Task | Main pieces |
|---|---|---|---|
| 1 | A commit order | Identities | `CommitSeq`, `CommitResult`, `ProjectionId`, `ProjectionEpoch` |
| 2 | A | Commit coordinator | timestamps and sequence numbers allocated together |
| 3 | A | WAL | `CommitV2` with sequence number and schema epoch; data format 4 |
| 4 | A | Engine | `latest_commit_seq`, one order for data and catalog commits |
| 5 | A | Recovery | a non-increasing sequence is corruption; restart tests |
| 6 | B change log | Model | `adb-change-log`: `ChangeEnvelope`, `RowChange`, codec, 64 MiB cap |
| 7 | B | Store | segments, CRC frames, rollover, torn-tail recovery |
| 8 | B | Append | idempotent append, regression and mismatch detection |
| 9 | B | Cursor | `ChangeCursor`, commit and byte budgets, truncation error |
| 10 | B | Retention | frontier, size/age/lag limits, `PruneReport` |
| 11 | B | Engine append | one envelope per commit before publishing |
| 12 | B | Recovery | rebuild the tail (or all) from the WAL at open |
| 13 | B | CDC | `commit_seq` in change events; agreement test |
| 14 | C projection runtime | Model | `adb-projection-runtime`: descriptors, kinds, modes, states, frontiers |
| 15 | C | Checkpoints | `projections.meta`, atomic and CRC-protected |
| 16 | C | Registry | monotonic frontiers, lag, smoothed rate |
| 17 | C | Waiting | `wait_until`, state changes, overtaken history, retention frontier |
| 18 | C | Synchronous projections | current-row store and indexes registered |
| 19 | C | Asynchronous projections | `register_consumer_projection`, `feed_projection` |
| 20 | D boundary | C ABI 8 | `adb_latest_commit_seq`, `*_commit` mutations |
| 21 | D | Cursors over the ABI | `adb_change_cursor_*` |
| 22 | D | Projections over the ABI | `adb_projection_*` |
| 23 | D | Java FFM | commit results, cursors, projections |
| 24 | E semantic consistency | Model | `adb-consistency`: token, levels, read context, projection model and codec |
| 25 | E | Resolver | `FreshnessResolver` |
| 26 | E | Planner | `SemanticFreshnessPlanner` filters index access paths |
| 27 | E | Sessions | `AdaptiveSession`, tokens from DML and DDL, waits and fallbacks |
| 28 | E | SQL | `SET SESSION CONSISTENCY` |
| 29 | E | Explain and shell | EXPLAIN read context, `\session`, `\projections`, `\changes` |
| 30 | F hardening | Hardening | fuzz tests, [security-review-2.5.0.md](security-review-2.5.0.md) |
| 31 | F | Acceptance | scripted scenarios and the end-to-end smoke |
| 32 | F | Benchmarks | workloads I–O and their checks |
| 33 | F | Demo | tour phase 2.5.0 |
| 34 | F | Documentation and version | this document, version 2.5.0, `build-milestone-2.5.0.sh` |

## Changes to the proposal

1. **Fsync policy.** The change log is not fsynced per commit (see decision 3); the proposal's
   per-commit sync would have doubled commit latency for data the WAL already protects.
2. **Staleness in SQL.** The proposal named a bounded-staleness level without a syntax; it is
   `BOUNDED n MS | n COMMITS` (decision 5).
3. **Wait outside the lock.** The Java binding waits for a projection under the read side of the
   lifecycle lock only, so writers (which make the projection progress) are never blocked by a
   waiting reader.
4. **One defect found by the end-to-end smoke:** the gateway read the row changes of a data
   commit under the wrong key of the change-cursor JSON (`changes` instead of `rows`). The unit
   test had used the same wrong shape; the smoke against the real library caught it, and both
   were fixed.
5. **The 35th step** of the plan — benchmark results on the reference machine — is produced by
   running the benchmark scripts on that machine and is not part of this change.

## Definition of done

| Item | Proven by |
|---|---|
| every commit has a number; DDL and DML share one order | `commit_seq::catalog_and_data_commits_share_one_order`, `FreshnessAcceptanceTest::dmlAndDdlShareOneOrder` |
| numbering survives restarts; 2.4.0 logs upgrade in place | `commit_seq::numbering_survives_restarts`, `::logs_without_sequence_numbers_are_numbered_by_timestamp` |
| a bad sequence is corruption | `commit_seq::a_repeated_sequence_number_is_corruption` |
| the change log is complete, ordered and idempotent | `change-log store::*`, `change_log::*` |
| a lost or damaged change log is rebuilt from the WAL | `change_log_recovery::*`, `change-log fuzz::*` |
| CDC and the change log agree | `change_log_cdc::*` |
| retention never removes what a consumer needs unless a limit says so | `retention::*`, benchmark O |
| projections: frontier, lag, wait, rate, checkpoint | `projection-runtime registry::*`, `wait::*`, `checkpoint::*` |
| an asynchronous projection catches up, follows, restarts, fails, is overtaken | `async_projection::*` |
| the ABI exposes all of it with bounded inputs | `ffi commit_seq::*`, `change_cursor::*`, `projection::*`, `CdcSmoke` |
| the resolver's rules | `FreshnessResolverTest` |
| the planner leaves out what a session may not read | `SemanticFreshnessPlannerTest` |
| sessions read their own writes; sessions are independent; tokens from the future are refused | `FreshnessAcceptanceTest`, `ConsistencySmoke` |
| `SET SESSION CONSISTENCY` parses, binds and round-trips | `SqlParserTest::parsesSessionConsistency`, `ConsistencyBinderTest` |
| EXPLAIN and the shell report the read context | `ConsistencyReportTest`, `ConsistencySmoke` |
| the benchmark reports agree | `scripts/check-benchmarks.py` (section `consistency`) |

## Supported SQL (2.5.0 additions)

```sql
SET SESSION CONSISTENCY STRONG;
SET SESSION CONSISTENCY READ_YOUR_WRITES;      -- the default
SET SESSION CONSISTENCY BOUNDED 250 MS;
SET SESSION CONSISTENCY BOUNDED 3 COMMITS;
SET SESSION CONSISTENCY EVENTUAL;
```

DML results say `commitSeq=<n>`, and `EXPLAIN` ends with a Read context section. Everything from
[milestone-2.4.0.md](milestone-2.4.0.md#supported-sql-240-additions) still applies.

## Out of scope (2.6 and later)

`DELTA_REPAIR`, delta caches, coalesced row deltas, read leases, real columnar, graph, search and
vector projections, adaptive plan families, a plan cache, and asynchronous index maintenance.
