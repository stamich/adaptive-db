package io.adb.schema

import io.adb.model.*

/** Schemas shared by the schema tests: an `account` table and its history. */
object SchemaFixtures:
  /** A catalog listing exactly as the engine writes it (`adb_list_schemas_json`): version 3 of
    * `account` after `ADD COLUMN currency` and `DROP COLUMN note`.
    */
  val CatalogJson: String =
    """{"entities":[{"entity_id":3,"epoch":5,"fields":[
      |{"default":null,"dropped_in":null,"id":1,"introduced_in":1,"name":"id","nullable":false,"references":null,"value_type":"Int64"},
      |{"default":{"Int64":0},"dropped_in":null,"id":2,"introduced_in":1,"name":"balance","nullable":false,"references":null,"value_type":"Int64"},
      |{"default":null,"dropped_in":3,"id":3,"introduced_in":1,"name":"note","nullable":true,"references":null,"value_type":"String"},
      |{"default":{"String":"EUR"},"dropped_in":null,"id":4,"introduced_in":2,"name":"currency","nullable":false,"references":null,"value_type":"String"}],
      |"name":"account","next_field_id":5,"parent":2,"primary_key":1,"version":3}],"schema_epoch":6}""".stripMargin

  /** Field `id` (primary key). */
  val Id: FieldDefinition = FieldDefinition(FieldId(1), "id", DataType.Int64, false, None, SchemaVersion(1), None, None)
  /** Field `balance` with default 0. */
  val Balance: FieldDefinition =
    FieldDefinition(FieldId(2), "balance", DataType.Int64, false, Some(DbValue.Int64Value(0)), SchemaVersion(1), None, None)
  /** Field `note`, dropped in version 3. */
  val Note: FieldDefinition = FieldDefinition(FieldId(3), "note", DataType.StringType, true, None, SchemaVersion(1), None, None)
  /** Field `currency`, added in version 2. */
  val Currency: FieldDefinition =
    FieldDefinition(FieldId(4), "currency", DataType.StringType, false, Some(DbValue.StringValue("EUR")), SchemaVersion(2), None, None)

  /** Version 1 of `account`: id, balance, note. */
  val V1: EntitySchema =
    EntitySchema(EntityId(3), "account", SchemaVersion(1), None, SchemaEpoch(1), FieldId(1), FieldId(4), Vector(Id, Balance, Note))
  /** Version 2: adds currency. */
  val V2: EntitySchema =
    V1.copy(version = SchemaVersion(2), parent = Some(SchemaVersion(1)), epoch = SchemaEpoch(3), nextFieldId = FieldId(5), fields = V1.fields :+ Currency)
  /** Version 3: drops note. */
  val V3: EntitySchema =
    V2.copy(
      version = SchemaVersion(3),
      parent = Some(SchemaVersion(2)),
      epoch = SchemaEpoch(5),
      fields = Vector(Id, Balance, Note.copy(droppedIn = Some(SchemaVersion(3))), Currency)
    )
