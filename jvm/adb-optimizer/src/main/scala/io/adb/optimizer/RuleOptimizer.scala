package io.adb.optimizer

import io.adb.logical.LogicalPlan

/** Applies rewrite rules repeatedly until the plan reaches a fixed point.
  *
  * @param rules rules applied in order during each round
  */
final class RuleOptimizer(rules: Vector[Rule] = RuleOptimizer.DefaultRules):
  /** Runs all rules in rounds until a round changes nothing, capped at 16 rounds to guarantee termination.
    *
    * @param plan bound logical plan
    * @return the optimized plan
    */
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

/** The default rule set. */
object RuleOptimizer:
  /** Pushdown first, so point lookups also apply to the inputs of joins. */
  val DefaultRules: Vector[Rule] = Vector(PredicatePushdownRule, PointLookupRule)
