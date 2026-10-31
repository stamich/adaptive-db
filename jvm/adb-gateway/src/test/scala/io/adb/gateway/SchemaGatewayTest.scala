package io.adb.gateway

import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.model.*
import io.adb.schema.*
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

  /** An index depends on its key and covered fields. */
  @Test def indexDependencies(): Unit =
    val index = IndexInfo(IndexId(5), "idx", EntityId(3), Vector(IndexKeyColumn(FieldId(2), true)), Vector(FieldId(4)), 1, 1, 9)
    assertEquals(IndexDependency(IndexId(5), "idx", EntityId(3), Vector(FieldId(2)), Vector(FieldId(4))), AdaptiveDatabase.dependency(index))
