package io.adb.optimizer

import io.adb.logical.LogicalPlan

/** Documents `Rule` and its role in the Milestone 2.0.1 JVM control plane. */
trait Rule:
  /** Documents `name` and its role in the Milestone 2.0.1 JVM control plane. */
  def name: String
  /** Documents `apply` and its role in the Milestone 2.0.1 JVM control plane. */
  def apply(plan: LogicalPlan): LogicalPlan
