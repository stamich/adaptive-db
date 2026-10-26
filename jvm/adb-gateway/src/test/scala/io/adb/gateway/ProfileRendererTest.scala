package io.adb.gateway

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the EXPLAIN ANALYZE profile rendering. */
final class ProfileRendererTest:
  /** The profile renders as an indented operator tree with sorted counters. */
  @Test def rendersOperatorTree(): Unit =
    val json =
      """{"peak_memory_bytes":2048,"memory_limit_bytes":268435456,"root":{"operator":"hash_join",
        |"rows_out":5,"batches_out":1,"elapsed_us":1500,"counters":{"probe_rows":4,"build_rows":3},
        |"children":[{"operator":"entity_scan","rows_out":4,"batches_out":1,"elapsed_us":10,"counters":{"pages":2}}]}}""".stripMargin
    assertEquals(
      "peak memory 2.0 KiB of 256.0 MiB\n" +
        "hash_join rows=5 batches=1 time=1.50ms build_rows=3 probe_rows=4\n" +
        "  entity_scan rows=4 batches=1 time=0.01ms pages=2",
      ProfileRenderer.render(json)
    )

  /** With estimates, operators show `[id]`, `est` and the q-error, plus the worst q-error. */
  @Test def comparesEstimatesWithActuals(): Unit =
    val json =
      """{"peak_memory_bytes":0,"memory_limit_bytes":1024,"root":{"node_id":0,"operator":"hash_join","rows_out":40,
        |"batches_out":1,"elapsed_us":0,"children":[{"node_id":1,"operator":"entity_scan","rows_out":4,"batches_out":1,"elapsed_us":0},
        |{"node_id":2,"operator":"entity_scan","rows_out":0,"batches_out":0,"elapsed_us":0}]}}""".stripMargin
    val estimates = Map(
      0 -> io.adb.physical.NodeEstimate(10, 5, 0.8, "statistics"),
      1 -> io.adb.physical.NodeEstimate(4, 1, 1.0, "statistics"),
      2 -> io.adb.physical.NodeEstimate(0.4, 1, 1.0, "statistics")
    )
    assertEquals(
      "peak memory 0 B of 1.0 KiB\n" +
        "[0] hash_join rows=40 est=10 q=4.0 batches=1 time=0.00ms\n" +
        "  [1] entity_scan rows=4 est=4 q=1.0 batches=1 time=0.00ms\n" +
        "  [2] entity_scan rows=0 est=0 q=1.0 batches=0 time=0.00ms\n" +
        "max q-error 4.0 at [0] hash_join",
      ProfileRenderer.render(json, estimates)
    )
    assertEquals(Vector(4.0, 1.0, 1.0), ProfileRenderer.compare(json, estimates).map(_.qError))
    assertEquals(2.5, ProfileRenderer.qError(10, 25))

  /** Index operators add a summary line: pages, entries examined versus returned, row fetches. */
  @Test def summarizesIndexAccess(): Unit =
    val json =
      """{"peak_memory_bytes":0,"memory_limit_bytes":1024,"root":{"operator":"row_fetch","rows_out":25,"batches_out":1,
        |"elapsed_us":0,"counters":{"fetch_batches":1,"rows_fetched":25},"children":[{"operator":"index_scan","rows_out":25,
        |"batches_out":1,"elapsed_us":0,"counters":{"pages":2,"entries_examined":40}}]}}""".stripMargin
    assertTrue(ProfileRenderer.render(json).endsWith(
      "index access: 2 pages, 40 entries examined, 25 returned (63% visible); row fetch: 1 batches, 25 rows"), ProfileRenderer.render(json))
    val covering =
      """{"peak_memory_bytes":0,"memory_limit_bytes":1024,"root":{"operator":"index_scan","rows_out":0,"batches_out":0,
        |"elapsed_us":0,"counters":{"pages":1,"entries_examined":0}}}""".stripMargin
    assertTrue(ProfileRenderer.render(covering).endsWith("0 returned (100% visible); covering (no row fetch)"))
    val plain = """{"peak_memory_bytes":0,"memory_limit_bytes":1024,"root":{"operator":"entity_scan","rows_out":0,"batches_out":0,"elapsed_us":0}}"""
    assertFalse(ProfileRenderer.render(plain).contains("index access"))

