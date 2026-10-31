# Secondary indexes (Milestone 2.3.0)

A secondary index is one more **derived projection of the canonical log**, next to the current
store and the version store. The log stays the only source of truth. An index writes no log
records of its own. It is maintained from the same committed mutations, published by the same
checkpoint journal, checked by `Database::verify` and rebuilt by
`Database::rebuild_projections`. If its files are lost, nothing is lost that the log cannot
restore.

```text
commit ─► WAL (rows only) ─► current store ─┬─► version store (before-images)
                                            └─► index projections (entries with begin/end ts)
checkpoint ─► journal publishes current + versions + indexes + indexes/catalog.meta together
```

## SQL

```sql
CREATE INDEX idx_balance ON account (balance DESC, owner) INCLUDE (note);
SHOW INDEXES [ON account];        -- name, table, key columns, covered columns, serving range
DROP INDEX idx_balance;
```

Names are case-insensitive and unique across the database. Key columns are ordered, most
significant first, and each can be `ASC` (the default) or `DESC`. `INCLUDE` lists covered
columns: a scan returns them from the index entry without reading the row. Over the C ABI the
same operations are `adb_create_index_json`, `adb_drop_index` and `adb_list_indexes_json`
([ffi.md](ffi.md)).

## Limits

| Limit | Value |
|---|---|
| Key columns per index | 16 |
| Covered (`INCLUDE`) columns per index | 32 |
| Encoded key of one entry | 512 bytes (a row whose key is longer is refused by the commit) |
| Encoded covered values of one entry | 1,024 bytes |
| Index name | 128 bytes, `[A-Za-z_][A-Za-z0-9_]*` |
| Indexes per database / per table | 256 / 32 |

A column can be a key or a covered column, not both, and appears only once. The primary key
needs no index: rows are stored in primary-key order.

## Entries and key encoding

Every row version an index covers is one tree entry:

```text
tree key   = enc(key columns) | row id (16 bytes, big-endian) | begin_ts (8 bytes, big-endian)
tree value = end_ts (8 bytes, big-endian; u64::MAX while current) | enc(covered values, ascending)
```

