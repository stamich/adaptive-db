# Milestone 1.5 architecture

```text
Transaction API
      |
      v
Transaction Manager / Snapshot Isolation
      |
      +------> logical WAL -> fsync
      |                        |
      |                        v
      |                    Commit LSN
      v
Persistent Current Store
      |
      +--> B+Tree: RowId -> RowLocation
      |
      +--> HeapFile -> SlottedPage -> BufferPool -> FilePageStore
      |
      +--> flush
      v
Checkpoint(last_applied_commit_lsn, last_commit_ts)
      |
      v
publish CommitTs

Historical versions:
logical WAL -> recovery -> in-memory VersionStore
```

`page_lsn` is stored on heap and B+Tree pages. Milestone 1.5 enforces WAL-before-data at the engine level; a later milestone can use pageLSN for physiological redo/ARIES-like recovery.
