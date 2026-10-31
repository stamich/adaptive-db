package io.adb.logical

import io.adb.catalog.InMemoryCatalog
import io.adb.model.*
import io.adb.schema.{NewField, SchemaChange}
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Binding of the schema statements (2.4.0) and of defaults. */
final class SchemaBinderTest:
  /** A catalog with `account(id, balance)` and `customer(id)`. */
  private val catalog =
    val c = new InMemoryCatalog
    c.createEntity("customer", Vector(("id", DataType.Int64, false)), "id")
    c.createEntity("account", Vector(("id", DataType.Int64, false), ("balance", DataType.Int64, false)), "id")
    c
  /** Binder under test. */
  private val binder = new Binder(catalog)
  /** Parser for the statements. */
  private val parser = new SqlParser
  /** Parses and binds `sql`. */
  private def bind(sql: String): BoundStatement = binder.bind(parser.parse(sql))

  /** ALTER TABLE binds to the engine's change form, with typed defaults and resolved names. */
  @Test def bindsAlterTable(): Unit =
    val account = catalog.entity("account").get
    assertEquals(
      BoundAlterTable(account, SchemaChange.AddField(NewField("currency", DataType.StringType, false, Some(DbValue.StringValue("EUR"))))),
      bind("ALTER TABLE ACCOUNT ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR'")
    )
    val customer = catalog.entity("customer").get
    assertEquals(
      BoundAlterTable(account, SchemaChange.AddField(NewField("customer_id", DataType.Int64, true, None, Some(ForeignKeyRef(customer.id, customer.primaryKey))))),
      bind("ALTER TABLE account ADD customer_id BIGINT DEFAULT NULL REFERENCES customer(id) NOT ENFORCED")
    )
    assertEquals(BoundAlterTable(account, SchemaChange.RenameField("balance", "amount")), bind("ALTER TABLE account RENAME COLUMN BALANCE TO amount"))
    assertEquals(BoundAlterTable(account, SchemaChange.DropField("balance")), bind("ALTER TABLE account DROP COLUMN Balance"))
    assertEquals(BoundDropTable(account), bind("DROP TABLE account"))
    assertEquals(BoundShowSchema(account), bind("SHOW SCHEMA account"))

  /** Defaults must be literals of the column's type; NOT NULL additions need one. */
  @Test def checksDefaults(): Unit =
    def refused(sql: String, fragment: String): Unit =
      val error = assertThrows(classOf[IllegalArgumentException], () => { bind(sql); () }, sql)
      assertTrue(error.getMessage.contains(fragment), s"${error.getMessage} should mention $fragment")
    refused("ALTER TABLE account ADD c BIGINT DEFAULT 'x'", "must be a BIGINT literal")
    refused("ALTER TABLE account ADD c BIGINT NOT NULL", "needs a DEFAULT")
    refused("ALTER TABLE account ADD c BIGINT NOT NULL DEFAULT NULL", "cannot default to NULL")
    refused("ALTER TABLE account ADD balance BIGINT", "already has a column balance")
    refused("ALTER TABLE account RENAME COLUMN missing TO x", "unknown column")
    refused("ALTER TABLE account DROP COLUMN missing", "unknown column")
    refused("ALTER TABLE nowhere DROP COLUMN x", "unknown table")
    refused("ALTER TABLE account ADD c STRANGE", "unsupported type")
    val create = bind("CREATE TABLE t (id BIGINT PRIMARY KEY, n BIGINT NOT NULL DEFAULT 7, s STRING DEFAULT NULL)").asInstanceOf[BoundCreateTable]
    assertEquals(Map("n" -> DbValue.Int64Value(7)), create.defaults)

  /** A NOT NULL column with a default may be omitted by INSERT; one without may not. */
  @Test def insertsMayOmitDefaultedColumns(): Unit =
    val withDefault = new InMemoryCatalog
    val created = withDefault.createEntity("t", Vector(("id", DataType.Int64, false), ("n", DataType.Int64, false)), "id")
    val patched = new io.adb.catalog.Catalog:
      def version = withDefault.version
      private val view = created.copy(fields = created.fields.map(f => if f.name == "n" then f.copy(default = Some(DbValue.Int64Value(0))) else f))
      def entity(name: String) = Option.when(name.equalsIgnoreCase("t"))(view)
      def entity(id: EntityId) = Option.when(id == view.id)(view)
      def entities = Vector(view)
      def createEntity(name: String, fields: Vector[(String, DataType, Boolean)], primaryKey: String, references: Map[String, ForeignKeyRef]) =
        throw new UnsupportedOperationException
    val insert = new Binder(patched).bind(parser.parse("INSERT INTO t (id) VALUES (1)")).asInstanceOf[BoundInsert]
    assertEquals(Map(FieldId(1) -> DbValue.Int64Value(1)), insert.values)
    assertThrows(classOf[IllegalArgumentException], () => { new Binder(withDefault).bind(parser.parse("INSERT INTO t (id) VALUES (1)")); () })
