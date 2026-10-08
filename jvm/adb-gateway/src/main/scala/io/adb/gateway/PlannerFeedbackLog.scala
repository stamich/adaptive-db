package io.adb.gateway

import io.adb.physical.PlanJsonEncoder
import java.nio.charset.StandardCharsets
import java.nio.file.{Files, Path, StandardCopyOption, StandardOpenOption}
import java.security.MessageDigest

/** Append-only JSON Lines log of estimated versus actual rows per executed query
  * (`planner-feedback.jsonl`): the raw material for calibrating the cost model and, later,
  * for workload-driven advisors.
  *
  * One line per query:
  * {{{
  * {"ts_ms":1730000000000,"mode":"cost","query":"3f2a…","nodes":[{"node_id":0,"operator":"top_k",
  *  "estimated_rows":10.0,"actual_rows":10,"q_error":1.0}]}
  * }}}
  * `query` is a SHA-256 prefix of the SQL text, so literals never reach the log. The log is
  * bounded: once it would exceed `maxBytes` it is moved to `<name>.1` (replacing the previous
  * one) and a new file is started. Failures to write are swallowed: feedback must never fail a
  * query.
  *
  * @param path     log file
  * @param maxBytes size at which the file is rotated
  */
final class PlannerFeedbackLog(val path: Path, maxBytes: Long = PlannerFeedbackLog.DefaultMaxBytes):
  require(maxBytes >= 1024, "maxBytes must be at least 1 KiB")

  /** Appends the comparison of one executed query. */
  def record(sql: String, mode: String, comparisons: Vector[ProfileRenderer.Comparison]): Unit =
    if comparisons.nonEmpty then
      val nodes = comparisons.map { c =>
        f"""{"node_id":${c.nodeId},"operator":${PlanJsonEncoder.quote(c.operator)},"estimated_rows":${c.estimated}%.1f,""" +
          f""""actual_rows":${c.actual},"q_error":${c.qError}%.3f}"""
      }
      val line = s"""{"ts_ms":${System.currentTimeMillis()},"mode":${PlanJsonEncoder.quote(mode)},"query":"${fingerprint(sql)}","nodes":${nodes.mkString("[", ",", "]")}}""" + "\n"
      append(line.getBytes(StandardCharsets.UTF_8))

  /** Writes `bytes`, rotating first if the file would grow past the bound. */
  private def append(bytes: Array[Byte]): Unit = synchronized {
    try
      Option(path.getParent).foreach(Files.createDirectories(_))
      if Files.exists(path) && Files.size(path) + bytes.length > maxBytes then
        Files.move(path, path.resolveSibling(path.getFileName.toString + ".1"), StandardCopyOption.REPLACE_EXISTING)
      Files.write(path, bytes, StandardOpenOption.CREATE, StandardOpenOption.APPEND, StandardOpenOption.WRITE)
    catch case _: java.io.IOException => ()
  }

  /** First 16 hex digits of the SHA-256 of the SQL text. */
  private def fingerprint(sql: String): String =
    MessageDigest.getInstance("SHA-256").digest(sql.trim.getBytes(StandardCharsets.UTF_8)).take(8).map(b => f"${b & 0xff}%02x").mkString

/** Defaults of [[PlannerFeedbackLog]]. */
object PlannerFeedbackLog:
  /** Rotate at 4 MiB (so at most 8 MiB on disk). */
  val DefaultMaxBytes: Long = 4L * 1024 * 1024