`enc` is the order-preserving encoding of `adb_core::key_encoding`. Comparing encoded keys
bytewise gives the SQL order of the values. Each value starts with a type tag: Bool `0x10`,
Int64 `0x20` (sign bit flipped, big-endian), Float64 `0x30` (total order, `-0.0` = `0.0`, one
canonical NaN), String `0x40` and Bytes `0x50`. Strings and bytes escape `0x00` as `0x00 0xFF`
and end with `0x00 0x01`. NULL is `0xF0`, so NULLs sort last. A `DESC` column stores the
inverted bytes. Ties between equal keys are ordered by row id, which since data format 2 is
primary-key order ([storage below](#data-format-2)).

An entry is **visible at snapshot `ts`** when `begin_ts <= ts < end_ts`, exactly like a row
version. An update that changes the key ends the old entry (`end_ts` = the commit) and inserts
a new one. An update that leaves the key unchanged but changes covered values does the same.
An update that changes neither keeps the entry, so the index does no work for it. A delete ends
the entry. Scans at older snapshots therefore see the index as it was, and
`AS OF VERSION` queries can use indexes. `VACUUM` removes entries that ended at or before the
vacuum horizon, and from then on the index serves only snapshots from that horizon.

## Lifecycle

* **Build.** `CREATE INDEX` validates the definition against the catalog and the limits. It
  then reads every row of the table at the latest snapshot, inserts the entries into a new
  `indexes/<id>/entries.idx`, and writes a checkpoint that publishes the index and
  `indexes/catalog.meta` together. The index serves snapshots from its creation
  (`created_at_ts`). A row the index cannot hold (for example, a key over 512 bytes) fails the
  build, and nothing is left behind.
* **Maintenance.** Every later commit updates every index of the changed tables before the
  commit becomes visible. A row that an index cannot hold fails the commit, so an index is never
  behind the table: its watermark is always the latest commit.
* **Serving range.** An index answers snapshot `ts` only if
  `usable_from <= ts <= watermark`, where `usable_from = max(created_at_ts, vacuum horizon)`.
  The engine re-checks this on every scan page. A query outside the range fails instead of
  returning partial results, and the planner never chooses such an index.
* **Drop.** `DROP INDEX` removes the definition with a checkpoint, then deletes the files.
* **Crashes.** A crash during a build leaves files that no published catalog names. The next
  open deletes them, and the index id is not used up. A crash after a drop's checkpoint leaves
  files that the catalog no longer names, and they are removed in the same way. Commits after
  the last checkpoint are replayed into the indexes by recovery, as into the other projections.

## Scans

An `index_scan` plan node (plan wire v3, [plan-wire-format.md](plan-wire-format.md)) reads one
equality prefix and at most one range on the next key column, forward or backward:

```json
{"op":"index_scan","index_id":4,"bounds":{"prefix":[{"Int64":2}],"lower":{"value":{"Int64":9000},"inclusive":true}},
 "direction":"backward","columns":[{"field_id":3,"slot":0}],"limit":10}
```

The bounds become one exact byte range of the tree. NULL keys never match a range. Scans page
through the tree by key (keyset paging), so a query holds at most one page of entries, and a
`limit` sizes the first page. `row_fetch` reads the columns the index does not cover. It sorts
the row ids of each input batch, fetches them in one pass over the current or version store,
and emits the rows in index order. A covering scan needs no `row_fetch`.

## Planning

In cost mode ([optimizer.md](optimizer.md#access-paths-230)) the planner matches the conjuncts
of each relation against every index that serves the query snapshot. Equalities on the leading
key columns become the prefix, and comparisons on the next column become the range; anything
else remains a residual filter. Each candidate is costed against the sequential scan: index
pages, then random accesses for row fetches (fewer when `pk_locality` says rows are stored near
each other). Under `ORDER BY`, an index in that order (or its reverse) can replace the sort.
Under `ORDER BY ... LIMIT`, it replaces the TopK and stops after `LIMIT` rows. Rule mode ignores
indexes.

`ANALYZE` also collects **index statistics** (statistics format 2,
[statistics.md](statistics.md)): entries, stored entries including history, tree height, leaf
pages, average key size, distinct prefixes and `pk_locality`. EXPLAIN shows them in its
`Indexes:` section.

## Data format 2

Indexes need the order-preserving encoding, and the primary key uses it too. Since 2.3.0, a row
id is `compose(entity, pk XOR 2^63)` (`RowId::from_primary_key`), so primary-key order equals
row-id order, and negative keys sort before positive ones. A directory written before 2.3.0 has
no `format.meta`. The engine refuses to open it (`DbError::IncompatibleFormat`, C status
`ADB_INCOMPATIBLE_FORMAT = 12`) until it is migrated:

```bash
cargo run --release -p adb-migrate -- /path/to/db            # in place, keeps a backup
cargo run --release -p adb-migrate -- /path/to/db --to /new   # into a new directory
```

`adb-migrate` rewrites the WAL with the new row ids. The LSNs are kept, so CDC consumer offsets
stay valid. It then rebuilds the projections from the rewritten log and stamps the format.

Since 2.4.0 the current format is **3** (row schema versions and catalog records,
[schema.md](schema.md#data-format-3)). It encodes 2.3.0 data unchanged, so a format-2 directory
is upgraded in place on open and `adb-migrate` goes straight from format 1 to format 3.
Indexes of a table with a schema hold adapted rows (a column added with a default is indexed
with the default for older rows), may only name active columns, and keep their columns from
being dropped (RESTRICT).

## Costs and known limitations

* Each index adds work to every write of its table. At scale 1, inserting 100 rows per commit
  was about 3.9× slower with one index and about 8× slower with three
  (`write_overhead` in `examples/results/2.3.0-database.json`).
* Builds are single-threaded inserts, about 25,000 rows per second on the reference VM. Bulk
  loading of sorted entries is planned.
* Only one range column per scan; `OR`, `IN` and `IS NULL` predicates do not use indexes yet.
* Indexes cannot be unique, partial or on expressions, and there are no index-only counts.
