package io.adb.logical

import io.adb.logical.LogicalPlan.*

/** Turns bound SELECT statements into canonical logical plans. */
object LogicalPlanner:
  /** Builds the canonical tree
    *
    * {{{
    * Limit(Project(Sort(Aggregate(Filter(Join(...Join(Scan, Scan)...))))))
    * }}}
    *
    * omitting every operator the query does not need. Joins are left-deep in FROM order, and
    * the WHERE predicate sits above all joins; the optimizer pushes it down.
    *
    * @param select bound SELECT
    * @return the unoptimized logical plan
    */
  def plan(select: BoundSelect): LogicalPlan =
    val joined = select.joins.foldLeft[LogicalPlan](TableScan(select.base)) { (left, join) =>
      Join(left, TableScan(join.relation), join.joinType, join.condition)
    }
    val filtered = select.predicate.fold(joined)(Filter(joined, _))
    val aggregated =
      if select.isAggregate then Aggregate(filtered, select.groupBy, select.aggregates)
      else filtered
    val sorted = if select.orderBy.isEmpty then aggregated else Sort(aggregated, select.orderBy)
    val projected = Project(sorted, select.output.map(_.attribute))
    select.limit.fold[LogicalPlan](projected)(Limit(projected, _))
