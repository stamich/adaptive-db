package io.adb.schema

import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Client-side planning of schema changes: predictions, refusals and dependencies. */
final class SchemaEvolutionPlannerTest:
  /** Index `idx_balance` on `account(balance)` including `currency`. */
  private val index = IndexDependency(IndexId(1), "idx_balance", EntityId(3), Vector(FieldId(2)), Vector(FieldId(4)))
  /** The fixture catalog with that index. */
  private val graph = DependencyGraph.build(Vector(SchemaFixtures.V3), Vector(index))
  /** The fixture catalog without indexes. */
  private val noIndexes = DependencyGraph.build(Vector(SchemaFixtures.V3), Vector.empty)

  /** Plans `change` of version 3 at epoch 6. */
  private def plan(change: SchemaChange, dependencies: DependencyGraph = graph) =
    SchemaEvolutionPlanner.plan(SchemaFixtures.V3, change, dependencies, SchemaEpoch(6))

  /** An added column gets the next id, the new version and the virtual-default class. */
  @Test def predictsAddedColumns(): Unit =
    val evolution = plan(SchemaChange.AddField(NewField("vip", DataType.Bool, true))).toOption.get
    assertEquals(SchemaVersion(4), evolution.next.version)
    assertEquals(Some(SchemaVersion(3)), evolution.next.parent)
    assertEquals(SchemaEpoch(7), evolution.next.epoch)
    assertEquals(FieldId(5), evolution.next.field("vip").get.id)
    assertEquals(FieldId(6), evolution.next.nextFieldId)
    assertEquals(Compatibility.VirtualDefault, evolution.compatibility)

  /** Renames keep the id; drops hide the field; both are classified. */
  @Test def predictsRenamesAndDrops(): Unit =
    val renamed = plan(SchemaChange.RenameField("Balance", "amount")).toOption.get
    assertEquals(FieldId(2), renamed.next.field("amount").get.id)
    assertEquals(Compatibility.MetadataOnly, renamed.compatibility)
    val dropped = plan(SchemaChange.DropField("currency"), noIndexes).toOption.get
    assertEquals(Some(SchemaVersion(4)), dropped.next.fieldById(FieldId(4)).get.droppedIn)
    assertEquals(Vector("id", "balance"), dropped.next.activeFields.map(_.name))
    assertEquals(Compatibility.LogicalDrop, dropped.compatibility)

  /** Every engine refusal is reported with a precise message. */
  @Test def refusesWhatTheEngineRefuses(): Unit =
    def refused(change: SchemaChange, fragment: String): Unit =
      val reason = plan(change).swap.toOption.getOrElse(fail(s"$change was accepted"))
      assertTrue(reason.contains(fragment), s"$reason should mention $fragment")
    refused(SchemaChange.AddField(NewField("BALANCE", DataType.Int64, true)), "already has a column")
    refused(SchemaChange.AddField(NewField("x", DataType.Int64, false)), "needs a DEFAULT")
    refused(SchemaChange.AddField(NewField("x", DataType.Int64, false, Some(DbValue.StringValue("1")))), "the column is BIGINT")
    refused(SchemaChange.AddField(NewField("x", DataType.Int64, true, Some(DbValue.NullValue))), "omit the DEFAULT")
    refused(SchemaChange.AddField(NewField("1x", DataType.Int64, true)), "must start with a letter")
    refused(SchemaChange.AddField(NewField("x" * 257, DataType.Int64, true)), "exceeds 256")
    refused(SchemaChange.RenameField("missing", "y"), "has no column missing")
    refused(SchemaChange.RenameField("balance", "currency"), "already has a column")
    refused(SchemaChange.RenameField("note", "y"), "has no column note")
    refused(SchemaChange.DropField("id"), "primary key")
    refused(SchemaChange.DropField("balance"), "index idx_balance (key column)")
    refused(SchemaChange.DropField("currency"), "index idx_balance (covered column)")

  /** A rename that only changes the case of a name is allowed. */
  @Test def allowsCaseOnlyRenames(): Unit =
    assertTrue(plan(SchemaChange.RenameField("balance", "Balance")).isRight)

  /** CREATE TABLE definitions are checked like the engine does. */
  @Test def checksCreateDefinitions(): Unit =
    val good = EntitySpec("t", Vector(NewField("id", DataType.Int64, false), NewField("v", DataType.StringType, true)), "id")
    assertEquals(None, SchemaEvolutionPlanner.checkCreate(good))
    assertTrue(SchemaEvolutionPlanner.checkCreate(good.copy(primaryKey = "v")).get.contains("non-nullable BIGINT"))
    assertTrue(SchemaEvolutionPlanner.checkCreate(good.copy(primaryKey = "w")).get.contains("not a column"))
    assertTrue(SchemaEvolutionPlanner.checkCreate(good.copy(fields = good.fields :+ NewField("V", DataType.Bool, true))).get.contains("twice"))
    assertTrue(SchemaEvolutionPlanner.checkCreate(good.copy(name = "a-b")).get.contains("letters, digits"))
