package io.adb.gateway

import io.adb.logical.BoundSelect

/** A planned SELECT as the plan cache keeps it: the encoded physical plan and what is needed to
  * run it and read its result, but no syntax trees.
  *
  * @param json      the plan as sent to the engine (with the schema epoch it was made at)
  * @param outputs   result columns as `(name, slot)` in select-list order
  * @param mode      optimizer mode the plan was made under (for the feedback log)
  * @param estimates the planner's per-operator estimates (for the feedback log)
  * @param schemaEpoch catalog epoch the plan was made at
  * @param shape     the statement's semantic shape, computed once at planning time so telemetry
  *                  on a cache hit only reads it
  */
final case class CachedQuery(
    json: String,
    outputs: Vector[(String, Int)],
    mode: io.adb.logical.OptimizerMode,
    estimates: Map[Int, io.adb.physical.NodeEstimate],
    shape: io.adb.telemetry.QueryShape,
    schemaEpoch: Long
):
  /** The planner's estimate of the rows the statement returns (-1 if there is none). */
  def estimatedRows: Long = estimates.get(0).fold(-1L)(e => math.round(e.rows))

  /** Approximate heap bytes (the JSON dominates). */
  def weight: Long = json.length.toLong * 2 + outputs.size * 64L + estimates.size * 96L + 512L

/** Construction of cache values. */
object CachedQuery:
  /** The result columns of `select` as `(name, slot)`. */
  def outputs(select: BoundSelect): Vector[(String, Int)] =
    select.output.map(column => column.name -> column.attribute.slot.value)

  /** The cache value of a plan made for `select`. */
  def of(select: BoundSelect, plans: AdaptiveDatabase.Plans): CachedQuery =
    CachedQuery(
      io.adb.physical.PlanJsonEncoder.encode(plans.physical.plan, plans.schemaEpoch),
      outputs(select),
      plans.mode,
      plans.physical.estimates,
      QueryShapeExtractor.extract(select),
      plans.schemaEpoch
    )
