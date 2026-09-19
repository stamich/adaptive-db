1. The `commit()` response may only be returned after a durable WAL sync.
2. The WAL before-image is sufficient for independent reconstruction of the Version Store.
3. `CurrentStore` and `VersionStore` may have different physical replay positions after a crash, but recovery brings both to the same committed LSN.
4. A checkpoint is published only after a durable flush of both persistent stores.
5. Reapplying a committed transaction is logically idempotent.
6. For a given `RowId`, historical intervals `[beginTs,endTs)` must not overlap.
7. `get_at(RowId,T)` returns the same result before and after a restart.
8. The Current Store contains at most one logically current version of each `RowId`.
9. The Version Store is not fully reconstructed in RAM.
10. A page checksum mismatch is treated as corruption.
11. The B+Tree physical layout is not part of the database’s public semantics.
12. WAL segments may only be deleted before the oldest required retain LSN.

Translated with DeepL.com (free version)