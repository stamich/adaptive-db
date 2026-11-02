package io.adb.logical

import io.adb.catalog.InMemoryCatalog
import io.adb.consistency.{ReadConsistency, StalenessBound}
import io.adb.sql.SqlParser
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Binding of `SET SESSION CONSISTENCY`. */
final class ConsistencyBinderTest:
  /** Binds `sql` against an empty catalog (the statement names no table). */
  private def bind(sql: String): BoundStatement = new Binder(new InMemoryCatalog).bind(new SqlParser().parse(sql))

  /** Every spelling maps to its level. */
  @Test def bindsEveryLevel(): Unit =
    assertEquals(BoundSetConsistency(ReadConsistency.StrongLatest), bind("SET SESSION CONSISTENCY STRONG"))
    assertEquals(BoundSetConsistency(ReadConsistency.ReadYourWrites), bind("SET SESSION CONSISTENCY READ_YOUR_WRITES"))
    assertEquals(BoundSetConsistency(ReadConsistency.Eventual), bind("SET SESSION CONSISTENCY EVENTUAL"))
    assertEquals(BoundSetConsistency(ReadConsistency.BoundedStaleness(StalenessBound.Millis(500))), bind("SET SESSION CONSISTENCY BOUNDED 500 MS"))
    assertEquals(BoundSetConsistency(ReadConsistency.BoundedStaleness(StalenessBound.Commits(7))), bind("SET SESSION CONSISTENCY BOUNDED 7 COMMITS"))

  /** The level's SQL spelling parses back to the same level. */
  @Test def spellingRoundTrips(): Unit =
    for level <- Seq(ReadConsistency.StrongLatest, ReadConsistency.ReadYourWrites, ReadConsistency.Eventual,
        ReadConsistency.BoundedStaleness(StalenessBound.Millis(12)), ReadConsistency.BoundedStaleness(StalenessBound.Commits(0))) do
      assertEquals(BoundSetConsistency(level), bind(s"SET SESSION CONSISTENCY ${level.sql}"))
