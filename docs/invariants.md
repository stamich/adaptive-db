# Engine invariants (Milestone 2.0.3)

Each invariant names the test that guards it.

## Log

1. The canonical log is the source of truth; the engine never deletes log segments.
   *(crash::checkpoints_never_delete_log_segments)*
2. A transaction is appended as one contiguous run `Begin, Version*, Put/Delete*, Commit`; runs of
   different transactions never interleave. *(committed::tests)*
3. `commit()` returns only after the transaction's records are durable.
   *(group_commit::concurrent_disjoint_commits_are_all_durable)*
4. A run without `Commit` belongs to a dead process and never takes effect.
   *(committed::an_unfinished_run_followed_by_begin_is_discarded)*

## Projections and checkpoints

5. Pages modified after the last checkpoint never reach projection files (no-steal).
   *(buffer::dirty_pages_are_never_evicted, btree::unflushed_changes_vanish_but_flushed_state_is_intact)*
6. A checkpoint publishes all dirty pages, both B+Tree roots and the checkpoint record atomically.
   *(journal::recover_reapplies_a_committed_journal, crash::interrupted_checkpoint_is_completed_on_open)*
7. After recovery, the projections equal the replay of the whole log.
   *(crash::automatic_checkpoints_and_repeated_recovery_preserve_every_commit)*
8. Projections can be rebuilt from the log alone. *(crash::corrupted_projection_is_rebuilt_from_the_log)*
9. Persisted bytes are checksummed; a mismatch is reported as corruption, never silently used.
   *(journal::corrupt_committed_journal_is_reported_not_ignored, crash::corrupted_projection_is_rebuilt_from_the_log)*

## MVCC

10. A snapshot sees exactly the commits published at or before it; publication follows durability.
    *(transactions::reads_never_see_uncommitted_or_other_transactions_writes)*
11. Historical intervals `[begin, end)` of a row never overlap and end before the current state begins.
    *(IntegrityChecker)*
12. `get_at(row, ts)` and scans at `ts` return the same result before and after restarts and vacuum.
    *(vacuum_and_scans::vacuum_removes_tombstones_and_history_stays_readable)*

## Transactions

13. `Snapshot`: no two concurrent transactions both commit a write to the same row.
    *(transactions::write_write_conflict_is_detected, group_commit::contended_increments_with_retry_are_not_lost)*
14. `Serializable`: a commit is equivalent to executing the transaction atomically at its commit
    point (no write skew). *(transactions::serializable_isolation_prevents_write_skew)*
15. A tombstone is vacuumed only when no live transaction's snapshot predates it.
    *(vacuum_and_scans::vacuum_keeps_tombstones_needed_by_active_transactions)*

## Failure handling

16. After any failure between the first log append and durability, the instance refuses all
    operations until reopened. *(crash::failure_inside_commit_poisons_until_reopen)*

## Space

17. Updating a row does not grow the current heap. *(persistent_current::updates_do_not_grow_the_heap)*

## Change feed

18. Events are delivered in commit order, durable only, gap-free and duplicate-free across pages.
    *(cdc::paging_with_the_returned_cursor_is_gap_free_and_duplicate_free)*
