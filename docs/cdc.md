# Native change data capture (CDC)

The change feed is the canonical log read forward. There is no outbox table, no WAL decoding
plugin and no second copy of the data: what the engine replays after a crash is exactly what a
consumer receives.

## Model

* **Event** = one committed transaction: `tx_id`, `commit_ts`, `commit_lsn` (unique, increasing),
  `cursor_after`, and its row changes in row order.
* **Row change** = `row_id` (`entity_id`, `primary_key`), `kind` (`insert` / `update` / `delete`),
  `before` and `after` images. Images come from the log itself (`Version` before-images and
  `Put` after-images), so no extra reads are needed. Deleting a row that did not exist produces no
  change.
* **Cursor** = a transaction boundary in the log. `ChangeCursor::BEGINNING` (0) is the start;
  `change_feed_end()` is the current durable end ("only new changes").

## Guarantees

* **Commit order.** Events arrive in commit order.
* **Durable only.** Only the durable prefix of the log is visible, so a consumer never sees a commit
  that a crash could take back.
* **Gap-free, duplicate-free paging.** `read_changes(cursor)` returns `next`; reading from `next`
  continues exactly after the last examined transaction (including transactions filtered out).
  `cursor_after` of each event allows per-event acknowledgement.
* **Exactly-once processing** follows when a consumer stores its cursor atomically with its own
  side effects, or uses durable named offsets and idempotent processing keyed by `commit_lsn`.
* **Survives restarts.** The feed *is* the log, which is never pruned in 2.0.3.

## API

Rust:

```rust
use adb_engine::{ChangeCursor, ChangeFilter, Database};

let mut cursor = db.consumer_offset("search-indexer").unwrap_or(ChangeCursor::BEGINNING);
loop {
    let batch = db.read_changes(cursor, 1_000, &ChangeFilter::entities([orders_entity]))?;
    if batch.events.is_empty() { break; }
    for event in &batch.events {
        for change in &event.changes {
            // change.kind, change.before, change.after, change.primary_key()
        }
    }
    db.commit_consumer_offset("search-indexer", batch.next)?;
    cursor = batch.next;
}
```

C ABI (`include/adb.h`): `adb_read_changes_json`, `adb_change_feed_end`,
`adb_commit_consumer_offset`, `adb_consumer_offset`. The JSON document:

```json
{"next_cursor":"8589934720",
 "events":[{"tx_id":5,"commit_ts":5,"commit_lsn":"420","cursor_after":"512",
            "changes":[{"entity_id":1,"primary_key":"7","kind":"update",
                        "before":{"fields":{"1":{"Int64":10}}},
                        "after":{"fields":{"1":{"Int64":20}}}}]}]}
```

Log positions and primary keys are decimal strings because they use the full `u64` range.

Java (`io.adb.ffm.NativeDatabase`): `readChangesJson(cursor, maxEvents, entityIds...)`,
`changeFeedEnd()`, `commitConsumerOffset(name, cursor)`, `consumerOffset(name)`. Cursors are
unsigned longs (`Long.parseUnsignedLong`).

## Errors

| Condition | Rust | C status |
|---|---|---|
| Cursor past the durable end, or not at a transaction boundary | `InvalidArgument` | `ADB_INVALID_ARGUMENT` |
| Cursor older than the retained log (only possible for logs pruned by 2.0.2) | `ChangeLogTruncated` | `ADB_LOG_TRUNCATED` |
| Unknown consumer | `consumer_offset` returns `None` | `ADB_NOT_FOUND` |

## Limits

* `max_events` per call: 1..=10 000 over the C ABI; the JSON response is cut at an event boundary
  above 32 MiB (the returned `next_cursor` accounts for it).
* Consumer names: 1..=256 bytes. Offsets live in `cdc/offsets.meta` (checksummed, atomically
  replaced).

## Not yet

* Push/long-poll subscriptions (consumers poll).
* Log retention driven by consumer offsets, and archival of old segments.
* Schema/column names in events (the feed carries field ids; the catalog maps them).
