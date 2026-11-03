package io.adb.cli

import io.adb.ffm.NativeDatabase
import io.adb.gateway.{AdaptiveDatabase, PlannerFeedbackLog}
import java.nio.file.Path
import scala.io.StdIn

/** Provides the interactive SQL shell and the repository feature-tour entry point. */
object Main:
  /** Dispatches to `--demo` mode or starts the interactive shell. */
  def main(args: Array[String]): Unit =
    if args.headOption.contains("--demo") then DemoMain.run()
    else runInteractiveShell()

  /** Runs a backslash meta-command, or returns `None` for ordinary SQL.
    *
    * `\\projections` lists the projections, `\\session` shows the session's consistency level and
    * token, `\\advice [log]` shows the advisor's recommendations (or its audit log), `\\actions [log]` shows what the actuator did (or its log), `\\cache [clear]` shows or empties the plan cache, `\\changes [after] [max]` lists change-log commits after commit `after` (default 0,
    * at most `max`, default 20).
    */
  private[cli] def metaCommand(db: AdaptiveDatabase, line: String): Option[io.adb.gateway.QueryResult] =
    val words = line.split("\\s+").toList
    words match
      case "\\projections" :: Nil => Some(db.showProjections())
      case "\\session" :: Nil => Some(db.showSession())
      case "\\telemetry" :: Nil => Some(db.showTelemetry())
      case "\\telemetry" :: "shapes" :: Nil => Some(db.showTelemetryShapes())
      case "\\advice" :: Nil => Some(db.showAdvice())
      case "\\advice" :: "log" :: Nil => Some(db.showAdviceLog())
      case "\\actions" :: Nil => Some(db.showActions())
      case "\\actions" :: "log" :: Nil => Some(db.showActionsLog())
      case "\\cache" :: Nil => Some(db.showPlanCache())
      case "\\cache" :: "clear" :: Nil =>
        Some(io.adb.gateway.QueryResult(Vector.empty, Vector.empty, Some(s"plan cache cleared (${db.clearPlanCache()} entries)")))
      case "\\changes" :: rest if rest.size <= 2 =>
        val after = rest.headOption.map(_.toLong).getOrElse(0L)
        val max = rest.drop(1).headOption.map(_.toInt).getOrElse(20)
        Some(db.showChanges(after, max))
      case word :: _ if word.startsWith("\\") =>
        throw new IllegalArgumentException(s"unknown command $word (\\projections, \\session, \\cache [clear], \\telemetry [shapes], \\advice [log], \\actions [log], \\changes [after] [max], \\q)")
      case _ => None

  /** Runs the interactive SQL shell until EOF or `\\q`. */
  private def runInteractiveShell(): Unit =
    val dataDir = Path.of(
      sys.env.getOrElse("ADB_DATA", sys.props.getOrElse("adb.data", "./adb-data"))
    )
    val nativeLib = Path.of(
      sys.env.getOrElse("ADB_NATIVE_LIBRARY", sys.props.getOrElse("adb.native.library", "../target/release/libadb_ffi.so"))
    )
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    val feedback = new PlannerFeedbackLog(dataDir.resolve("planner-feedback.jsonl"))
    // The engine owns the catalog; a pre-2.4.0 catalog file is imported once.
    val db = AdaptiveDatabase.open(native, Some(dataDir.resolve("catalog.properties")), feedbackLog = Some(feedback))

    println("Adaptive DB 2.5.4 shell. End statements with ';'. Type \\q to quit.")
    try
      var running = true
      while running do
        val line = StdIn.readLine("adb> ")
        if line == null || line.trim == "\\q" then running = false
        else if line.trim.nonEmpty then
          try
            val result = metaCommand(db, line.trim).getOrElse(db.execute(line))
            result.message.foreach(println)
            if result.columns.nonEmpty then
              println(result.columns.mkString(" | "))
              result.rows.foreach(row => println(row.map(v => Option(v).getOrElse("NULL")).mkString(" | ")))
          catch case e: Exception => System.err.println("ERROR: " + e.getMessage)
    finally native.close()
