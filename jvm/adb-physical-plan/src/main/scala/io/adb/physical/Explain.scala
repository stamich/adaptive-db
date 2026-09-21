package io.adb.physical

/** Documents `Explain` and its role in the Milestone 2.0.1 JVM control plane. */
object Explain:
  /** Documents `logical` and its role in the Milestone 2.0.1 JVM control plane. */
  def logical(plan: io.adb.logical.LogicalPlan): String = render(plan.toString)
  /** Documents `physical` and its role in the Milestone 2.0.1 JVM control plane. */
  def physical(plan: PhysicalPlan): String = render(plan.toString)
  /** Documents `render` and its role in the Milestone 2.0.1 JVM control plane. */
  private def render(value: String): String = value.replace(",", ",\n  ")
