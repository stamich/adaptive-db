package io.adb.gateway

import io.adb.model.JsonReader

/** Renders the native query profile (`adb_query_profile_json`) as an indented operator tree:
  *
  * {{{
  * peak memory 18.2 KiB of 256.0 MiB
  * top_k rows=3 batches=1 time=0.21ms rows_in=4 compactions=1 peak_memory_bytes=1296
  *   aggregate rows=4 ...
  * }}}
  */
object ProfileRenderer:
  /** Renders a profile document. */
  def render(json: String): String =
    val profile = JsonReader.parse(json).asInstanceOf[Map[String, Any]]
    val peak = number(profile("peak_memory_bytes"))
    val limit = number(profile("memory_limit_bytes"))
    (s"peak memory ${bytes(peak)} of ${bytes(limit)}" +: renderOperator(profile("root"), 0)).mkString("\n")

  /** Lines of one operator and its inputs. */
  private def renderOperator(node: Any, depth: Int): Vector[String] =
    val operator = node.asInstanceOf[Map[String, Any]]
    val counters = operator.get("counters").map(_.asInstanceOf[Map[String, Any]]).getOrElse(Map.empty)
    val micros = number(operator("elapsed_us"))
    val line = ("  " * depth) + s"${operator("operator")} rows=${number(operator("rows_out"))} " +
      s"batches=${number(operator("batches_out"))} time=${f"${micros / 1000.0}%.2f"}ms" +
      counters.toVector.sortBy(_._1).map((name, value) => s" $name=${number(value)}").mkString
    val children = operator.get("children").map(_.asInstanceOf[Vector[Any]]).getOrElse(Vector.empty)
    line +: children.flatMap(renderOperator(_, depth + 1))

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
