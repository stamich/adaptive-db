# Milestone 1.5 invariants

1. Commit is acknowledged only after the logical WAL Commit record is durable.
2. Persistent Current Store pages are flushed only after WAL fsync for the commit that created them.
3. A checkpoint is advanced only after Current Store heap and B+Tree pages are flushed.
4. New transactions see a CommitTs only after WAL durability, Current Store application and checkpoint persistence.
5. B+Tree maps one logical RowId to the newest physical RowLocation; older heap tuples are immutable stale copies.
6. DELETE is represented as a tombstone CurrentRecord; B+Tree physical deletion is intentionally deferred.
7. Snapshot reads use persistent Current Store when current.commit_ts <= snapshot; otherwise VersionStore supplies an older committed version.
8. VersionStore remains reconstructable from canonical WAL in Milestone 1.5.
9. A crash between WAL fsync and checkpoint is repaired by replaying committed WAL entries newer than checkpoint.last_applied_commit_lsn.
10. Replaying an already-written CurrentRecord is semantically idempotent because the B+Tree pointer is replaced with the latest appended copy.
