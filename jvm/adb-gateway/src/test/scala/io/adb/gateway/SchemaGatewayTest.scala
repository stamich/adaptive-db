package io.adb.gateway

import io.adb.catalog.{Catalog, IndexInfo, IndexKeyColumn}
import io.adb.logical.{Binder, BoundSelect, OptimizerMode}
import io.adb.model.*
import io.adb.optimizer.OptimizerConfig
import io.adb.physical.PhysicalPlan
import io.adb.schema.*
import io.adb.sql.SqlParser
import io.adb.statistics.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Gateway pieces of schema evolution that need no native library: SHOW SCHEMA rows and the
  * index dependencies handed to the engine catalog.
  */
final class SchemaGatewayTest:
  /** Version 1 of `account(id, balance)`. */
  private val v1 = EntitySchema(
    EntityId(3), "account", SchemaVersion(1), None, SchemaEpoch(1), FieldId(1), FieldId(3),
    Vector(
      FieldDefinition(FieldId(1), "id", DataType.Int64, false, None, SchemaVersion(1), None, None),
      FieldDefinition(FieldId(2), "balance", DataType.Int64, false, None, SchemaVersion(1), None, None)
    )
  )
  /** Version 2: `balance` renamed to `amount`. */
  private val v2 = v1.copy(
    version = SchemaVersion(2),
    parent = Some(SchemaVersion(1)),
    epoch = SchemaEpoch(4),
    fields = v1.fields.map(f => if f.id == FieldId(2) then f.copy(name = "amount") else f)
  )

  /** One row per version: number, epoch, change and columns. */
  @Test def showSchemaRows(): Unit =
    val rows = AdaptiveDatabase.describe(SchemaHistory(EntityId(3), Vector(v1, v2)))
    assertEquals(
      Vector(
        Vector[Any](1L, 1L, "CREATE TABLE", "id BIGINT NOT NULL PRIMARY KEY, balance BIGINT NOT NULL"),
        Vector[Any](2L, 4L, "RENAME COLUMN balance TO amount", "id BIGINT NOT NULL PRIMARY KEY, amount BIGINT NOT NULL")
      ),
      rows
    )

  /** A predicate on a column added after ANALYZE is estimated from its default, and EXPLAIN
    * says so in its Schema section.
    */
  @Test def columnsAddedAfterAnalyzeAreEstimatedFromDefaults(): Unit =
    val v3 = v2.copy(
      version = SchemaVersion(3),
      parent = Some(SchemaVersion(2)),
      epoch = SchemaEpoch(5),
      nextFieldId = FieldId(4),
      fields = v2.fields :+ FieldDefinition(FieldId(3), "currency", DataType.StringType, false, Some(DbValue.StringValue("EUR")), SchemaVersion(3), None, None)
    )
    val entity = v3.toEntity
    val catalog = new Catalog:
      def version = SchemaVersion(5)
      def entity(name: String) = Option.when(name.equalsIgnoreCase("account"))(v3.toEntity)
      def entity(id: EntityId) = Option.when(id == v3.entityId)(v3.toEntity)
      def entities = Vector(v3.toEntity)
      def createEntity(name: String, fields: Vector[(String, DataType, Boolean)], primaryKey: String, references: Map[String, ForeignKeyRef]) =
        throw new UnsupportedOperationException
    val stats = StatisticsProvider.of(EntityStatistics(TableStatistics(entity.id, 1000, 24, 7, 0, 1000, true, Map.empty, schemaVersion = 2), 0))
    def estimate(sql: String): AdaptiveDatabase.Plans =
      val select = new Binder(catalog).bind(new SqlParser().parse(sql)).asInstanceOf[BoundSelect]
      AdaptiveDatabase.plan(select, catalog, stats, OptimizerConfig.Default, OptimizerMode.Cost).copy(schemaEpoch = 5)
    val matching = estimate("SELECT id FROM account WHERE currency = 'EUR'")
    val filter = PhysicalPlan.preorder(matching.physical.plan).indexWhere(_.isInstanceOf[PhysicalPlan.Filter])
    assertEquals(1000.0, matching.physical.estimates(filter).rows, 1.0, "every analyzed row reads the default")
    val other = estimate("SELECT id FROM account WHERE currency = 'PLN'")
    assertTrue(other.physical.estimates(filter).rows < 10.0, s"${other.physical.estimates(filter)}")
    val text = AdaptiveDatabase.explain(matching).mkString("\n\n")
    assertTrue(
      text.contains("Schema (catalog epoch 5):\n- account: schema version 3\n  - currency added in version 3, after ANALYZE (version 2): every analyzed row reads its DEFAULT 'EUR'"),
      text
    )

  /** An index depends on its key and covered fields. */
  @Test def indexDependencies(): Unit =
    val index = IndexInfo(IndexId(5), "idx", EntityId(3), Vector(IndexKeyColumn(FieldId(2), true)), Vector(FieldId(4)), 1, 1, 9)
    assertEquals(IndexDependency(IndexId(5), "idx", EntityId(3), Vector(FieldId(2)), Vector(FieldId(4))), AdaptiveDatabase.dependency(index))
