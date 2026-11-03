package io.adb.telemetry

/** The compact event one captured query leaves in the queue: the id of its shape and a few
  * numbers. The shape itself is registered once in the [[QueryShapeRegistry]], never copied into
  * events.
  *
  * @param shapeId       id of the query's [[QueryShape]]
  * @param schemaEpoch   catalog epoch the plan was made at
  * @param estimatedRows planner's estimate of the rows returned (-1 if unknown)
  * @param actualRows    rows returned
  * @param rowsRead      rows the scans produced (-1 if the profile was not available)
  * @param elapsedNanos  wall-clock nanoseconds of the whole statement
  * @param cacheHit      whether the plan came from the plan cache
  * @param failed        whether the statement failed
  */
final case class QueryTelemetry(
    shapeId: Long,
    schemaEpoch: Long,
    estimatedRows: Long,
    actualRows: Long,
    rowsRead: Long,
    elapsedNanos: Long,
    cacheHit: Boolean,
    failed: Boolean
):
  /** `max(actual, estimated) / max(1, min(actual, estimated))`, the q-error of the estimate
    * (1.0 = exact); 1.0 when no estimate was recorded.
    */
  def qError: Double =
    if estimatedRows < 0 then 1.0
    else
      val hi = math.max(actualRows, estimatedRows).toDouble
      val lo = math.max(1L, math.min(actualRows, estimatedRows)).toDouble
      hi / lo
