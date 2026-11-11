package io.adb.bench

import java.lang.management.ManagementFactory
import java.nio.file.{Files, Path}
import scala.jdk.CollectionConverters.*
import scala.util.Try

/** The class of the CPU governor. Only `performance` gives comparable numbers. */
enum GovernorClass:
  /** `performance`: the CPU runs at its highest frequency. */
  case Performance

  /** Any other governor (`powersave`, `schedutil`, `ondemand`, ...). */
  case Other(name: String)

  /** The governor could not be read (a VM, a container, a non-Linux system). */
  case Unknown

/** Constructors of [[GovernorClass]]. */
object GovernorClass:
  /** Classifies the text of `scaling_governor`. */
  def fromGovernor(governor: Option[String]): GovernorClass =
    governor.map(_.trim) match
      case None | Some("") => Unknown
      case Some("performance") => Performance
      case Some(other) => Other(other)

/** Volatile machine state, captured before and after each family.
  *
  * @param cpuFrequencyKhz current frequency of CPU 0
  * @param maxThermalC     temperature of the hottest thermal zone in degrees Celsius
  * @param loadavg         load averages over 1, 5 and 15 minutes
  */
final case class RuntimeSnapshot(cpuFrequencyKhz: Option[Long], maxThermalC: Option[Double], loadavg: Option[Vector[Double]]):
  /** The snapshot as JSON; missing values are `null`. */
  def json: String = Json.obj(
    "cpu_frequency_khz" -> cpuFrequencyKhz.fold("null")(Json.num),
    "max_thermal_c" -> Json.numOpt(maxThermalC),
    "loadavg" -> loadavg.fold("null")(values => Json.arr(values.map(Json.num)))
  )

/** Reading the live machine or a fake tree (`capture(root)`, for tests). */
object RuntimeSnapshot:
  /** Reads the live machine. */
  def capture(): RuntimeSnapshot = capture(Path.of("/"))

  /** Reads the tree below `root`. */
  def capture(root: Path): RuntimeSnapshot =
    RuntimeSnapshot(
      Environment.readTrimmed(root, Environment.Frequency).flatMap(_.toLongOption),
      maxThermal(root),
      loadavg(root)
    )

  /** The hottest thermal zone in degrees Celsius (the kernel reports millidegrees). */
  private def maxThermal(root: Path): Option[Double] =
    val dir = root.resolve(Environment.Thermal)
    if !Files.isDirectory(dir) then None
    else
      val zones = Try(Files.list(dir).iterator().asScala.toVector).getOrElse(Vector.empty)
      val temps = zones.filter(_.getFileName.toString.startsWith("thermal_zone")).flatMap { zone =>
        Try(Files.readString(zone.resolve("temp")).trim.toDouble / 1000.0).toOption
      }
      temps.maxOption

  /** The 1, 5 and 15 minute load averages. */
  private def loadavg(root: Path): Option[Vector[Double]] =
    Environment.readTrimmed(root, Environment.LoadAvg).flatMap { text =>
      val fields = text.split("\\s+").take(3).flatMap(_.toDoubleOption).toVector
      Option.when(fields.size == 3)(fields)
    }

/** What does not change during a run: the machine, the operating system and the JVM.
  *
  * @param os              operating system name
  * @param arch            CPU architecture
  * @param kernel          kernel release
  * @param cpuModel        model name of the CPU
  * @param logicalCpus     processors available to the JVM
  * @param governor        text of `scaling_governor` of CPU 0
  * @param maxFrequencyKhz maximum frequency of CPU 0
  * @param java            `java.version`
  * @param jvm             `java.vm.name`
  * @param vendor          `java.vendor`
  * @param jvmArguments    the JVM's input arguments (heap size, GC, flags)
  * @param maxHeapMib      maximum heap in MiB
  * @param pid             process id (a fresh fork per family has a pid of its own)
  * @param freshFork       whether the launcher started this JVM for this family alone
  */
