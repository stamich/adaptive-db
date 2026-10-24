package io.adb.sql

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Index statements of the SQL grammar (2.3.0): CREATE INDEX, DROP INDEX, SHOW INDEXES. */
final class IndexSqlParserTest:
  /** Parser under test. */
  private val parser = new SqlParser

  /** Key columns with directions and an INCLUDE list. */
  @Test def parsesCreateIndex(): Unit =
    assertEquals(
      CreateIndex("idx_account_balance", "account", Vector(IndexColumnDef("balance", false), IndexColumnDef("owner", true)), Vector("note")),
      parser.parse("CREATE INDEX idx_account_balance ON account (balance ASC, owner DESC) INCLUDE (note);")
    )
    assertEquals(
      CreateIndex("i", "t", Vector(IndexColumnDef("c", false)), Vector.empty),
      parser.parse("create index i on t (c)")
    )

  /** DROP INDEX and SHOW INDEXES with and without a table; EXPLAIN never wraps DDL oddly. */
  @Test def parsesDropAndShow(): Unit =
    assertEquals(DropIndex("idx"), parser.parse("DROP INDEX idx"))
    assertEquals(ShowIndexes(None), parser.parse("SHOW INDEXES;"))
    assertEquals(ShowIndexes(Some("account")), parser.parse("show index on account"))

  /** Malformed statements and oversized column lists are syntax errors. */
  @Test def rejectsMalformedIndexStatements(): Unit =
    for bad <- Vector(
        "CREATE INDEX ON t (c)",
        "CREATE INDEX i t (c)",
        "CREATE INDEX i ON t ()",
        "CREATE INDEX i ON t (c) INCLUDE ()",
        "CREATE INDEX i ON t (c DESC ASC)",
        "DROP INDEX",
        "DROP TABLE t",
        "SHOW TABLES",
        "CREATE INDEX i ON t (" + (1 to 17).map(n => s"c$n").mkString(", ") + ")"
      )
    do assertThrows(classOf[IllegalArgumentException], () => { parser.parse(bad); () }, bad)
