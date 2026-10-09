package io.adb.cli

import io.adb.catalog.FileCatalog
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

  /** Runs the interactive SQL shell until EOF or `\\q`. */
  private def runInteractiveShell(): Unit =
    val dataDir = Path.of(
      sys.env.getOrElse("ADB_DATA", sys.props.getOrElse("adb.data", "./adb-data"))
    )
    val nativeLib = Path.of(
      sys.env.getOrElse("ADB_NATIVE_LIBRARY", sys.props.getOrElse("adb.native.library", "../target/release/libadb_ffi.so"))
    )
    val catalog = new FileCatalog(dataDir.resolve("catalog.properties"))
    val native = NativeDatabase.open(nativeLib, dataDir.resolve("rust"))
    val feedback = new PlannerFeedbackLog(dataDir.resolve("planner-feedback.jsonl"))
    val db = new AdaptiveDatabase(catalog, native, feedbackLog = Some(feedback))

    println("Adaptive DB 2.2.3 shell. End statements with ';'. Type \\q to quit.")
    try
      var running = true
      while running do
        val line = StdIn.readLine("adb> ")
        if line == null || line.trim == "\\q" then running = false
        else if line.trim.nonEmpty then
          try
            val result = db.execute(line)
            result.message.foreach(println)
            if result.columns.nonEmpty then
              println(result.columns.mkString(" | "))
              result.rows.foreach(row => println(row.map(v => Option(v).getOrElse("NULL")).mkString(" | ")))
          catch case e: Exception => System.err.println("ERROR: " + e.getMessage)
    finally native.close()
