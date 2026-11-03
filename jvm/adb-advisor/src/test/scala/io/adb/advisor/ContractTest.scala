package io.adb.advisor

import java.nio.file.{Files, Path, Paths}
import java.nio.charset.StandardCharsets
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

import scala.jdk.CollectionConverters.*

/** Freezes the 2.5.3 contract that the 2.5.4 actuator builds on: the advisor can only recommend.
  *
  * The rule is structural. No class of `io.adb.advisor` may refer to the native engine
  * (`io.adb.ffm`), to the gateway (`io.adb.gateway`) or to the actuator (`io.adb.actuator`). The test
  * reads the compiled classes and looks for those package names in their constant pools, so a
  * forbidden reference fails the build even if nothing calls it.
  */
final class ContractTest:
  /** Package names, in the internal form class files use, that the advisor must never mention. */
  private val forbidden = Vector("io/adb/ffm", "io/adb/gateway", "io/adb/actuator")

  /** Directory holding the compiled classes of the advisor package. */
  private def classesDir: Path =
    val location = classOf[Advisor].getProtectionDomain.getCodeSource.getLocation.toURI
    Paths.get(location).resolve("io/adb/advisor")

  @Test def noAdvisorClassMentionsTheEngineTheGatewayOrTheActuator(): Unit =
    val dir = classesDir
    assumeDirectory(dir)
    val files = Files.list(dir).iterator().asScala.filter(_.toString.endsWith(".class")).toVector
    assertTrue(files.nonEmpty, s"no classes found in $dir")
    for file <- files do
      // ISO-8859-1 keeps every byte as one char, so the UTF-8 names in the constant pool match.
      val text = new String(Files.readAllBytes(file), StandardCharsets.ISO_8859_1)
      for name <- forbidden do
        assertFalse(text.contains(name), s"${file.getFileName} refers to $name")

  /** The advisor stays a pure producer of reports: the entry point has no engine-shaped parameter. */
  @Test def theAdvisorEntryPointTakesOnlyImmutableValues(): Unit =
    val evaluate = classOf[Advisor].getMethods.filter(_.getName == "evaluate")
    assertTrue(evaluate.nonEmpty)
    for m <- evaluate; p <- m.getParameterTypes do
      assertFalse(p.getName.startsWith("io.adb.ffm"), p.getName)
      assertFalse(p.getName.startsWith("io.adb.gateway"), p.getName)

  /** Fails with a clear message when the classes are packed in a jar, where they cannot be listed. */
  private def assumeDirectory(dir: Path): Unit =
    if !Files.isDirectory(dir) then fail(s"advisor classes are not a directory: $dir")
