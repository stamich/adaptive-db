# The change log and the commit order (Milestone 2.5.0)

The canonical write-ahead log (WAL) stays the source of truth. Since 2.5.0 the engine also keeps a
**change log**: a derived, ordered stream of what each commit changed, which projections and
external consumers read instead of decoding the WAL. Losing the change log never loses data; it is
rebuilt from the WAL when the database opens.

```
transactions ──► canonical WAL ──► change log ──► projections (current rows, indexes, consumers)
   (truth)         (truth)         (derived)             (derived)
```

## Identities

| Name | Meaning |
|---|---|
| `LSN` | position in the WAL (physical) |
| `CommitTs` | MVCC timestamp of a commit (what `AS OF` uses) |
| `CommitSeq` | **position in the global commit order**; data commits and catalog (DDL) commits share it; starts at 1, `0` means "before the first commit" |
| `SchemaEpoch` | catalog epoch after the commit |
| `ProjectionId`, `ProjectionEpoch` | a projection and the version of its definition |

`CommitSeq` is assigned inside the commit, under the same lock that assigns the timestamp, and is
written into the WAL record (`CommitV2`). Read-only transactions take no number: `Database::commit`
returns a `CommitResult { commit_ts, commit_seq }` that reports the head for them. Logs written by
2.4.0 (commit records without a number) are upgraded in place on open (data format 4): their
commits are numbered by timestamp and new commits continue above them. A log whose numbers do not
increase is refused as corrupt.

`Database::latest_commit_seq()` is the newest **published** commit. A commit is visible to readers
when its number is published, which happens after its change-log envelope was appended.

## What is stored

One **envelope** per commit (`ChangeEnvelope`): `commit_seq`, `commit_ts`, `schema_epoch` and a
payload, either `Data { tx_id, rows }` with a `RowChange` per changed row (entity id, row id,
schema versions and the before/after images) or `Schema { tx_id, changes }` with the schema
markers of a DDL commit. Envelopes are encoded with bincode (fixed-width little-endian integers)
and are limited to 64 MiB (`MAX_ENVELOPE_BYTES`), checked before any allocation.

Files live in `<data dir>/change-log/`. A segment is named after its first commit number, starts
with the header `ADBCLOG1` plus a format version, and holds frames `length | CRC-32 | payload`.
A segment rolls over at `DatabaseOptions::change_log_segment_bytes` (default 16 MiB).

## Durability

The envelope is **appended before the commit number is published** but **fsynced lazily**: at a
checkpoint and at close, not per commit. This keeps the per-commit cost at one buffered write.
The WAL is what makes a commit durable; after a crash the change log may be missing its tail (or,
rarely, hold envelopes of a commit that never became durable). At open the engine compares the log
with the WAL and repairs it: it truncates envelopes beyond the WAL's last commit and rebuilds the
missing tail from the WAL. A missing or damaged change log is rebuilt completely. A torn last frame
is truncated; damage in an earlier segment is an error for the store and a rebuild for the engine.

Appending is idempotent (the same envelope twice is a no-op) and strictly ordered: a number below
the head is a regression and is refused.

## Reading: cursors

`ChangeLog::cursor(after)` returns a `ChangeCursor`; `next_batch(max_commits, max_bytes)` returns
the next commits as a `ChangeBatch` (never empty) or `None` when the consumer is caught up. A batch
stops at a commit boundary once the byte budget is reached, but always holds at least one commit,
so a commit larger than the budget is still delivered whole. `cursor(after)` fails with
`Truncated` when `after` is older than the oldest retained commit (C ABI: `LOG_TRUNCATED`).
Over the C ABI the same stream is `adb_change_cursor_*` (see [ffi.md](ffi.md)); the JSON batch is
described in `include/adb.h` and `crates/adb-ffi/src/change_cursor.rs`. This is the successor of
the per-event CDC feed ([cdc.md](cdc.md)), which now also carries `commit_seq`.

## Retention

`Database::prune_change_log(&ChangeRetentionPolicy)` removes whole segments. A segment is removed
when every registered consumer has read past it. Three optional limits can remove segments that a
slow consumer still needs: `max_bytes`, `max_age_seconds` and `max_lag_commits`. A consumer that
loses commits this way is marked `NeedsRebuild` (it cannot serve reads until rebuilt) and the
report says `overtook_frontier`. The active segment is never removed. No limit is set by default.

## Projections that consume the log

The **projection runtime** (`adb-projection-runtime`) is a registry of projections, each with a
descriptor (id, name, kind, mode, entity), a state and a frontier. It is checkpointed in
`<data dir>/projection-runtime/projections.meta` (atomic replace, CRC-protected).

| Kind | Id | Mode | Maintained by |
|---|---|---|---|
| `CurrentRow` | 1 | synchronous | the commit |
| `SecondaryIndex` | 1000 + index id, named `index:<name>` | synchronous | the commit |
| `TestAsync` | ≥ 1,000,000 | asynchronous | an external consumer through `Database::feed_projection` |

States: `Building` (not readable), `Ready`, `NeedsRebuild` (the log no longer holds what it needs),
`Failed` (its consumer reported an error; it keeps its frontier and is not fed again until reset).
`Database::register_consumer_projection(name, entity)` registers (or finds) an asynchronous
projection; `feed_projection(id, (max_commits, max_bytes), apply)` hands it the next batch
(only commits already published), advances the frontier when `apply` succeeds and marks the
projection `Failed` when it does not. `ProjectionRegistry::wait_until(id, commit_seq, timeout)`
blocks until the projection is `Ready` and has consumed that commit, returning `Reached`,
`TimedOut`, `NotReadable` or `Unknown`; a wait is capped at 60 seconds. The registry also keeps a
smoothed consumption rate (`commits_per_second`, an EWMA that rejects NaN, infinity and negative
values), which the JVM uses to estimate how long a projection needs to catch up.

Everything about *when a read may use a projection* is in [consistency.md](consistency.md).

## Tests

`crates/adb-change-log/tests` (`store.rs`, `retention.rs`, `fuzz.rs`),
`crates/adb-projection-runtime/tests`, and in the engine `commit_seq.rs`, `change_log.rs`,
`change_log_recovery.rs`, `change_log_cdc.rs`, `projection_registry.rs`, `async_projection.rs`;
over the ABI `crates/adb-ffi/tests/{commit_seq,change_cursor,projection}.rs`.
