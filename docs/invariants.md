# Engine invariants (Milestone 2.4.0)

Each invariant names the test that guards it.

## Log

1. The canonical log is the source of truth; the engine never deletes log segments.
   *(crash::checkpoints_never_delete_log_segments)*
2. A transaction is appended as one contiguous run `Begin, Version*, Put/Delete*, Catalog*, Commit`; runs of
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

## Query execution (2.1)

19. The native engine executes only plans that passed validation: bounded depth, lists, slots and
    LIMIT/TopK; no slot produced twice; every slot read is produced by the operator's input.
    *(plan_validation::\*, joins::invalid_join_plans_are_rejected, wire::invalid_plans_are_rejected_both_ways)*
20. A plan from a producer with another wire version or dialect is refused, never half-understood.
    *(wire::version_mismatch_is_reported_as_such, wire::unknown_fields_are_rejected, jvm_contract, PlanJsonEncoderTest.matchesTheNativeContractFixture)*
21. Two instances of one entity in a query never share a slot.
    *(joins::self_join_keeps_relation_instances_apart, SelectBinderTest.selfJoinGetsDistinctSlots)*
22. NULL join keys never match; a LEFT JOIN emits every unmatched left row exactly once with NULL
    right slots, and the join condition (not a later WHERE) decides what matched.
    *(joins::inner_hash_join_matches_equal_keys, joins::left_hash_join_null_fills_unmatched_rows, joins::residual_condition_decides_left_join_matches, PredicatePushdownRuleTest.leftJoinPreservesNullFilling)*
23. Hash and nested-loop joins return the same rows for the same equality condition.
    *(joins::nested_loop_join_matches_hash_join_and_handles_inequalities)*
24. `TopK(k)` returns exactly what `Limit(Sort, k)` returns, ties and NULL placement included.
    *(aggregate_sort::top_k_equals_limit_over_sort)*
25. An INT64 aggregate never returns a wrapped value: it is exact or fails with
    `ARITHMETIC_OVERFLOW`. *(aggregate_sort::sum_is_checked, relational::sum_overflow_maps_to_its_own_status)*
26. Unsortable values (mixed types, NaN) fail cleanly; sorting never panics.
    *(aggregate_sort::unsortable_values_are_errors)*
27. Blocking operators never hold more than the query memory budget or the materialized-row cap,
    and return all reserved memory when the query ends or fails.
    *(memory::tests, joins::build_side_respects_memory_and_row_limits, aggregate_sort::group_count_is_capped)*
28. Join work is bounded: per-left-row fanout, total nested-loop comparisons and the size of an
    output batch. *(joins::join_fanout_is_capped, joins::nested_loop_comparisons_are_capped, joins::join_output_is_batched)*

## Statistics and optimization (2.2.3)

29. Statistics are derived data: a missing, damaged or half-published document only makes
    the entity count as never analyzed; queries still run.
    *(statistics::damaged_document_counts_as_missing, statistics::interrupted_publication_keeps_the_previous_document,
    statistics::concurrent_analyze_publishes_a_consistent_document, CostBasedGatewayTest.engineStatisticsAreCachedByGeneration)*
30. The modification counters equal the committed mutations of the log after a crash, a clean
    shutdown and a projection rebuild. *(statistics::modification_counters_track_commits, statistics::rebuild_recounts_modifications)*
31. `ANALYZE` never exceeds its row, working-set, time or column limits and persists nothing
    when it fails. *(collector::limits_are_enforced, collector::sample_memory_is_charged_realistically,
    statistics::analyze_failures_are_reported)*
32. The JVM never plans on a misread statistics document.
    *(StatisticsCodecTest.rejectsMalformedDocuments)*
33. Join reordering never changes a query's result; LEFT JOINs are never reordered across.
    *(JoinOrderPropertiesTest.reorderingPreservesResults, JoinReorderRuleTest.leftJoinIsABoundary,
    JoinReorderRuleTest.singleRelationConditionsAreKept)*
34. A LEFT JOIN is never executed with swapped inputs.
    *(CostBasedPlanningTest.leftJoinNeverSwaps, CostModelTest.leftJoinBuildsRight)*
35. Estimates and costs are finite and non-negative whatever the statistics.
    *(CardinalityEstimatorTest.boundedEstimates, CostModelTest.boundedArithmetic)*
