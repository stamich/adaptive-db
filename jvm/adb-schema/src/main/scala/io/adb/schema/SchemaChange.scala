package io.adb.schema

/** One schema change of an existing entity (`ALTER TABLE`), in the form the engine accepts
  * (`adb_alter_entity_json`). Every change creates a new schema version without rewriting rows:
  * stored rows are adapted when read and migrated when next written.
  */
sealed trait SchemaChange derives CanEqual:
  /** The change as SQL, for messages and `SHOW SCHEMA` (e.g. `RENAME COLUMN a TO b`). */
  def sql: String

/** The change variants. */
object SchemaChange:
  /** `ADD COLUMN`: a nullable field, or a `NOT NULL` field with a default. Rows written before
    * the change read the default ("virtual default") until they are rewritten.
    */
  final case class AddField(field: NewField) extends SchemaChange:
    def sql: String =
      val nullability = if field.nullable then "" else " NOT NULL"
      val default = field.default.fold("")(value => s" DEFAULT ${SchemaJson.sqlLiteral(value)}")
      s"ADD COLUMN ${field.name} ${field.dataType.sqlName}$nullability$default"

  /** `RENAME COLUMN from TO to`: metadata only; the field keeps its id, so stored rows,
    * indexes and statistics are unaffected.
    */
  final case class RenameField(from: String, to: String) extends SchemaChange:
    def sql: String = s"RENAME COLUMN $from TO $to"

  /** `DROP COLUMN name [RESTRICT]`: a logical drop. Stored values are hidden from reads of the
    * new version but kept in the log (`AS OF` older snapshots still see them). Refused while the
    * field is the primary key or part of an index (RESTRICT is the only behaviour).
    */
  final case class DropField(name: String) extends SchemaChange:
    def sql: String = s"DROP COLUMN $name RESTRICT"

/** How a change affects data that already exists. */
enum Compatibility derives CanEqual:
  /** Only metadata changes (a rename): stored rows read exactly as before. */
  case MetadataOnly
  /** A field is added: rows stored earlier read its default until rewritten. */
  case VirtualDefault
  /** A field is hidden: stored values remain in the log but no longer appear in reads. */
  case LogicalDrop

/** Compatibility classification of changes. */
object Compatibility:
  /** The compatibility class of `change`. */
  def of(change: SchemaChange): Compatibility = change match
    case _: SchemaChange.RenameField => MetadataOnly
    case _: SchemaChange.AddField => VirtualDefault
    case _: SchemaChange.DropField => LogicalDrop
