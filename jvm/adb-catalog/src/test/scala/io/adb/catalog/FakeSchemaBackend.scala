package io.adb.catalog

import io.adb.model.*
import io.adb.schema.*

/** An in-memory stand-in for the engine's catalog functions, speaking the engine's JSON. It
  * applies changes with [[SchemaEvolutionPlanner]] (the engine's rules) and counts calls, so
  * tests can check what the catalog sends and when it reloads.
  */
final class FakeSchemaBackend extends SchemaBackend:
  /** Every version of every entity ever created, by entity id. */
  private var histories = Map.empty[Long, Vector[EntitySchema]]
  /** Ids of dropped entities. */
  private var dropped = Set.empty[Long]
  /** Current catalog epoch. */
  var epoch: Long = 0
  /** Calls of `adb_list_schemas_json`. */
  var listings: Int = 0
  /** Request documents received, in order. */
  val requests: collection.mutable.Buffer[String] = collection.mutable.Buffer.empty

  /** Live latest schemas in id order. */
  private def live: Vector[EntitySchema] =
    histories.toVector.sortBy(_._1).collect { case (id, versions) if !dropped(id) => versions.last }

  def currentSchemaEpoch(): Long = epoch

  def listSchemasJson(): String =
    listings += 1
    SchemaJson.encodeCatalog(SchemaJson.CatalogListing(SchemaEpoch(epoch), live))

  /** Decodes a create request (the subset the catalog sends) and publishes version 1. */
  def createEntityJson(specJson: String): String =
    requests += specJson
    val root = JsonReader.parse(specJson).asInstanceOf[Map[String, Any]]
    val name = root("name").asInstanceOf[String]
    require(!live.exists(_.name.equalsIgnoreCase(name)), s"table $name already exists")
    val id = root.get("entity_id").fold(histories.keys.maxOption.getOrElse(0L) + 1)(_.asInstanceOf[Long])
    require(!histories.contains(id), s"entity id $id is already used")
    epoch += 1
    val fields = root("fields").asInstanceOf[Vector[Map[String, Any]]].zipWithIndex.map { (field, index) =>
      FieldDefinition(
        FieldId(field.get("id").fold(index + 1)(_.asInstanceOf[Long].toInt)),
        field("name").asInstanceOf[String],
        SchemaJson.parseTypeName(field("value_type").asInstanceOf[String]),
        field("nullable").asInstanceOf[Boolean],
        field.get("default").map(SchemaJson.decodeValue),
        SchemaVersion(1),
        None,
        field.get("references").map { raw =>
          val fk = raw.asInstanceOf[Map[String, Any]]
          ForeignKeyRef(EntityId(fk("entity_id").asInstanceOf[Long]), FieldId(fk("field_id").asInstanceOf[Long].toInt))
        }
      )
    }
    val pk = fields.find(_.name.equalsIgnoreCase(root("primary_key").asInstanceOf[String])).get.id
    val schema = EntitySchema(EntityId(id), name, SchemaVersion(1), None, SchemaEpoch(epoch), pk, FieldId(fields.map(_.id.value).max + 1), fields)
    histories += id -> Vector(schema)
    SchemaJson.encodeSchema(schema)

  /** Applies a change sent as JSON; only the parts the tests send are decoded. */
  def alterEntityJson(requestJson: String): String =
    requests += requestJson
    val root = JsonReader.parse(requestJson).asInstanceOf[Map[String, Any]]
    val id = root("entity_id").asInstanceOf[Long]
    val change = root("change").asInstanceOf[Map[String, Any]]
    val parsed = change("op") match
      case "rename_field" => SchemaChange.RenameField(change("from").asInstanceOf[String], change("to").asInstanceOf[String])
      case "drop_field" => SchemaChange.DropField(change("name").asInstanceOf[String])
      case "add_field" =>
        val field = change("field").asInstanceOf[Map[String, Any]]
        SchemaChange.AddField(
          NewField(
            field("name").asInstanceOf[String],
            SchemaJson.parseTypeName(field("value_type").asInstanceOf[String]),
            field("nullable").asInstanceOf[Boolean],
            field.get("default").map(SchemaJson.decodeValue)
          )
        )
    val current = histories(id).last
    val next = SchemaEvolutionPlanner.plan(current, parsed, DependencyGraph.build(live, Vector.empty), SchemaEpoch(epoch)).fold(
      reason => throw new IllegalArgumentException(reason),
      _.next
    )
    epoch += 1
    histories += id -> (histories(id) :+ next)
    SchemaJson.encodeSchema(next)

  def dropEntity(entityId: Long): Unit =
    require(histories.contains(entityId) && !dropped(entityId), s"table $entityId does not exist")
    epoch += 1
    dropped += entityId

  def schemaHistoryJson(entityId: Long): String =
    val versions = histories.getOrElse(entityId, throw new IllegalArgumentException(s"entity $entityId has no schema"))
    s"{\"entity_id\":$entityId,\"versions\":${versions.map(SchemaJson.encodeSchema).mkString("[", ",", "]")}}"

  /** Simulates DDL by another client: renames a field behind the catalog's back. */
  def renameBehindTheBack(entity: Long, from: String, to: String): Unit =
    alterEntityJson(SchemaJson.encodeAlter(EntityId(entity), SchemaChange.RenameField(from, to)))
