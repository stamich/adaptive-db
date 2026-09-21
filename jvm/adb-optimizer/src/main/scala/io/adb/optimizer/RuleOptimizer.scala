package io.adb.optimizer

import io.adb.logical.LogicalPlan

/** Documents `RuleOptimizer` and its role in the Milestone 2.0.1 JVM control plane. */
final class RuleOptimizer(rules: Vector[Rule] = Vector(PointLookupRule)):
  /** Documents `optimize` and its role in the Milestone 2.0.1 JVM control plane. */
  def optimize(plan: LogicalPlan): LogicalPlan =
    var current = plan
    var changed = true
    var rounds = 0
    while changed && rounds < 16 do
      val next = rules.foldLeft(current)((p, rule) => rule(p))
      changed = next != current
      current = next
      rounds += 1
    current
