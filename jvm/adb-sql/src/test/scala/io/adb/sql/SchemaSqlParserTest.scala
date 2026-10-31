package io.adb.sql

import io.adb.sql.SqlExpr.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Schema statements of the SQL grammar (2.4.0): ALTER TABLE, DROP TABLE, SHOW SCHEMA, DEFAULT. */
final class SchemaSqlParserTest:
  /** Parser under test. */
  private val parser = new SqlParser

  /** The three ALTER TABLE actions, with and without the optional COLUMN keyword. */
  @Test def parsesAlterTable(): Unit =
    assertEquals(
      AlterTable("account", AlterAction.AddColumn(ColumnDef("currency", "STRING", false, false, None, Some(StringLiteral("EUR"))))),
      parser.parse("ALTER TABLE account ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR'")
    )
    assertEquals(
      AlterTable("account", AlterAction.AddColumn(ColumnDef("vip", "BOOLEAN", true, false))),
      parser.parse("alter table account add vip BOOLEAN NULL;")
    )
    assertEquals(
      AlterTable("t", AlterAction.AddColumn(ColumnDef("owner", "BIGINT", true, false, Some(ColumnReference("p", "id")), Some(LongLiteral(-1))))),
      parser.parse("ALTER TABLE t ADD owner BIGINT DEFAULT -1 REFERENCES p(id) NOT ENFORCED")
    )
    assertEquals(AlterTable("t", AlterAction.RenameColumn("a", "b")), parser.parse("ALTER TABLE t RENAME COLUMN a TO b"))
    assertEquals(AlterTable("t", AlterAction.RenameColumn("a", "b")), parser.parse("ALTER TABLE t RENAME a TO b"))
    assertEquals(AlterTable("t", AlterAction.DropColumn("a")), parser.parse("ALTER TABLE t DROP COLUMN a RESTRICT"))
    assertEquals(AlterTable("t", AlterAction.DropColumn("a")), parser.parse("ALTER TABLE t DROP a"))

  /** DROP TABLE, SHOW SCHEMA and defaults in CREATE TABLE. */
  @Test def parsesDropShowAndDefaults(): Unit =
    assertEquals(DropTable("t"), parser.parse("DROP TABLE t"))
    assertEquals(DropTable("t"), parser.parse("drop table t restrict;"))
    assertEquals(ShowSchema("account"), parser.parse("SHOW SCHEMA account"))
    assertEquals(
      CreateTable("t", Vector(ColumnDef("id", "BIGINT", false, true), ColumnDef("ratio", "DOUBLE", false, false, None, Some(DoubleLiteral(0.5))))),
      parser.parse("CREATE TABLE t (id BIGINT PRIMARY KEY, ratio DOUBLE DEFAULT 0.5 NOT NULL)")
    )

  /** Unsupported shapes are syntax errors with a pointed message. */
  @Test def rejectsUnsupportedShapes(): Unit =
    def refused(sql: String, fragment: String): Unit =
      val error = assertThrows(classOf[IllegalArgumentException], () => { parser.parse(sql); () }, sql)
      assertTrue(error.getMessage.contains(fragment), s"${error.getMessage} should mention $fragment")
    refused("ALTER TABLE t DROP COLUMN a CASCADE", "CASCADE is not supported")
    refused("DROP TABLE t CASCADE", "CASCADE is not supported")
    refused("ALTER TABLE t RENAME TO u", "renaming tables is not supported")
    refused("ALTER TABLE t ADD COLUMN k BIGINT PRIMARY KEY", "cannot add a PRIMARY KEY")
    refused("ALTER TABLE t ALTER COLUMN a TYPE BIGINT", "expected ADD, RENAME or DROP")
    refused("ALTER TABLE t ADD c BIGINT DEFAULT x", "must be a literal")
    refused("ALTER TABLE t ADD c BIGINT DEFAULT 1 DEFAULT 2", "more than one DEFAULT")
    refused("SHOW SCHEMA", "expected identifier")
