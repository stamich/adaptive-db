package io.adb.gateway

import io.adb.model.JsonReader
import io.adb.physical.NodeEstimate

/** Renders the native query profile (`adb_query_profile_json`) as an indented operator tree:
  *
  * {{{
  * peak memory 18.2 KiB of 256.0 MiB
  * [0] top_k rows=3 est=3 q=1.0 batches=1 time=0.21ms rows_in=4 compactions=1 peak_memory_bytes=1296
  *   [1] aggregate rows=4 est=2 q=2.0 ...
  * max q-error 2.0 at [1] aggregate
  * }}}
  *
  * With estimates (keyed by the same pre-order node ids the engine reports) every operator
  * shows its estimated rows and the q-error `max(est, actual) / min(est, actual)` (both
  * counted as at least one row), the standard measure of cardinality-estimation quality.
  */
object ProfileRenderer:
  /** One operator's estimate compared with what it actually produced.
    *
    * @param nodeId    pre-order node id
    * @param operator  operator name
    * @param estimated estimated rows
    * @param actual    rows the operator returned
    */
  final case class Comparison(nodeId: Int, operator: String, estimated: Double, actual: Long) derives CanEqual:
    /** `max(est, actual) / min(est, actual)`, both at least one row. */
    def qError: Double = ProfileRenderer.qError(estimated, actual.toDouble)

  /** Renders a profile document, comparing it with `estimates` when given. */
  def render(json: String, estimates: Map[Int, NodeEstimate] = Map.empty): String =
    val profile = JsonReader.parse(json).asInstanceOf[Map[String, Any]]
    val peak = number(profile("peak_memory_bytes"))
    val limit = number(profile("memory_limit_bytes"))
    val lines = renderOperator(profile("root"), 0, estimates)
    val worst = compare(json, estimates).maxByOption(_.qError).map(c => f"max q-error ${c.qError}%.1f at [${c.nodeId}] ${c.operator}")
    ((s"peak memory ${bytes(peak)} of ${bytes(limit)}" +: lines) ++ worst).mkString("\n")

  /** Every operator of the profile that has an estimate, in pre-order. */
  def compare(json: String, estimates: Map[Int, NodeEstimate]): Vector[Comparison] =
    val profile = JsonReader.parse(json).asInstanceOf[Map[String, Any]]
    /** Comparisons of `node` and its inputs, in pre-order. */
    def walk(node: Any): Vector[Comparison] =
      val operator = node.asInstanceOf[Map[String, Any]]
      val own = nodeId(operator).flatMap(id => estimates.get(id).map(e =>
        Comparison(id, operator("operator").toString, e.rows, number(operator("rows_out")))))
      own.toVector ++ children(operator).flatMap(walk)
    walk(profile("root"))

  /** `max(a, b) / min(a, b)` with both at least one. */
  def qError(estimated: Double, actual: Double): Double =
    val (e, a) = (math.max(1.0, estimated), math.max(1.0, actual))
    math.max(e, a) / math.min(e, a)

  /** Lines of one operator and its inputs. */
  private def renderOperator(node: Any, depth: Int, estimates: Map[Int, NodeEstimate]): Vector[String] =
    val operator = node.asInstanceOf[Map[String, Any]]
    val counters = operator.get("counters").map(_.asInstanceOf[Map[String, Any]]).getOrElse(Map.empty)
    val micros = number(operator("elapsed_us"))
    val rows = number(operator("rows_out"))
    val id = nodeId(operator)
    val prefix = if estimates.isEmpty then "" else id.fold("")(i => s"[$i] ")
    val estimate = id.flatMap(estimates.get).fold("")(e => f" est=${e.rows}%.0f q=${qError(e.rows, rows.toDouble)}%.1f")
    val line = ("  " * depth) + s"$prefix${operator("operator")} rows=$rows$estimate " +
      s"batches=${number(operator("batches_out"))} time=${f"${micros / 1000.0}%.2f"}ms" +
      counters.toVector.sortBy(_._1).map((name, value) => s" $name=${number(value)}").mkString
    line +: children(operator).flatMap(renderOperator(_, depth + 1, estimates))

  /** The operator's `node_id` (absent in profiles of engines before 2.2.3). */
  private def nodeId(operator: Map[String, Any]): Option[Int] = operator.get("node_id").map(number(_).toInt)

  /** Inputs of an operator. */
  private def children(operator: Map[String, Any]): Vector[Any] =
    operator.get("children").map(_.asInstanceOf[Vector[Any]]).getOrElse(Vector.empty)

  /** A JSON number as `Long`. */
  private def number(value: Any): Long = value match
    case n: Long => n
    case n: Double => n.toLong
    case other => throw new IllegalArgumentException(s"expected a number, got $other")

  /** Human-readable byte size. */
  private def bytes(value: Long): String =
    if value < 1024 then s"$value B"
    else if value < 1024 * 1024 then f"${value / 1024.0}%.1f KiB"
    else f"${value / (1024.0 * 1024)}%.1f MiB"
