package io.adb.schema

import io.adb.model.*

/** The engine's catalog documents decoded into the schema model, and DDL requests encoded for
  * the engine (see `include/adb.h`, ABI 7, and docs/schema.md).
  *
  * Values use the engine's JSON form (`"Null"`, `{"Int64":5}`, `{"String":"x"}`, ...), types
  * its type names (`"Int64"`, `"String"`, ...). Decoding is strict: a missing or mistyped member
  * is an `IllegalArgumentException`, so a catalog from another engine version is never
  * half-understood.
  */
object SchemaJson:
  /** A decoded `adb_list_schemas_json` document: the epoch and the live schemas in id order. */
  final case class CatalogListing(epoch: SchemaEpoch, entities: Vector[EntitySchema]) derives CanEqual

  /** Decodes an `adb_list_schemas_json` document. */
  def decodeCatalog(json: String): CatalogListing =
    val root = obj(JsonReader.parse(json), "catalog")
    CatalogListing(
      SchemaEpoch(long(root, "schema_epoch")),
      array(root, "entities").map(decodeSchemaValue)
    )

  /** Decodes an `adb_schema_history_json` document into a history. */
  def decodeHistory(json: String): SchemaHistory =
    val root = obj(JsonReader.parse(json), "history")
    SchemaHistory(EntityId(long(root, "entity_id")), array(root, "versions").map(decodeSchemaValue))

  /** Decodes one schema document (the result of create and alter). */
  def decodeSchema(json: String): EntitySchema = decodeSchemaValue(JsonReader.parse(json))

  /** Encodes a `CREATE TABLE` definition for `adb_create_entity_json`. */
  def encodeSpec(spec: EntitySpec): String =
    val members = Vector(
      s"\"name\":${string(spec.name)}",
      s"\"primary_key\":${string(spec.primaryKey)}",
      s"\"fields\":[${spec.fields.map(encodeNewField).mkString(",")}]"
    ) ++ spec.entityId.map(id => s"\"entity_id\":${id.value}")
    members.mkString("{", ",", "}")

  /** Encodes an `ALTER TABLE` request for `adb_alter_entity_json`. */
  def encodeAlter(entity: EntityId, change: SchemaChange): String =
    s"{\"entity_id\":${entity.value},\"change\":${encodeChange(change)}}"

  /** Encodes one change (`{"op":"add_field",...}`). */
  def encodeChange(change: SchemaChange): String = change match
    case SchemaChange.AddField(field) => s"{\"op\":\"add_field\",\"field\":${encodeNewField(field)}}"
    case SchemaChange.RenameField(from, to) =>
      s"{\"op\":\"rename_field\",\"from\":${string(from)},\"to\":${string(to)}}"
    case SchemaChange.DropField(name) => s"{\"op\":\"drop_field\",\"name\":${string(name)}}"

  /** Encodes a schema version exactly as the engine writes it (tools, tests and fakes of the
    * engine; the gateway only decodes).
    */
  def encodeSchema(schema: EntitySchema): String =
    def optional(value: Option[Long]): String = value.fold("null")(_.toString)
    val fields = schema.fields.map { field =>
      Vector(
        s"\"default\":${field.default.fold("null")(encodeValue)}",
        s"\"dropped_in\":${optional(field.droppedIn.map(_.value))}",
        s"\"id\":${field.id.value}",
        s"\"introduced_in\":${field.introducedIn.value}",
        s"\"name\":${string(field.name)}",
        s"\"nullable\":${field.nullable}",
        s"\"references\":${field.references.fold("null")(fk => s"{\"entity_id\":${fk.entity.value},\"field_id\":${fk.field.value}}")}",
        s"\"value_type\":\"${typeName(field.dataType)}\""
      ).mkString("{", ",", "}")
    }
    Vector(
      s"\"entity_id\":${schema.entityId.value}",
      s"\"epoch\":${schema.epoch.value}",
      s"\"fields\":${fields.mkString("[", ",", "]")}",
      s"\"name\":${string(schema.name)}",
      s"\"next_field_id\":${schema.nextFieldId.value}",
      s"\"parent\":${optional(schema.parent.map(_.value))}",
      s"\"primary_key\":${schema.primaryKey.value}",
      s"\"version\":${schema.version.value}"
    ).mkString("{", ",", "}")

  /** Encodes a catalog listing exactly as `adb_list_schemas_json` writes it. */
  def encodeCatalog(listing: CatalogListing): String =
    s"{\"entities\":${listing.entities.map(encodeSchema).mkString("[", ",", "]")},\"schema_epoch\":${listing.epoch.value}}"

  /** Encodes a value in the engine's form. */
  def encodeValue(value: DbValue): String = value match
    case DbValue.NullValue => "\"Null\""
    case DbValue.BoolValue(v) => s"{\"Bool\":$v}"
    case DbValue.Int64Value(v) => s"{\"Int64\":$v}"
    case DbValue.Float64Value(v) =>
      require(!v.isNaN && !v.isInfinite, "non-finite DOUBLE values have no JSON form")
      s"{\"Float64\":$v}"
    case DbValue.StringValue(v) => s"{\"String\":${string(v)}}"
    case DbValue.BytesValue(v) => s"{\"Bytes\":[${v.map(b => b & 0xff).mkString(",")}]}"

  /** A value as a SQL literal (for messages and `SHOW SCHEMA`). */
  def sqlLiteral(value: DbValue): String = value match
    case DbValue.NullValue => "NULL"
    case DbValue.BoolValue(v) => if v then "TRUE" else "FALSE"
    case DbValue.Int64Value(v) => v.toString
    case DbValue.Float64Value(v) => v.toString
    case DbValue.StringValue(v) => "'" + v.replace("'", "''") + "'"
    case DbValue.BytesValue(v) => v.map(b => f"${b & 0xff}%02x").mkString("X'", "", "'")

  /** The engine name of a type. */
  def typeName(dataType: DataType): String = dataType match
    case DataType.Bool => "Bool"
    case DataType.Int64 => "Int64"
    case DataType.Float64 => "Float64"
    case DataType.StringType => "String"
    case DataType.Bytes => "Bytes"

  /** The type of an engine type name. */
  def parseTypeName(name: String): DataType = name match
    case "Bool" => DataType.Bool
    case "Int64" => DataType.Int64
    case "Float64" => DataType.Float64
    case "String" => DataType.StringType
    case "Bytes" => DataType.Bytes
    case other => throw new IllegalArgumentException(s"unknown value type $other")

  /** Decodes an engine value. */
  def decodeValue(raw: Any): DbValue = raw match
    case "Null" => DbValue.NullValue
    case tagged: Map[?, ?] if tagged.size == 1 =>
      tagged.head.asInstanceOf[(Any, Any)] match
        case ("Bool", v: Boolean) => DbValue.BoolValue(v)
        case ("Int64", v: Long) => DbValue.Int64Value(v)
        case ("Float64", v: Double) => DbValue.Float64Value(v)
        case ("Float64", v: Long) => DbValue.Float64Value(v.toDouble)
        case ("String", v: String) => DbValue.StringValue(v)
        case ("Bytes", v: Vector[?]) =>
          DbValue.BytesValue(v.map {
            case b: Long if b >= 0 && b <= 255 => b.toByte
            case other => throw new IllegalArgumentException(s"invalid byte $other")
          }.toArray)
        case _ => throw new IllegalArgumentException(s"invalid value $raw")
    case _ => throw new IllegalArgumentException(s"invalid value $raw")

  /** Encodes a field of a create or add request. */
  private def encodeNewField(field: NewField): String =
    val members = Vector(
      s"\"name\":${string(field.name)}",
      s"\"value_type\":\"${typeName(field.dataType)}\"",
      s"\"nullable\":${field.nullable}"
    ) ++ field.default.map(value => s"\"default\":${encodeValue(value)}") ++
      field.references.map(fk => s"\"references\":{\"entity_id\":${fk.entity.value},\"field_id\":${fk.field.value}}") ++
      field.id.map(id => s"\"id\":${id.value}")
    members.mkString("{", ",", "}")

  /** Decodes one parsed schema document. */
  private def decodeSchemaValue(raw: Any): EntitySchema =
    val map = obj(raw, "schema")
    val version = long(map, "version")
    require(version >= 1, s"schema version must be at least 1, got $version")
    EntitySchema(
      EntityId(long(map, "entity_id")),
      str(map, "name"),
      SchemaVersion(version),
      optionalLong(map, "parent").map(SchemaVersion(_)),
      SchemaEpoch(long(map, "epoch")),
      FieldId(int(map, "primary_key")),
      FieldId(int(map, "next_field_id")),
      array(map, "fields").map(decodeField)
    )

  /** Decodes one field definition. */
  private def decodeField(raw: Any): FieldDefinition =
    val map = obj(raw, "field")
    FieldDefinition(
      FieldId(int(map, "id")),
      str(map, "name"),
      parseTypeName(str(map, "value_type")),
      bool(map, "nullable"),
      map.get("default").flatMap(Option(_)).map(decodeValue).filterNot(_ == DbValue.NullValue),
      SchemaVersion(long(map, "introduced_in")),
      optionalLong(map, "dropped_in").map(SchemaVersion(_)),
      map.get("references").flatMap(Option(_)).map { fk =>
        val refs = obj(fk, "references")
        ForeignKeyRef(EntityId(long(refs, "entity_id")), FieldId(int(refs, "field_id")))
      }
    )

  /** A JSON string literal with the mandatory escapes. */
  private[schema] def string(text: String): String =
    val out = new StringBuilder("\"")
    text.foreach {
      case '"' => out ++= "\\\""
      case '\\' => out ++= "\\\\"
      case '\n' => out ++= "\\n"
      case '\r' => out ++= "\\r"
      case '\t' => out ++= "\\t"
      case c if c < ' ' => out ++= f"\\u${c.toInt}%04x"
      case c => out += c
    }
    out += '"'
    out.result()

  /** A JSON object or a decode error naming `what`. */
  private def obj(raw: Any, what: String): Map[String, Any] = raw match
    case map: Map[?, ?] => map.asInstanceOf[Map[String, Any]]
    case other => throw new IllegalArgumentException(s"$what must be an object, got $other")

  /** A required array member. */
  private def array(map: Map[String, Any], key: String): Vector[Any] = map.get(key) match
    case Some(values: Vector[?]) => values
    case other => throw new IllegalArgumentException(s"$key must be an array, got $other")

  /** A required non-negative integer member. */
  private def long(map: Map[String, Any], key: String): Long = map.get(key) match
    case Some(v: Long) if v >= 0 => v
    case other => throw new IllegalArgumentException(s"$key must be a non-negative integer, got $other")

  /** A required non-negative integer member that fits an `Int`. */
  private def int(map: Map[String, Any], key: String): Int =
    val value = long(map, key)
    require(value <= Int.MaxValue, s"$key out of range: $value")
    value.toInt

  /** An optional non-negative integer member (absent or null = none). */
  private def optionalLong(map: Map[String, Any], key: String): Option[Long] =
    map.get(key).flatMap(Option(_)).map(_ => long(map, key))

  /** A required string member. */
  private def str(map: Map[String, Any], key: String): String = map.get(key) match
    case Some(v: String) => v
    case other => throw new IllegalArgumentException(s"$key must be a string, got $other")

  /** A required boolean member. */
  private def bool(map: Map[String, Any], key: String): Boolean = map.get(key) match
    case Some(v: Boolean) => v
    case other => throw new IllegalArgumentException(s"$key must be a boolean, got $other")
