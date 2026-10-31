package io.adb.schema

import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Schema histories: lookups by version and epoch, and reconstruction of changes. */
final class SchemaHistoryTest:
  /** The history of the fixtures. */
  private val history = SchemaHistory(EntityId(3), Vector(SchemaFixtures.V1, SchemaFixtures.V2, SchemaFixtures.V3))

  /** Versions are found by number and by the epoch they were current at. */
  @Test def looksUpVersions(): Unit =
    assertEquals(SchemaFixtures.V3, history.latest)
    assertEquals(Some(SchemaFixtures.V2), history.at(SchemaVersion(2)))
    assertEquals(None, history.at(SchemaVersion(0)))
    assertEquals(None, history.at(SchemaVersion(4)))
    assertEquals(None, history.atEpoch(SchemaEpoch(0)), "before the table existed")
    assertEquals(Some(SchemaFixtures.V1), history.atEpoch(SchemaEpoch(2)))
    assertEquals(Some(SchemaFixtures.V2), history.atEpoch(SchemaEpoch(3)))
    assertEquals(Some(SchemaFixtures.V3), history.atEpoch(SchemaEpoch(100)))

  /** Each version's change is reconstructed from its parent. */
  @Test def reconstructsChanges(): Unit =
    assertEquals(
      Vector(
        SchemaVersion(2) -> SchemaChange.AddField(NewField("currency", DataType.StringType, false, Some(DbValue.StringValue("EUR")))),
        SchemaVersion(3) -> SchemaChange.DropField("note")
      ),
      history.changes
    )
    val renamed = SchemaFixtures.V3.copy(
      version = SchemaVersion(4),
      parent = Some(SchemaVersion(3)),
      fields = SchemaFixtures.V3.fields.map(f => if f.id == FieldId(2) then f.copy(name = "amount") else f)
    )
    assertEquals(SchemaChange.RenameField("balance", "amount"), SchemaHistory.diff(SchemaFixtures.V3, renamed))
    assertEquals("RENAME COLUMN balance TO amount", SchemaHistory.diff(SchemaFixtures.V3, renamed).sql)
    assertThrows(classOf[IllegalArgumentException], () => SchemaHistory.diff(SchemaFixtures.V1, SchemaFixtures.V3))

  /** A history must be the chain 1, 2, 3, ... of one entity. */
  @Test def refusesBrokenChains(): Unit =
    assertThrows(classOf[IllegalArgumentException], () => SchemaHistory(EntityId(3), Vector.empty))
    assertThrows(classOf[IllegalArgumentException], () => SchemaHistory(EntityId(3), Vector(SchemaFixtures.V2)))
    assertThrows(classOf[IllegalArgumentException], () => SchemaHistory(EntityId(4), Vector(SchemaFixtures.V1)))
