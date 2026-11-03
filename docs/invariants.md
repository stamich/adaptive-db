# Engine invariants (Milestone 2.5.3)

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

## Commit order and change log (2.5.0)

53. Every data and catalog commit takes the next `CommitSeq`; a read-only transaction takes none;
    a log whose numbers do not increase is refused as corrupt.
    *(commit_seq::catalog_and_data_commits_share_one_order, commit_seq::read_only_commits_do_not_consume_a_number,
    commit_seq::a_repeated_sequence_number_is_corruption)*
54. `latest_commit_seq` is the newest commit whose change-log envelope has been appended; the
    sequence is published only after the append. *(change_log::every_commit_has_an_envelope_with_row_images)*
55. The change log is derived: after any damage, loss or surplus it is repaired from the WAL at
    open and then agrees with it. *(change_log_recovery::*, change-log fuzz::*)*
56. Change-log appends are idempotent and strictly ordered; frames are CRC-checked; a torn last
    frame is dropped, earlier damage is an error. *(store::append_is_idempotent, store::torn_tail_is_truncated,
    store::mid_log_damage_is_an_error)*
57. A cursor delivers each commit once, in order, at least one per batch, and fails with
    `Truncated` when its start was pruned. *(store::cursor_streams_in_batches, store::cursor_respects_the_byte_budget)*
58. Retention never removes a segment a registered consumer still needs unless a configured limit
    says so; an overtaken consumer is `NeedsRebuild`. *(retention::*)*
59. A projection frontier never moves backward; waiting for a projection is bounded (60 s); an
    unreadable projection never satisfies a wait. *(registry::frontiers_are_monotonic_and_lag_follows_the_head,
    wait::the_wait_cap_is_bounded, wait::unreadable_states)*
60. A session token only moves forward, a token beyond the head is refused, and a read is served
    from a projection only if the freshness resolver allows it. *(FreshnessResolverTest,
    FreshnessAcceptanceTest, ConsistencySmoke)*
61. Data format 3 upgrades to 4 without rewriting anything. *(format::format_three_directories_are_upgraded_in_place, commit_seq::logs_without_sequence_numbers_are_numbered_by_timestamp)*

## Plan cache (2.5.1)

62. A cached plan is used only if the engine's schema epoch and the statistics generations of the
    tables it reads equal the ones read before it was planned; otherwise it is dropped and the
    statement is planned again. *(PlanCacheTest, PlanCacheSmoke)*
63. The plan cache never exceeds its byte budget, stores a statement only after a second
    sighting, and never stores AS OF statements or plans that read a projection whose use depends
    on the session. *(BoundedLruTest, PlanCacheTest, PlanCacheSmoke)*

## Telemetry (2.5.2)

64. With telemetry off, a query does no telemetry work: no clock read, no counter, no allocation,
    no profile fetch. *(TelemetrySmoke)*
65. The telemetry queue is bounded and `offer` never blocks; a full queue drops the event and counts
    it. *(TelemetryRuntimeTest, TelemetrySmoke)*
66. A shape contains ids and flags only: never SQL text or a literal value, and two statements that
    differ only in literals share it. *(QueryShapeExtractorTest, TelemetrySmoke)*

## Telemetry capture path (2.5.2.1)

67. A query the sampler skips allocates nothing more than an unobserved one (at most 64 bytes in
    the benchmark budget); a captured query allocates its event, not a profile document (at most
    512 bytes). *(check-benchmarks.py, TelemetrySmoke, LongIdSetTest)*
68. `adb_query_telemetry` writes exactly the 32 bytes of `AdbQueryTelemetry`, zeroes them first, and
    reports the rows the plan's leaf operators produced; the Java and C layouts are identical.
    *(query_telemetry.rs, NativeQueryTelemetryTest)*

## Workload advisor (2.5.3)

69. The advisor never changes the database: it holds no reference to the engine, the catalog or the
    gateway, and its only output is a report. *(AdvisorSmoke: the index count is the same before
    and after; check-benchmarks.py)*
70. An evaluation reads immutable values only (per-shape counters, shape definitions, a
    `PhysicalDesign`), never the telemetry queue or a live structure. *(AdvisorTest)*
71. A finding is shown only after `confirmAfter` evaluations in a row, withdrawn only after
    `withdrawAfter` misses in a row, and advice that was carried out is withdrawn at once.
    *(HysteresisTest, AdvisorTest, AdvisorSmoke)*
72. The first evaluation, and one after a counter reset, are baselines: telemetry counters since the
    process started are never read as one window. *(WorkloadModelTest)*
73. With the advisor off (the default) there is no advisor thread. *(AdvisorSchedulerTest, AdvisorSmoke)*

## AUTO_SAFE actuator (2.5.4)

74. `off` is the default, and with it there is no actuator thread and no state change on any query.
    *(AdaptiveControllerTest, ActuatorSmoke)*
75. `advisory` never runs a statement; only `auto_safe` does. *(AdaptiveControllerTest, ActuatorSmoke)*
76. The actuator runs `CREATE INDEX`, `ANALYZE` and `DROP INDEX`, and nothing else. *(ActionTest, RollbackPlannerTest)*
77. A `DROP INDEX` reaches the engine only for an index in the ownership registry, checked twice
    (`OwnershipRegistry.mayDrop` and `OwnedOnlyActuator`); a user's index is never dropped.
    *(OwnershipRegistryTest, RollbackPlannerTest, ActuatorSmoke, check-benchmarks.py)*
78. At most one action is approved per tick and at most one build runs at a time. *(AdaptiveControllerTest, BuildGuardTest)*
79. The gate returns the same rejection reason for the same input, checked in a fixed order, and a
    rejection is logged once per target and reason. *(PolicyGateTest, AdaptiveControllerTest)*
80. An action is kept only on evidence of fewer rows read per execution, measured over windows that
    started after it was applied; time can only veto beyond the tolerance. *(EffectivenessVerifierTest, BaselineTest)*
81. A rolled-back target waits longer each time (doubling, capped at 8x), and an index a person
    dropped is not rebuilt. *(FlapGuardTest, AdaptiveControllerTest, ActuatorSmoke)*
82. A query returns the same rows in every mode, with or without the advisor's indexes.
    *(ActuatorSmoke)*
