package io.adb.statistics

import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Statistics of columns added after ANALYZE are derived from their defaults. */
final class SchemaStatisticsTest:
  /** account(id, balance) at version 3 with `currency` (v2, DEFAULT 'EUR') and `note` (v3, nullable). */
  private val account = Entity(
    EntityId(3),
    "account",
    Vector(
      Field(FieldId(1), "id", DataType.Int64, false),
      Field(FieldId(2), "balance", DataType.Int64, false),
      Field(FieldId(4), "currency", DataType.StringType, false, default = Some(DbValue.StringValue("EUR")), introducedIn = SchemaVersion(2)),
      Field(FieldId(5), "note", DataType.StringType, true, introducedIn = SchemaVersion(3))
    ),
    FieldId(1),
    SchemaVersion(3)
  )
  /** Column statistics of `balance`. */
  private val balance = ColumnStatistics(FieldId(2), 0, 100, true, Some(DbValue.Int64Value(0)), Some(DbValue.Int64Value(99)), 8, Vector.empty, Vector.empty)

  /** Statistics analyzed at `version` with only `balance` described. */
  private def analyzedAt(version: Long): EntityStatistics =
    EntityStatistics(TableStatistics(EntityId(3), 1000, 24, 7, 0, 1000, true, Map(FieldId(2) -> balance), schemaVersion = version), 0)

  /** Columns added after the analyzed version get exact statistics from their default. */
  @Test def derivesColumnsFromDefaults(): Unit =
    val adjusted = SchemaStatistics.adjust(analyzedAt(1), account).table
    val currency = adjusted.column(FieldId(4)).get
    assertEquals(0L, currency.nullCount)
    assertEquals(1L, currency.distinctCount)
    assertEquals(Some(DbValue.StringValue("EUR")), currency.min)
    assertEquals(Vector(MostCommonValue(DbValue.StringValue("EUR"), 1000)), currency.mostCommon)
    assertEquals(3.0, currency.avgWidthBytes, 0.0)
    val note = adjusted.column(FieldId(5)).get
    assertEquals(1000L, note.nullCount, "no default: every analyzed row reads NULL")
    assertEquals(0L, note.distinctCount)
    assertEquals(Some(balance), adjusted.column(FieldId(2)), "analyzed columns are untouched")

  /** Only columns newer than the analyzed version, and without statistics, are derived. */
  @Test def onlyColumnsAddedAfterAnalyze(): Unit =
    assertEquals(Vector("note"), SchemaStatistics.addedAfterAnalyze(analyzedAt(2).table, account).map(_.field.name))
    assertTrue(SchemaStatistics.addedAfterAnalyze(analyzedAt(3).table, account).isEmpty)
    assertEquals(Vector("currency", "note"), SchemaStatistics.addedAfterAnalyze(analyzedAt(0).table, account).map(_.field.name),
      "version 0 (analyzed without a schema) matches version 1")
    val unchanged = analyzedAt(3)
    assertTrue(SchemaStatistics.adjust(unchanged, account) eq unchanged)

  /** The provider adjusts per entity and passes unknown entities through. */
  @Test def providerAdjustsByEntity(): Unit =
    val provider = SchemaStatistics.provider(StatisticsProvider.of(analyzedAt(1)), id => Option.when(id == account.id)(account))
    assertTrue(provider.statistics(EntityId(3)).get.table.column(FieldId(4)).isDefined)
    assertEquals(None, provider.statistics(EntityId(9)))
    val noSchema = SchemaStatistics.provider(StatisticsProvider.of(analyzedAt(1)), _ => None)
    assertTrue(noSchema.statistics(EntityId(3)).get.table.column(FieldId(4)).isEmpty)
    assertEquals(
      "currency added in version 2, after ANALYZE (version 1): every analyzed row reads its DEFAULT 'EUR'",
      SchemaStatistics.addedAfterAnalyze(analyzedAt(1).table, account).head.display
    )
