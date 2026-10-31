package io.adb.schema

import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Dependencies between tables, columns and indexes, and the RESTRICT messages built from them. */
final class DependencyGraphTest:
  /** A `transfer` table whose `account_id` references `account(id)`. */
  private val transfer = EntitySchema(
    EntityId(4),
    "transfer",
    SchemaVersion(1),
    None,
    SchemaEpoch(6),
    FieldId(1),
    FieldId(3),
    Vector(
      FieldDefinition(FieldId(1), "id", DataType.Int64, false, None, SchemaVersion(1), None, None),
      FieldDefinition(FieldId(2), "account_id", DataType.Int64, false, None, SchemaVersion(1), None, Some(ForeignKeyRef(EntityId(3), FieldId(1))))
    )
  )
  /** An index on `transfer(account_id)`. */
  private val index = IndexDependency(IndexId(7), "idx_transfer_account", EntityId(4), Vector(FieldId(2)), Vector.empty)
  /** The graph of both tables and the index. */
  private val graph = DependencyGraph.build(Vector(SchemaFixtures.V3, transfer), Vector(index))

  /** Columns report their dependents by kind. */
  @Test def findsColumnDependents(): Unit =
    val onAccountId = graph.dependents(CatalogObjectId.Field(EntityId(4), FieldId(2)))
    assertEquals(Vector(Dependency(CatalogObjectId.Index(IndexId(7)), CatalogObjectId.Field(EntityId(4), FieldId(2)), DependencyKind.IndexKey)), onAccountId)
    val onAccountPk = graph.dependents(CatalogObjectId.Field(EntityId(3), FieldId(1))).map(_.kind).toSet
    assertEquals(Set(DependencyKind.PrimaryKey, DependencyKind.ForeignKey), onAccountPk)
    assertTrue(graph.dependents(CatalogObjectId.Field(EntityId(3), FieldId(2))).isEmpty)

  /** Tables report indexes and foreign-key hints of other tables, not their own primary key. */
  @Test def findsTableDependents(): Unit =
    assertEquals(
      Some("cannot drop table account: column transfer.account_id (foreign-key hint) depends on it"),
      graph.dropRefusal(CatalogObjectId.Entity(EntityId(3)))
    )
    assertEquals(
      Some("cannot drop table transfer: index idx_transfer_account (index of the table) depends on it"),
      graph.dropRefusal(CatalogObjectId.Entity(EntityId(4)))
    )
    val alone = DependencyGraph.build(Vector(SchemaFixtures.V3), Vector.empty)
    assertEquals(None, alone.dropRefusal(CatalogObjectId.Entity(EntityId(3))))

  /** Objects are described by name, with ids for unknown ones. */
  @Test def describesObjects(): Unit =
    assertEquals("column account.balance", graph.describe(CatalogObjectId.Field(EntityId(3), FieldId(2))))
    assertEquals("index idx_transfer_account", graph.describe(CatalogObjectId.Index(IndexId(7))))
    assertEquals("table #99", graph.describe(CatalogObjectId.Entity(EntityId(99))))
