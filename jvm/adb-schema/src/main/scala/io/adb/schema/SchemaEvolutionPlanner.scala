package io.adb.schema

import io.adb.model.*

/** The outcome of planning one schema change.
  *
  * @param change        the change, as it will be sent to the engine
  * @param next          the schema version the engine will publish (its epoch is the catalog
  *                      epoch the change is expected to commit at)
  * @param compatibility how existing rows are affected
  */
final case class SchemaEvolution(change: SchemaChange, next: EntitySchema, compatibility: Compatibility) derives CanEqual

/** Checks a schema change against the current catalog before it is sent to the engine, and
  * predicts the version it produces.
  *
  * The rules mirror the engine's (`adb-schema::apply` and the RESTRICT checks of
  * `Database::alter_entity`), so a statement is refused with a precise SQL-level message without
  * a native round trip; the engine still re-checks everything and stays the authority (another
  * client may have changed the catalog meanwhile).
  */
object SchemaEvolutionPlanner:
  /** Longest entity or field name, in characters (the engine's limit). */
  val MaxNameChars: Int = 256
  /** Most fields of one entity, dropped ones included (the engine's limit). */
  val MaxFieldsPerEntity: Int = 4096
  /** Most versions of one entity (the engine's limit). */
  val MaxVersionsPerEntity: Long = 65535

  /** Plans `change` of `schema`.
    *
    * @param schema       latest schema of the entity
    * @param change       requested change
    * @param dependencies dependencies of the live catalog (indexes, foreign-key hints)
    * @param catalogEpoch current catalog epoch; the change commits at the next one
    * @return the evolution, or the reason the engine would refuse the change
    */
  def plan(
      schema: EntitySchema,
      change: SchemaChange,
      dependencies: DependencyGraph,
      catalogEpoch: SchemaEpoch
  ): Either[String, SchemaEvolution] =
    val version = SchemaVersion(schema.version.value + 1)
    val base = schema.copy(version = version, parent = Some(schema.version), epoch = SchemaEpoch(catalogEpoch.value + 1))
    val next: Either[String, EntitySchema] =
      if schema.version.value >= MaxVersionsPerEntity then Left(s"${schema.name} has reached $MaxVersionsPerEntity schema versions")
      else
        change match
          case SchemaChange.AddField(field) =>
            for
              _ <- validateName("column", field.name)
              _ <- Either.cond(field.id.isEmpty, (), "ADD COLUMN assigns the field id itself")
              _ <- Either.cond(schema.field(field.name).isEmpty, (), s"${schema.name} already has a column ${field.name}")
              _ <- Either.cond(
                field.nullable || field.default.isDefined,
                (),
                s"ADD COLUMN ${field.name} NOT NULL needs a DEFAULT: existing rows have no value"
              )
              _ <- validateDefault(field.name, field.dataType, field.default)
              _ <- Either.cond(
                schema.fields.size < MaxFieldsPerEntity,
                (),
                s"an entity has at most $MaxFieldsPerEntity fields, dropped ones included"
              )
            yield base.copy(
              fields = schema.fields :+ FieldDefinition(
                schema.nextFieldId,
                field.name,
                field.dataType,
                field.nullable,
                field.default,
                version,
                None,
                field.references
              ),
              nextFieldId = FieldId(schema.nextFieldId.value + 1)
            )
          case SchemaChange.RenameField(from, to) =>
            for
              current <- schema.field(from).toRight(s"${schema.name} has no column $from")
              _ <- validateName("column", to)
              _ <- schema.field(to) match
                case Some(other) if other.id != current.id || other.name == to => Left(s"${schema.name} already has a column $to")
                case _ => Right(())
            yield base.copy(fields = schema.fields.map(field => if field.id == current.id then field.copy(name = to) else field))
          case SchemaChange.DropField(name) =>
            for
              current <- schema.field(name).toRight(s"${schema.name} has no column $name")
              _ <- Either.cond(
                current.id != schema.primaryKey,
                (),
                s"$name is the primary key of ${schema.name} and cannot be dropped"
              )
              _ <- dependencies.dropRefusal(CatalogObjectId.Field(schema.entityId, current.id)).toLeft(())
            yield base.copy(fields = schema.fields.map(field => if field.id == current.id then field.copy(droppedIn = Some(version)) else field))
    next.map(SchemaEvolution(change, _, Compatibility.of(change)))

  /** Checks a `CREATE TABLE` definition: names, a BIGINT non-nullable primary key, distinct
    * column names and well-typed defaults.
    *
    * @return the reason the engine would refuse the definition, if any
    */
  def checkCreate(spec: EntitySpec): Option[String] =
    val checks: Iterator[Either[String, Unit]] = Iterator(
      validateName("table", spec.name),
      Either.cond(spec.fields.nonEmpty, (), s"${spec.name} needs at least one column"),
      Either.cond(spec.fields.size <= MaxFieldsPerEntity, (), s"an entity has at most $MaxFieldsPerEntity fields")
    ) ++ spec.fields.iterator.flatMap(field =>
      Iterator(validateName("column", field.name), validateDefault(field.name, field.dataType, field.default))
    ) ++ Iterator(
      spec.fields.groupBy(_.name.toLowerCase).collectFirst { case (_, Vector(first, _, _*)) => first.name }
        .toLeft(()).left.map(name => s"${spec.name} declares column $name twice"),
      spec.fields.find(_.name.equalsIgnoreCase(spec.primaryKey)) match
        case None => Left(s"primary key ${spec.primaryKey} is not a column of ${spec.name}")
        case Some(pk) if pk.dataType != DataType.Int64 || pk.nullable =>
          Left(s"primary key ${pk.name} must be a non-nullable BIGINT")
        case Some(_) => Right(())
    )
    checks.collectFirst { case Left(reason) => reason }

  /** Checks a name against the engine's identifier rule. */
  def validateName(kind: String, name: String): Either[String, Unit] =
    val wellFormed = name.nonEmpty && (name.head.isLetter || name.head == '_') && name.forall(c => c.isLetterOrDigit || c == '_')
    if !wellFormed then Left(s"$kind name '$name' must start with a letter or _ and hold only letters, digits and _")
    else if name.codePointCount(0, name.length) > MaxNameChars then Left(s"$kind name exceeds $MaxNameChars characters")
    else Right(())

  /** Checks that a default has the column's type (NULL is "no default", never a default value). */
  private def validateDefault(column: String, dataType: DataType, default: Option[DbValue]): Either[String, Unit] =
    default match
      case None => Right(())
      case Some(value) =>
        DbValue.dataType(value) match
          case Some(actual) if actual == dataType =>
            value match
              case DbValue.Float64Value(v) if v.isNaN || v.isInfinite => Left(s"DEFAULT of $column must be a finite number")
              case _ => Right(())
          case Some(actual) => Left(s"DEFAULT of $column is ${actual.sqlName}, the column is ${dataType.sqlName}")
          case None => Left(s"DEFAULT NULL of $column is not a value: omit the DEFAULT instead")
