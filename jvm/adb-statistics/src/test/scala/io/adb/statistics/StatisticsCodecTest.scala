package io.adb.statistics

import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Decoding of engine statistics documents and the freshness rules. */
final class StatisticsCodecTest:
  /** A document in the exact shape the engine emits. */
  private val document =
    """{"format_version":1,"entity_id":7,"row_count":100,"avg_row_bytes":24.5,"analyzed_at_ts":42,
      |"modifications_at_analyze":3,"sampled_rows":100,"exact":true,"columns":[
      |{"field_id":1,"null_count":0,"distinct_count":100,"distinct_exact":true,"min":{"Int64":1},
      | "max":{"Int64":100},"avg_width_bytes":8.0,
      | "histogram":[{"lower":{"Int64":1},"upper":{"Int64":100},"rows":100,"distinct":100}],"most_common":[]},
      |{"field_id":2,"null_count":10,"distinct_count":2,"distinct_exact":true,"min":{"String":"DE"},
      | "max":{"String":"PL"},"avg_width_bytes":2.0,"histogram":[],
      | "most_common":[{"value":{"String":"PL"},"rows":60},{"value":{"String":"DE"},"rows":30}]},
      |{"field_id":3,"null_count":100,"distinct_count":0,"distinct_exact":true,"avg_width_bytes":0.0,
      | "histogram":[],"most_common":[]}]}""".stripMargin

  /** Every field of the document is decoded. */
  @Test def decodesDocument(): Unit =
    val table = StatisticsCodec.decode(document)
    assertEquals(EntityId(7), table.entityId)
    assertEquals(100L, table.rowCount)
    assertEquals(24.5, table.avgRowBytes)
    assertEquals(3L, table.modificationsAtAnalyze)
    assertTrue(table.exact)
    val id = table.column(FieldId(1)).get
    assertEquals(Some(DbValue.Int64Value(1)), id.min)
    assertEquals(Vector(HistogramBucket(DbValue.Int64Value(1), DbValue.Int64Value(100), 100, 100)), id.histogram)
    val country = table.column(FieldId(2)).get
    assertEquals(90L, country.mostCommonRows)
    assertEquals(DbValue.StringValue("PL"), country.mostCommon.head.value)
    assertEquals(None, table.column(FieldId(3)).get.min)
    assertEquals(None, table.column(FieldId(4)))

  /** Every engine value encoding round-trips through the decoder. */
  @Test def decodesValues(): Unit =
    assertEquals(DbValue.NullValue, StatisticsCodec.value("Null"))
    assertEquals(DbValue.BoolValue(true), StatisticsCodec.value(Map("Bool" -> true)))
    assertEquals(DbValue.Float64Value(2.0), StatisticsCodec.value(Map("Float64" -> 2L)))
    assertEquals(Vector[Byte](1, 2), StatisticsCodec.value(Map("Bytes" -> Vector(1L, 2L))).asInstanceOf[DbValue.BytesValue].value.toVector)
    assertThrows(classOf[IllegalArgumentException], () => StatisticsCodec.value(Map("Bytes" -> Vector(300L))))
    assertThrows(classOf[IllegalArgumentException], () => StatisticsCodec.value(Map("Int64" -> 1L, "Bool" -> true)))

  /** Unknown versions, missing fields and impossible counts are rejected. */
  @Test def rejectsMalformedDocuments(): Unit =
    for bad <- Vector(
        document.replace("\"format_version\":1", "\"format_version\":2"),
        document.replace("\"row_count\":100,", ""),
        document.replace("\"exact\":true", "\"exact\":1"),
        document.replace("\"null_count\":10", "\"null_count\":101"),
        document.replace("\"distinct_count\":2,", "\"distinct_count\":95,"),
        document.replace("\"field_id\":3", "\"field_id\":2"),
        "[]"
      )
    do assertThrows(classOf[IllegalArgumentException], () => { StatisticsCodec.decode(bad); () }, bad)

  /** Statistics turn stale above 20% changed rows and lose half their confidence. */
  @Test def freshnessAndConfidence(): Unit =
    val table = StatisticsCodec.decode(document)
    val fresh = EntityStatistics(table, 20)
    val stale = EntityStatistics(table, 21)
    assertEquals(Freshness.Fresh, fresh.freshness())
    assertEquals(Freshness.Stale, stale.freshness())
    assertEquals(1.0, fresh.confidence())
    assertEquals(0.5, stale.confidence())
    assertEquals(0.45, EntityStatistics(table.copy(exact = false), 50).confidence())
    val provider = StatisticsProvider.of(fresh)
    assertEquals(Some(fresh), provider.statistics(EntityId(7)))
    assertEquals(None, provider.statistics(EntityId(8)))
    assertEquals(None, StatisticsProvider.Empty.statistics(EntityId(7)))
