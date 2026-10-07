package io.adb.logical

import io.adb.logical.LogicalPlan.*

/** Turns bound SELECT statements into canonical logical plans. */
object LogicalPlanner:
  /** Builds `Limit(Project(Filter(TableScan)))`, omitting the filter and limit when absent.
    *
    * @param select bound SELECT
    * @return the unoptimized logical plan
    */
  def plan(select: BoundSelect): LogicalPlan =
    val base = TableScan(select.entity)
    val filtered = select.predicate.fold[LogicalPlan](base)(Filter(base, _))
    val projected = Project(filtered, select.fields)
    select.limit.fold[LogicalPlan](projected)(Limit(projected, _))
