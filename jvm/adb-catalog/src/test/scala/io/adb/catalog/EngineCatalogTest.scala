package io.adb.catalog

import io.adb.model.*
import io.adb.schema.*
import java.nio.file.Files
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The engine-owned catalog: DDL through the engine, cached views, reloads and refusals. */
final class EngineCatalogTest:
  /** Columns of the `account` table. */
  private val accountFields = Vector(("id", DataType.Int64, false), ("balance", DataType.Int64, false))

  /** CREATE, ALTER and DROP go to the engine; the views follow every change. */
  @Test def ddlGoesThroughTheEngine(): Unit =
    val backend = new FakeSchemaBackend
    val catalog = new EngineCatalog(backend)
    val account = catalog.createEntity("account", accountFields, "id")
    assertEquals(EntityId(1), account.id)
    assertEquals(SchemaEpoch(1), catalog.epoch)
    assertEquals(SchemaVersion(1), catalog.version)

    val v2 = catalog.alterEntity(account.id, SchemaChange.AddField(NewField("currency", DataType.StringType, false, Some(DbValue.StringValue("EUR")))))
    assertEquals(SchemaVersion(2), v2.version)
    val currency = catalog.entity("ACCOUNT").get.field("currency").get
    assertEquals(Some(DbValue.StringValue("EUR")), currency.default)
    assertEquals(SchemaVersion(2), currency.introducedIn)

    catalog.alterEntity(account.id, SchemaChange.RenameField("balance", "amount"))
    assertEquals(Vector("id", "amount", "currency"), catalog.entity(account.id).get.fields.map(_.name))
    assertEquals(Vector(SchemaVersion(2), SchemaVersion(3)), catalog.history(account.id).changes.map(_._1))

    catalog.dropEntity(account.id)
    assertEquals(None, catalog.entity("account"))
    assertEquals(SchemaEpoch(4), catalog.epoch)
    assertEquals(3, catalog.history(account.id).versions.size, "history outlives the table")

  /** Changes another client made are picked up by `ensureCurrent`, and only then. */
  @Test def reloadsWhenTheEpochMoves(): Unit =
    val backend = new FakeSchemaBackend
    val catalog = new EngineCatalog(backend)
    catalog.createEntity("account", accountFields, "id")
    val listings = backend.listings
    assertEquals(SchemaEpoch(1), catalog.ensureCurrent())
    assertEquals(listings, backend.listings, "an unchanged epoch costs no listing")
    backend.renameBehindTheBack(1, "balance", "amount")
    assertTrue(catalog.entity("account").get.field("balance").isDefined, "the cache is stale until checked")
    assertEquals(SchemaEpoch(2), catalog.ensureCurrent())
    assertTrue(catalog.entity("account").get.field("amount").isDefined)

  /** Requests the engine would refuse are refused before the round trip, with SQL-level messages. */
  @Test def refusesBeforeTheRoundTrip(): Unit =
    val backend = new FakeSchemaBackend
    val index = IndexDependency(IndexId(1), "idx_balance", EntityId(1), Vector(FieldId(2)), Vector.empty)
    var indexes = Vector(index)
    val catalog = new EngineCatalog(backend, () => indexes)
    catalog.createEntity("account", accountFields, "id")
    val sent = backend.requests.size
    val drop = assertThrows(classOf[IllegalArgumentException], () => catalog.alterEntity(EntityId(1), SchemaChange.DropField("balance")))
    assertTrue(drop.getMessage.contains("index idx_balance"), drop.getMessage)
    val table = assertThrows(classOf[IllegalArgumentException], () => catalog.dropEntity(EntityId(1)))
    assertTrue(table.getMessage.contains("index idx_balance"), table.getMessage)
    assertThrows(classOf[IllegalArgumentException], () => catalog.alterEntity(EntityId(9), SchemaChange.DropField("x")))
    assertThrows(classOf[IllegalArgumentException], () => catalog.createEntity("bad", Vector(("id", DataType.StringType, false)), "id"))
    assertEquals(sent, backend.requests.size, "nothing was sent")
    indexes = Vector.empty
    catalog.alterEntity(EntityId(1), SchemaChange.DropField("balance"))
    assertEquals(Vector("id"), catalog.entity("account").get.fields.map(_.name))

  /** Foreign-key hints given by column name travel to the engine. */
  @Test def sendsForeignKeyHints(): Unit =
    val backend = new FakeSchemaBackend
    val catalog = new EngineCatalog(backend)
    val parent = catalog.createEntity("parent", accountFields, "id")
    val child = catalog.createEntity(
      "child",
      Vector(("id", DataType.Int64, false), ("parent_id", DataType.Int64, true)),
      "id",
      Map("PARENT_ID" -> ForeignKeyRef(parent.id, parent.primaryKey))
    )
    assertEquals(Some(ForeignKeyRef(parent.id, parent.primaryKey)), child.field("parent_id").get.references)

  /** A 2.3.0 file catalog is imported with its ids; a second run skips what exists. */
  @Test def importsLegacyCatalogs(): Unit =
    val dir = Files.createTempDirectory("adb-import")
    val legacy = new FileCatalog(dir.resolve("catalog.properties"))
    val parent = legacy.createEntity("parent", accountFields, "id")
    legacy.createEntity("gap", accountFields, "id")
    val child = legacy.createEntity(
      "child",
      Vector(("id", DataType.Int64, false), ("parent_id", DataType.Int64, true)),
      "id",
      Map("parent_id" -> ForeignKeyRef(parent.id, parent.primaryKey))
    )
    val backend = new FakeSchemaBackend
    val catalog = new EngineCatalog(backend)
    val report = CatalogImport.importInto(catalog, new FileCatalog(dir.resolve("catalog.properties")))
    assertEquals(Vector("parent", "gap", "child"), report.imported.map(_.name))
    assertEquals(legacy.entities.map(e => e.id -> e.fields.map(f => (f.id, f.name, f.dataType, f.nullable, f.references))),
      catalog.entities.map(e => e.id -> e.fields.map(f => (f.id, f.name, f.dataType, f.nullable, f.references))))
    assertTrue(backend.requests.forall(_.contains("\"entity_id\":")), "ids are explicit")

    catalog.alterEntity(child.id, SchemaChange.RenameField("parent_id", "owner_id"))
    val again = CatalogImport.importInto(catalog, legacy)
    assertTrue(again.imported.isEmpty)
    assertEquals(3, again.skipped.size)

    val clash = new FakeSchemaBackend
    val other = new EngineCatalog(clash)
    other.createEntity("parent", accountFields, "id")
    other.createEntity("intruder", accountFields, "id")
    assertThrows(classOf[IllegalStateException], () => CatalogImport.importInto(other, legacy))
