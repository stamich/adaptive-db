package io.adb.schema

import io.adb.model.*

/** Identity of an object in the catalog that others can depend on. */
enum CatalogObjectId derives CanEqual:
  /** An entity (table). */
  case Entity(id: EntityId)
  /** One field of an entity. */
  case Field(entity: EntityId, field: FieldId)
  /** A secondary index. */
  case Index(id: IndexId)

/** Why one catalog object depends on another. */
enum DependencyKind derives CanEqual:
  /** The entity is keyed by the field. */
  case PrimaryKey
  /** The index uses the field as a key column. */
  case IndexKey
  /** The index covers the field (`INCLUDE`). */
  case IndexInclude
  /** The field holds primary keys of the referenced entity (a foreign-key hint). */
  case ForeignKey
  /** The index belongs to the entity. */
  case IndexOf

/** `dependent` cannot exist without `dependsOn`.
  *
  * @param dependent the object that would break
  * @param dependsOn the object it relies on
  * @param kind      the kind of reliance
  */
final case class Dependency(dependent: CatalogObjectId, dependsOn: CatalogObjectId, kind: DependencyKind) derives CanEqual

/** The fields an index relies on (the part of an index definition the graph needs).
  *
  * @param id      index id
  * @param name    index name, used in messages
  * @param entity  indexed entity
  * @param keys    key columns
  * @param include covered columns
  */
final case class IndexDependency(id: IndexId, name: String, entity: EntityId, keys: Vector[FieldId], include: Vector[FieldId]) derives CanEqual

/** Dependencies between the live catalog objects: primary keys, index columns and foreign-key
  * hints. `DROP COLUMN ... RESTRICT` and `DROP TABLE` consult it to refuse a drop that would
  * break a dependent object and to name that object in the error (the engine enforces the same
  * rules; the graph explains them before a round trip).
  *
  * @param schemas      live schemas by entity id
  * @param indexes      index definitions by id
  * @param dependencies every dependency edge
  */
final class DependencyGraph private (
    schemas: Map[EntityId, EntitySchema],
    indexes: Map[IndexId, IndexDependency],
    val dependencies: Vector[Dependency]
):
  /** Objects that depend on `target` directly (for an entity: also those depending on any of
    * its fields from other entities, e.g. foreign-key hints referencing its primary key).
    */
  def dependents(target: CatalogObjectId): Vector[Dependency] = target match
    case CatalogObjectId.Entity(id) =>
      dependencies.filter(dependency =>
        dependency.dependsOn == target || (dependency.dependsOn match
          case CatalogObjectId.Field(entity, _) => entity == id && !belongsTo(dependency.dependent, id)
          case _ => false)
      )
    case other => dependencies.filter(_.dependsOn == other)

  /** Dependents that make dropping `target` impossible (RESTRICT): an empty result means the
    * drop is allowed.
    */
  def blockingDrop(target: CatalogObjectId): Vector[Dependency] = dependents(target)

  /** A human-readable description of a catalog object (`index idx_balance`, `column account.balance`). */
  def describe(target: CatalogObjectId): String = target match
    case CatalogObjectId.Entity(id) => s"table ${entityName(id)}"
    case CatalogObjectId.Field(entity, field) =>
      val name = schemas.get(entity).flatMap(_.fieldById(field)).fold(s"#${field.value}")(_.name)
      s"column ${entityName(entity)}.$name"
    case CatalogObjectId.Index(id) => s"index ${indexes.get(id).fold(s"#${id.value}")(_.name)}"

  /** Why `target` cannot be dropped, or `None` if nothing depends on it. */
  def dropRefusal(target: CatalogObjectId): Option[String] =
    val blocking = blockingDrop(target)
    Option.when(blocking.nonEmpty) {
      val reasons = blocking.map(dependency => s"${describe(dependency.dependent)} (${DependencyGraph.reason(dependency.kind)})").distinct
      s"cannot drop ${describe(target)}: ${reasons.mkString(", ")} depends on it"
    }

  /** Whether `candidate` is the entity `entity` or one of its fields. */
  private def belongsTo(candidate: CatalogObjectId, entity: EntityId): Boolean = candidate match
    case CatalogObjectId.Entity(id) => id == entity
    case CatalogObjectId.Field(id, _) => id == entity
    case CatalogObjectId.Index(id) => indexes.get(id).exists(_.entity == entity)

  /** The name of an entity, or its id when unknown. */
  private def entityName(id: EntityId): String = schemas.get(id).fold(s"#${id.value}")(_.name)

/** Construction of dependency graphs. */
object DependencyGraph:
  /** The graph of live `schemas` and `indexes`. */
  def build(schemas: Iterable[EntitySchema], indexes: Iterable[IndexDependency]): DependencyGraph =
    val edges = Vector.newBuilder[Dependency]
    for schema <- schemas do
      val entity = CatalogObjectId.Entity(schema.entityId)
      edges += Dependency(entity, CatalogObjectId.Field(schema.entityId, schema.primaryKey), DependencyKind.PrimaryKey)
      for field <- schema.activeFields; fk <- field.references do
        edges += Dependency(CatalogObjectId.Field(schema.entityId, field.id), CatalogObjectId.Field(fk.entity, fk.field), DependencyKind.ForeignKey)
    for index <- indexes do
      val target = CatalogObjectId.Index(index.id)
      edges += Dependency(target, CatalogObjectId.Entity(index.entity), DependencyKind.IndexOf)
      for key <- index.keys do edges += Dependency(target, CatalogObjectId.Field(index.entity, key), DependencyKind.IndexKey)
      for covered <- index.include do edges += Dependency(target, CatalogObjectId.Field(index.entity, covered), DependencyKind.IndexInclude)
    new DependencyGraph(
      schemas.map(schema => schema.entityId -> schema).toMap,
      indexes.map(index => index.id -> index).toMap,
      edges.result()
    )

  /** The phrase used for a dependency kind in messages. */
  private def reason(kind: DependencyKind): String = kind match
    case DependencyKind.PrimaryKey => "primary key"
    case DependencyKind.IndexKey => "key column"
    case DependencyKind.IndexInclude => "covered column"
    case DependencyKind.ForeignKey => "foreign-key hint"
    case DependencyKind.IndexOf => "index of the table"
