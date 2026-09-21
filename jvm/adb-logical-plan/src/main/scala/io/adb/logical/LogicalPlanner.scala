package io.adb.logical

import io.adb.logical.LogicalPlan.*

/** Documents `LogicalPlanner` and its role in the Milestone 2.0.1 JVM control plane. */
object LogicalPlanner:
  /** Documents `plan` and its role in the Milestone 2.0.1 JVM control plane. */
  def plan(select: BoundSelect): LogicalPlan =
    val base = TableScan(select.entity)
    val filtered = select.predicate.fold[LogicalPlan](base)(Filter(base, _))
    val projected = Project(filtered, select.fields)
    select.limit.fold[LogicalPlan](projected)(Limit(projected, _))
