package io.adb.schema

import io.adb.model.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** The engine's catalog documents decode into the model; DDL requests encode as the engine expects. */
final class SchemaJsonTest:
  /** The engine's listing decodes field by field, dropped fields included. */
  @Test def decodesTheEngineCatalog(): Unit =
    val listing = SchemaJson.decodeCatalog(SchemaFixtures.CatalogJson)
    assertEquals(SchemaEpoch(6), listing.epoch)
    assertEquals(Vector(SchemaFixtures.V3), listing.entities)
    val account = listing.entities.head
    assertEquals(Vector("id", "balance", "currency"), account.activeFields.map(_.name))
    assertTrue(account.field("NOTE").isEmpty, "dropped fields are not found by name")
    assertEquals(Some("note"), account.fieldById(FieldId(3)).map(_.name))

  /** The binder's entity view keeps active fields with their defaults and lifetimes. */
  @Test def convertsToTheBinderView(): Unit =
    val entity = SchemaFixtures.V3.toEntity
    assertEquals(Vector("id", "balance", "currency"), entity.fields.map(_.name))
    assertEquals(Some(DbValue.StringValue("EUR")), entity.field("currency").flatMap(_.default))
    assertEquals(SchemaVersion(2), entity.field("currency").get.introducedIn)
    assertEquals(SchemaVersion(3), entity.schemaVersion)

  /** Requests use the engine's member names, types and value forms. */
  @Test def encodesRequests(): Unit =
    val spec = EntitySpec(
      "t\"x",
      Vector(NewField("id", DataType.Int64, false), NewField("b", DataType.Bytes, true, Some(DbValue.BytesValue(Array[Byte](1, -1))))),
      "id",
      Some(EntityId(9))
    )
    assertEquals(
      """{"name":"t\"x","primary_key":"id","fields":[{"name":"id","value_type":"Int64","nullable":false},""" +
        """{"name":"b","value_type":"Bytes","nullable":true,"default":{"Bytes":[1,255]}}],"entity_id":9}""",
      SchemaJson.encodeSpec(spec)
    )
    assertEquals(
      """{"entity_id":3,"change":{"op":"rename_field","from":"a","to":"b"}}""",
      SchemaJson.encodeAlter(EntityId(3), SchemaChange.RenameField("a", "b"))
    )
    assertEquals(
      """{"op":"add_field","field":{"name":"c","value_type":"String","nullable":false,"default":{"String":"EUR"}}}""",
      SchemaJson.encodeChange(SchemaChange.AddField(NewField("c", DataType.StringType, false, Some(DbValue.StringValue("EUR")))))
    )
    assertEquals("""{"op":"drop_field","name":"c"}""", SchemaJson.encodeChange(SchemaChange.DropField("c")))

  /** Values round-trip through the engine form. */
  @Test def valuesRoundTrip(): Unit =
    for value <- Vector(DbValue.NullValue, DbValue.BoolValue(true), DbValue.Int64Value(-5), DbValue.Float64Value(1.5), DbValue.StringValue("ü\n")) do
      assertEquals(value, SchemaJson.decodeValue(JsonReader.parse(SchemaJson.encodeValue(value))))
    assertThrows(classOf[IllegalArgumentException], () => SchemaJson.encodeValue(DbValue.Float64Value(Double.NaN)))

  /** Malformed documents are refused rather than half-read. */
  @Test def refusesMalformedDocuments(): Unit =
    for bad <- Vector(
        """{"entities":[],"schema_epoch":-1}""",
        """{"entities":{},"schema_epoch":1}""",
        SchemaFixtures.CatalogJson.replace("\"Int64\"}", "\"Int128\"}"),
        SchemaFixtures.CatalogJson.replace("\"version\":3", "\"version\":0")
      )
    do assertThrows(classOf[IllegalArgumentException], () => { SchemaJson.decodeCatalog(bad); () }, bad)
