package io.adb.catalog

import io.adb.model.*
import java.nio.file.Files
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Foreign-key hints in the catalog: validation and persistence. */
final class ForeignKeyCatalogTest:
  /** Column definitions of a two-column child table. */
  private val childFields = Vector(("id", DataType.Int64, false), ("parent_id", DataType.Int64, true))

  /** A hint must target an existing entity's primary key of the same type. */
  @Test def validatesReferences(): Unit =
    val catalog = new InMemoryCatalog
    val parent = catalog.createEntity("parent", Vector(("id", DataType.Int64, false), ("label", DataType.StringType, true)), "id")
    for bad <- Vector(
        Map("parent_id" -> ForeignKeyRef(EntityId(99), FieldId(1))),
        Map("parent_id" -> ForeignKeyRef(parent.id, FieldId(2))),
        Map("missing" -> ForeignKeyRef(parent.id, parent.primaryKey))
      )
    do assertThrows(classOf[IllegalArgumentException], () => catalog.createEntity("child", childFields, "id", bad))
    assertThrows(classOf[IllegalArgumentException], () =>
      catalog.createEntity("child", Vector(("id", DataType.Int64, false), ("parent_id", DataType.StringType, true)), "id",
        Map("parent_id" -> ForeignKeyRef(parent.id, parent.primaryKey))))
    assertEquals(Vector(parent), catalog.entities, "failed definitions leave no trace")

  /** Hints survive a reload of the file catalog (case-insensitive column names). */
  @Test def persistsReferences(): Unit =
    val path = Files.createTempDirectory("adb-catalog").resolve("catalog.properties")
    val catalog = new FileCatalog(path)
    val parent = catalog.createEntity("parent", Vector(("id", DataType.Int64, false)), "id")
    val child = catalog.createEntity("child", childFields, "id", Map("PARENT_ID" -> ForeignKeyRef(parent.id, parent.primaryKey)))
    val reloaded = new FileCatalog(path)
    assertEquals(Some(child), reloaded.entity("child"))
    assertEquals(Some(ForeignKeyRef(parent.id, parent.primaryKey)), reloaded.entity("child").get.field("parent_id").get.references)
    assertEquals(None, reloaded.entity("parent").get.fields.head.references)
