package io.adb.consistency

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Decoding the engine's projection documents. */
final class ProjectionJsonCodecTest:
  /** What `adb_projection_get_json` returns for an external consumer. */
  private val consumer =
    """{"descriptor":{"id":1000000,"name":"search","kind":"TestAsync","mode":"Asynchronous","entity_id":3,"epoch":1,"supports_delta_repair":false},
      |"state":"Ready","frontier":{"base_commit_seq":41,"delta_commit_seq":41},
      |"head_commit_seq":43,"lag_commits":2,"lag_changes":5,"lag_bytes":900,"commits_per_second":12.5}""".stripMargin

  /** The engine's own projection, without an entity and with an integral rate. */
  private val currentRow =
    """{"descriptor":{"id":1,"name":"__current_row","kind":"CurrentRow","mode":"Synchronous","entity_id":null,"epoch":1,"supports_delta_repair":false},
      |"state":"Ready","frontier":{"base_commit_seq":43,"delta_commit_seq":43},
      |"head_commit_seq":43,"lag_commits":0,"lag_changes":0,"lag_bytes":0,"commits_per_second":0}""".stripMargin

  /** One document and a listing decode to the same values. */
  @Test def decodesDocuments(): Unit =
    val info = ProjectionJsonCodec.decode(consumer)
    assertEquals("search", info.name)
    assertEquals(ProjectionKind.TestAsync, info.kind)
    assertEquals(ProjectionMode.Asynchronous, info.mode)
    assertEquals(Some(3L), info.entityId)
    assertEquals(ProjectionFrontier(41, 41), info.frontier)
    assertEquals((43L, 2L, 5L, 900L), (info.headCommitSeq, info.lagCommits, info.lagChanges, info.lagBytes))
    assertEquals(12.5, info.commitsPerSecond, 0.0)
    val listing = ProjectionJsonCodec.decodeList(s"[$currentRow,$consumer]")
    assertEquals(Vector("__current_row", "search"), listing.map(_.name))
    assertEquals(None, listing.head.entityId)
    assertEquals(0.0, listing.head.commitsPerSecond, 0.0)
    assertEquals(info, listing(1))

  /** Malformed documents are refused with a clear error, never half-decoded. */
  @Test def rejectsMalformedDocuments(): Unit =
    def bad(json: String): Unit = assertThrows(classOf[IllegalArgumentException], () => ProjectionJsonCodec.decode(json))
    bad("[]")
    bad("{}")
    bad(consumer.replace("\"Ready\"", "\"Sleeping\""))
    bad(consumer.replace("\"TestAsync\"", "1"))
    bad(consumer.replace("\"base_commit_seq\":41", "\"base_commit_seq\":-1"))
    bad(consumer.replace("12.5", "\"fast\""))
    bad(consumer.replace("\"search\"", "\"\""))
    assertThrows(classOf[IllegalArgumentException], () => ProjectionJsonCodec.decodeList("{}"))