final case class Environment(
    os: String,
    arch: String,
    kernel: Option[String],
    cpuModel: Option[String],
    logicalCpus: Int,
    governor: Option[String],
    maxFrequencyKhz: Option[Long],
    java: String,
    jvm: String,
    vendor: String,
    jvmArguments: Vector[String],
    maxHeapMib: Long,
    pid: Long,
    freshFork: Boolean
):
  /** The class of the CPU governor. */
  def governorClass: GovernorClass = GovernorClass.fromGovernor(governor)

  /** The environment as JSON; missing values are `null`. The `jvm` object is the fork record. */
  def json: String = Json.obj(
    "os" -> Json.str(os),
    "arch" -> Json.str(arch),
    "kernel" -> Json.strOpt(kernel),
    "cpu_model" -> Json.strOpt(cpuModel),
    "logical_cpus" -> Json.num(logicalCpus.toLong),
    "governor" -> Json.strOpt(governor),
    "max_frequency_khz" -> maxFrequencyKhz.fold("null")(Json.num),
    "jvm" -> Json.obj(
      "java" -> Json.str(java),
      "vm" -> Json.str(jvm),
      "vendor" -> Json.str(vendor),
      "arguments" -> Json.arr(jvmArguments.map(Json.str)),
      "max_heap_mib" -> Json.num(maxHeapMib),
      "pid" -> Json.num(pid),
      "fresh_fork" -> Json.bool(freshFork)
    )
  )

/** Capturing the [[Environment]]. */
object Environment:
  /** Environment variable the launcher sets to `1` when it starts one JVM per family. */
  val ForkEnv = "ADB_BENCH_FORK"

  /** Path of the governor below the root. */
  val Governor = "sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"

  /** Path of the current frequency below the root. */
  val Frequency = "sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"

  /** Path of the maximum frequency below the root. */
  val MaxFrequency = "sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"

  /** Directory of the thermal zones below the root. */
  val Thermal = "sys/class/thermal"

  /** Path of the load averages below the root. */
  val LoadAvg = "proc/loadavg"

  /** Path of the CPU description below the root. */
  val CpuInfo = "proc/cpuinfo"

  /** Path of the kernel release below the root. */
  val Kernel = "proc/sys/kernel/osrelease"

  /** Reads the live machine and this JVM. */
  def capture(): Environment = capture(Path.of("/"))

  /** Reads the tree below `root`, and the running JVM. */
  def capture(root: Path): Environment =
    val runtime = ManagementFactory.getRuntimeMXBean
    Environment(
      os = System.getProperty("os.name"),
      arch = System.getProperty("os.arch"),
      kernel = readTrimmed(root, Kernel),
      cpuModel = cpuModel(root),
      logicalCpus = Runtime.getRuntime.availableProcessors(),
      governor = readTrimmed(root, Governor),
      maxFrequencyKhz = readTrimmed(root, MaxFrequency).flatMap(_.toLongOption),
      java = System.getProperty("java.version"),
      jvm = System.getProperty("java.vm.name"),
      vendor = System.getProperty("java.vendor"),
      jvmArguments = runtime.getInputArguments.asScala.toVector,
      maxHeapMib = Runtime.getRuntime.maxMemory() / 1048576,
      pid = ProcessHandle.current().pid(),
      freshFork = Option(System.getenv(ForkEnv)).contains("1")
    )

  /** Reads a file below `root` and trims it; `None` when it is missing or empty. */
  private[bench] def readTrimmed(root: Path, relative: String): Option[String] =
    Try(Files.readString(root.resolve(relative)).trim).toOption.filter(_.nonEmpty)

  /** The `model name` line of the CPU description. */
  private def cpuModel(root: Path): Option[String] =
    readTrimmed(root, CpuInfo).flatMap { text =>
      text.linesIterator.find(_.startsWith("model name")).flatMap(_.split(":", 2).lift(1)).map(_.trim).filter(_.nonEmpty)
    }
