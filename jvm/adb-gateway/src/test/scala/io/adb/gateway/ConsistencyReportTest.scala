package io.adb.gateway

import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.consistency.*
import io.adb.model.*
import io.adb.optimizer.access.IndexDecision
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The renderings behind EXPLAIN's read context and the shell's meta-commands. */
final class ConsistencyReportTest:
  /** A projection at `at` with a head of 20. */
  private def projection(at: Long) = ProjectionInfo(1001, "index:by_region", ProjectionKind.SecondaryIndex, ProjectionMode.Asynchronous,
    Some(1), 1, ProjectionState.Ready, ProjectionFrontier(at, at), 20, 20 - at, 0, 0, 12.5)
  /** An index. */
  private val index = IndexInfo(IndexId(1), "by_region", EntityId(1), Vector(IndexKeyColumn(FieldId(2), false)), Vector.empty, 1, 1, 10)
  /** A context at head 20. */
  private val context = SemanticReadContext(ReadConsistency.ReadYourWrites, SessionConsistencyToken(18), 20, 2000)

  /** The read-context section names the level and every index decision. */
  @Test def explainSection(): Unit =
    val text = ConsistencyReport.explain(context, Vector(
      IndexDecision(index, Some(projection(20)), FreshnessResolution.Direct),
      IndexDecision(index.copy(name = "other"), Some(projection(10)), FreshnessResolution.Fallback("projection is behind commit 18")),
      IndexDecision(index.copy(name = "plain"), None, FreshnessResolution.Direct)))
    val lines = text.linesIterator.toVector
    assertEquals("Read context:", lines.head)
    assertEquals("- consistency=READ_YOUR_WRITES token=18 head=20 max wait 2000 ms", lines(1))
    assertEquals("- index by_region: usable (projection index:by_region Ready, at 20, lag 0)", lines(2))
    assertEquals("- index other: not used: projection is behind commit 18 (projection index:by_region Ready, at 10, lag 10)", lines(3))
    assertEquals("- index plain: usable (synchronous (no projection report))", lines(4))

  /** Resolutions read as words. */
  @Test def describesResolutions(): Unit =
    assertEquals("usable after waiting for commit 5 (about 30 ms)", ConsistencyReport.describe(FreshnessResolution.WaitUntil(5, Some(30))))
    assertEquals("usable after waiting for commit 5", ConsistencyReport.describe(FreshnessResolution.WaitUntil(5, None)))
    assertEquals("usable, 3 commits behind is allowed", ConsistencyReport.describe(FreshnessResolution.AllowStale(3)))

  /** Rows for projections and the session. */
  @Test def tables(): Unit =
    val rows = ConsistencyReport.projectionRows(Vector(projection(15)))
    assertEquals(ConsistencyReport.ProjectionColumns.size, rows.head.size)
    assertEquals(Vector[Any](1001L, "index:by_region", "SecondaryIndex", "Asynchronous", "Ready", 15L, 20L, 5L, "12.5"), rows.head)
    assertEquals(Vector(Vector("consistency", "READ_YOUR_WRITES"), Vector("token", 18L), Vector("head", 20L), Vector("max_wait_ms", 2000L)),
      ConsistencyReport.sessionRows(context))

  /** A change-cursor batch becomes one row per commit. */
  @Test def changeRows(): Unit =
    val json = """{"first_commit_seq":"5","last_commit_seq":"6","bytes":412,"commits":[
      |{"commit_seq":"5","commit_ts":"5","schema_epoch":"2","tx_id":"9","kind":"data","changes":[{"kind":"put"},{"kind":"put"}]},
      |{"commit_seq":"6","commit_ts":"6","schema_epoch":"3","tx_id":"10","kind":"schema","changes":[{"kind":"schema"}]}]}""".stripMargin
    assertEquals(Vector(Vector[Any]("5", "5", "2", "data", 2), Vector[Any]("6", "6", "3", "schema", 1)), ConsistencyReport.changeRows(json))
    assertEquals(Vector.empty, ConsistencyReport.changeRows("""{"commits":[]}"""))
    for bad <- Seq("[]", "{}", """{"commits":[1]}""", """{"commits":[{"commit_seq":5}]}""") do
      assertThrows(classOf[IllegalArgumentException], () => { ConsistencyReport.changeRows(bad); () }, bad)
