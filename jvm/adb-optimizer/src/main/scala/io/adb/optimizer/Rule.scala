package io.adb.optimizer

import io.adb.logical.LogicalPlan

/** A logical-plan rewrite applied by [[RuleOptimizer]]; must preserve query semantics. */
trait Rule:
  /** Human-readable rule name. */
  def name: String
  /** Rewrites `plan`; returns it unchanged (equal) when the rule does not apply. */
  def apply(plan: LogicalPlan): LogicalPlan
