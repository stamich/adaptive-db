package io.adb.physical

/** Human-readable EXPLAIN rendering of logical and physical plans. */
object Explain:
  /** Renders a logical plan, one operator argument per line. */
  def logical(plan: io.adb.logical.LogicalPlan): String = render(plan.toString)
  /** Renders a physical plan, one operator argument per line. */
  def physical(plan: PhysicalPlan): String = render(plan.toString)
  /** Breaks a case-class rendering into lines. */
  private def render(value: String): String = value.replace(",", ",\n  ")