36. Profile node ids are the pre-order positions of the plan nodes, so estimates and actuals
    are matched exactly. *(profile::profile_numbers_nodes_in_plan_preorder, CostBasedPlanningTest.annotatesEveryNode)*

## Secondary indexes (2.3.0)

37. The byte order of encoded keys equals the SQL order of their values, column by column, in
    both directions; decoding never panics on any input.
    *(key_encoding::tests::byte_order_matches_sql_order, key_encoding_fuzz::adversarial_values_keep_their_order,
    key_encoding_fuzz::arbitrary_bytes_never_panic)*
38. At every snapshot an index serves, a scan returns exactly the visible rows that match its
    bounds, in index order (ties in primary-key order), after any sequence of inserts, updates
    and deletes. *(projection::scans_at_any_snapshot_match_the_model, index_queries::scans_are_snapshot_consistent)*
39. An index never lags its table: a row an index cannot hold fails the commit before it is
    logged, and a failed build leaves no files and no definition.
    *(index_maintenance::unindexable_rows_are_refused_before_logging, index_lifecycle::failed_builds_leave_nothing_behind)*
40. Indexes are projections of the log: after a crash, recovery and `rebuild_projections`
    restore them, and `verify` checks them against the table.
    *(index_maintenance::recovery_replays_and_checkpoints_publish_the_index,
    index_maintenance::rebuild_projections_rebuilds_the_index, index_lifecycle::create_index_builds_over_existing_rows)*
41. A published catalog names exactly the index directories on disk: an unpublished build or a
    dropped index leaves no files after the next open.
    *(index_lifecycle::crash_during_build_leaves_no_index, index_lifecycle::crash_after_drop_checkpoint_removes_leftover_files)*
42. An index answers only snapshots in `usable_from..=watermark`; a scan outside fails rather
    than returning partial results, and the planner never chooses such an index.
    *(index_queries::unusable_snapshots_and_dropped_indexes_fail, AccessPathPlanningTest.unusableSnapshotsAndRuleModeScanTheTable)*
43. Choosing an access path or eliding a sort never changes a query's result: index, sequential
    and rule-mode plans return the same rows.
    *(AccessPathPlanningTest, the selectivity sweep and workloads C and D in `scripts/check-benchmarks.py`)*
44. A data directory of an older format is never opened as if it were current; migration keeps
    every commit, LSN and consumer offset.
    *(format::unmarked_directories_with_data_are_refused, migrate::migration_rewrites_keys_and_keeps_everything_else)*

## Schema evolution (2.4.0)

45. A DDL statement is validated against a copy of the registry before it is logged: a refused
    statement leaves no record, no epoch change and no poisoned instance.
    *(schema_ddl::refused_ddl_leaves_no_trace, schema_restrict::drop_column_is_restricted_by_indexes)*
46. The registry is a projection of the log: recovery replays catalog records with the rows,
    `rebuild_projections` restores it, and a damaged `registry.meta` is reported as corruption.
    *(schema_registry::recovery_replays_catalog_records, schema_registry::rebuild_restores_the_registry,
    schema_registry::damaged_registry_is_corruption)*
47. Every row written to a table with a schema fits its latest version when it is logged (only
    active fields, right types, `NOT NULL` present, primary key equal to the row key).
    *(schema_writes::invalid_rows_are_refused_before_logging, fuzz::random_evolutions_keep_valid_chains_and_adapt_any_row)*
48. A row is read through the version of the read view's epoch: fields dropped by then are
    hidden, fields added after the row was written read their default, and a query's epoch never
    moves while it runs. *(adapter::*, schema_reads::running_queries_keep_their_schema,
    schema_hardening::ddl_writes_and_scans_run_concurrently)*
49. A plan runs only at the catalog epoch it was made at; any other epoch is refused before a row
    is produced. *(schema_reads::plans_of_another_epoch_are_refused, ffi index::stale_plans_are_refused_over_the_abi)*
50. Field and entity ids are never reused, and no index key or covered column, primary key or
    referenced table can be dropped. *(validation::unsafe_changes_are_refused, schema_restrict::*)*
51. Indexes and statistics see rows exactly as queries do (adapted to the latest version).
    *(schema_reads::indexes_and_statistics_see_adapted_rows)*
52. Data format 2 upgrades to 3 without rewriting anything; a newer format is refused.
    *(format::format_two_directories_are_upgraded_in_place, format::newer_formats_are_refused)*

