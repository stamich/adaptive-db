package io.adb.benchmark

import java.nio.file.{Files, Path}
import scala.annotation.tailrec

/** JVM benchmark (Milestone 2.5.4.1), organised in **families**: `sql-path` ([[SqlPathFamily]]),
  * `telemetry` ([[TelemetryFamily]]), `actuator` ([[ActuatorFamily]]), `ffi` ([[FfiFamily]]) and
  * `workloads` ([[WorkloadBenchmark]], the 2.3.0 workloads on the engine catalog since 2.4.0).
  *
  * Every family writes one schema-3 report (`--out`) and is meant to run in a JVM of its own: the
  * launcher `scripts/benchmark.sh` starts one JVM per family and sets `ADB_BENCH_FORK=1`, so the JIT
  * profile of one family cannot colour another and the report records `fresh_fork`. `--family all`
  * runs the four non-workload families in one JVM for a quick look; the report of each then says
  * `fresh_fork: false`.
  */
object BenchmarkMain:
  /** Command-line options.
    *
    * @param family   the family to run (`sql-path`, `telemetry`, `actuator`, `ffi`, `workloads`, `all`)
    * @param dataDir  database directory (the gateway families and the workloads need it)
    * @param nativeLib path of the native library
    * @param out      where the report is written (for `all`, a directory)
    * @param scale    scale factor the workload data was prepared with
    */
  private final case class Config(
      family: String = "all",
      dataDir: Option[Path] = None,
      nativeLib: Option[Path] = None,
      out: Option[Path] = None,
      scale: Int = 1
  )

  /** The families that need no prepared dataset. */
  private val Families = Vector("sql-path", "telemetry")

  /** Runs the chosen family. */
  def main(args: Array[String]): Unit =
    val cfg = parseArgs(args.toList, Config())
    cfg.family match
      case "workloads" =>
        (cfg.dataDir, cfg.nativeLib) match
          case (Some(data), Some(lib)) =>
            WorkloadBenchmark.run(WorkloadBenchmark.Settings(data, lib, 0, cfg.scale, 0L, cfg.out))
          case _ => throw new IllegalArgumentException("--family workloads needs --data DIR and --native-lib FILE")
      case "all" => Families.foreach(family => runFamily(family, cfg, cfg.out.map(_.resolve(s"$family.json"))))
      case family if Families.contains(family) => runFamily(family, cfg, cfg.out)
      case other => throw new IllegalArgumentException(s"unknown family: $other (one of ${Families.mkString(", ")}, workloads, all)")

  /** Runs one non-workload family and prints a one-line summary. */
  private def runFamily(family: String, cfg: Config, out: Option[Path]): Unit =
    out.flatMap(path => Option(path.getParent)).foreach(Files.createDirectories(_))
    val settings = FamilySettings(cfg.dataDir, cfg.nativeLib, out)
    val text = family match
      case "sql-path" => SqlPathFamily.run(settings)
      case "telemetry" => TelemetryFamily.run(settings)
      case _ => SqlPathFamily.run(settings)
    println(s"family $family: ${text.length} bytes${out.fold("")(p => s" -> $p")}")

  /** Parses `--family`, `--data`, `--native-lib`, `--out` and `--scale`. The pre-2.5.4.1 options
    * `--iterations`, `--gateway-iterations` and `--warmup-ms` are accepted and ignored (the
    * measurement plan decides), and `--workloads` means `--family workloads`.
    */
  @tailrec
  private def parseArgs(args: List[String], cfg: Config): Config = args match
    case Nil => cfg
    case "--family" :: value :: tail => parseArgs(tail, cfg.copy(family = value))
    case "--workloads" :: tail => parseArgs(tail, cfg.copy(family = "workloads"))
    case ("--iterations" | "--gateway-iterations" | "--warmup-ms") :: _ :: tail => parseArgs(tail, cfg)
    case "--data" :: value :: tail => parseArgs(tail, cfg.copy(dataDir = Some(Path.of(value).toAbsolutePath.normalize())))
    case "--native-lib" :: value :: tail => parseArgs(tail, cfg.copy(nativeLib = Some(Path.of(value).toAbsolutePath.normalize())))
    case "--scale" :: value :: tail => parseArgs(tail, cfg.copy(scale = value.toInt))
    case "--out" :: value :: tail => parseArgs(tail, cfg.copy(out = Some(Path.of(value).toAbsolutePath.normalize())))
    case other :: _ => throw new IllegalArgumentException(s"unknown/incomplete argument: $other")
