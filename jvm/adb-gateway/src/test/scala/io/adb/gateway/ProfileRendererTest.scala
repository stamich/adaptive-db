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
