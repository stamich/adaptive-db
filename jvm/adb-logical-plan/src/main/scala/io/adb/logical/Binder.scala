package io.adb.logical

import io.adb.catalog.Catalog
import io.adb.model.*
import io.adb.sql.*
import io.adb.sql.SqlExpr.*

/** Semantic analysis: resolves table and column names against the catalog, type-checks
  * expressions and literals, and enforces the statement restrictions of the engine.
  *
  * SELECT statements are bound by [[SelectBinder]]; this class handles DDL, DML and EXPLAIN.
  *
  * @param catalog schema registry used for name resolution
  */
final class Binder(catalog: Catalog):
  /** Binds one parsed statement.
    *
    * @param statement AST produced by the SQL parser
    * @return the resolved, type-checked statement
    * @throws IllegalArgumentException on unknown names, type mismatches, or unsupported statement shapes
    */
  def bind(statement: Statement): BoundStatement = statement match
    case CreateTable(name, columns) =>
      val primaryKeys = columns.filter(_.primaryKey)
      require(primaryKeys.size == 1, "CREATE TABLE requires exactly one inline PRIMARY KEY")
      val fields = columns.map { c =>
        val tpe = DataType.parse(c.dataType).getOrElse(throw new IllegalArgumentException(s"unsupported type ${c.dataType}"))
        (c.name, tpe, c.nullable)
      }
      val references = columns.zip(fields).collect { case (column, (_, tpe, _)) if column.references.isDefined =>
        column.name -> bindReference(name, column.name, tpe, column.references.get)
      }.toMap
      BoundCreateTable(name, fields, primaryKeys.head.name, references)

    case Analyze(table) => BoundAnalyze(table.fold(catalog.entities)(name => Vector(requireEntity(name))))

    case SetOption(name, value) =>
      require(name.equalsIgnoreCase("optimizer"), s"unknown setting $name (supported: optimizer)")
      BoundSetOptimizer(OptimizerMode.named(value).getOrElse(throw new IllegalArgumentException(s"optimizer must be cost or rule, got $value")))

    case Insert(table, columns, values) =>
      val entity = requireEntity(table)
      val names = columns.getOrElse(entity.fields.map(_.name))
      require(names.size == values.size, "column/value count mismatch")
      val resolved = names.zip(values).map { case (name, expr) =>
        val field = requireField(entity, name)
        field.id -> literalFor(field, expr)
      }.toMap
      entity.fields.foreach { field =>
        if !field.nullable && !resolved.contains(field.id) then
          throw new IllegalArgumentException(s"missing non-null field ${field.name}")
      }
      BoundInsert(entity, resolved)

    case select: Select => new SelectBinder(catalog).bind(select)

    case Update(table, assignments, where) =>
      val entity = requireEntity(table)
      val pk = extractPkEquality(entity, where)
      val resolved = assignments.map { case (name, expr) =>
        val field = requireField(entity, name)
        require(field.id != entity.primaryKey, "updating the primary key is not supported")
        field.id -> literalFor(field, expr)
      }.toMap
      BoundUpdate(entity, resolved, pk)

    case Delete(table, where) =>
      val entity = requireEntity(table)
      BoundDelete(entity, extractPkEquality(entity, where))

    case Explain(inner, analyze) => BoundExplain(bind(inner), analyze)

  /** Resolves `REFERENCES table(column)` of `column` in the table being created: the target must
    * be another existing table's primary key with the same type.
    */
  private def bindReference(table: String, column: String, dataType: DataType, target: ColumnReference): ForeignKeyRef =
    require(!target.table.equalsIgnoreCase(table), s"$column: self-referencing foreign keys are not supported")
    val parent = requireEntity(target.table)
    val field = requireField(parent, target.column)
    require(field.id == parent.primaryKey, s"$column must reference the primary key of ${parent.name}, not ${field.name}")
    require(field.dataType == dataType, s"$column is ${dataType.sqlName} but ${parent.name}.${field.name} is ${field.dataType.sqlName}")
    ForeignKeyRef(parent.id, field.id)

  /** Resolves a table name or fails with "unknown table". */
  private def requireEntity(name: String): Entity =
    catalog.entity(name).getOrElse(throw new IllegalArgumentException(s"unknown table $name"))

  /** Resolves a column of `entity` (case-insensitive) or fails with "unknown column". */
  private def requireField(entity: Entity, name: String): Field =
    entity.field(name).getOrElse(throw new IllegalArgumentException(s"unknown column ${entity.name}.$name"))

  /** Converts a mutation value to a literal of the column's type; only literals are accepted and NULL only for nullable columns. */
  private def literalFor(field: Field, expr: SqlExpr): DbValue =
    val value = expr match
      case LongLiteral(v) => DbValue.Int64Value(v)
      case DoubleLiteral(v) => DbValue.Float64Value(v)
      case StringLiteral(v) => DbValue.StringValue(v)
      case BoolLiteral(v) => DbValue.BoolValue(v)
      case NullLiteral => DbValue.NullValue
      case _ => throw new IllegalArgumentException(s"mutation values must be literals: ${SqlExpr.render(expr)}")
    if value == DbValue.NullValue then require(field.nullable, s"${field.name} is NOT NULL")
    else require(DbValue.dataType(value).contains(field.dataType), s"type mismatch for ${field.name}")
    value

  /** Extracts the key from a `WHERE pk = <BIGINT>` clause (either operand order; the column may
    * be qualified with the table name), the only filter UPDATE and DELETE support.
    */
  private def extractPkEquality(entity: Entity, expr: SqlExpr): Long =
    /** Whether `column` names this entity's primary key. */
    def isPk(column: Column): Boolean =
      column.qualifier.forall(_.equalsIgnoreCase(entity.name)) && requireField(entity, column.name).id == entity.primaryKey
    expr match
      case SqlExpr.Binary(column: Column, SqlBinaryOp.Eq, LongLiteral(v)) if isPk(column) => v
      case SqlExpr.Binary(LongLiteral(v), SqlBinaryOp.Eq, column: Column) if isPk(column) => v
      case _ => throw new IllegalArgumentException("UPDATE/DELETE require WHERE primary_key = BIGINT")

/** Limits and expression helpers shared by the binders. */
object Binder:
  /** Largest LIMIT accepted (the native engine's LIMIT/TopK bound). */
  val MaxLimit: Int = 1_000_000

  /** Maps a SQL operator to its typed counterpart. */
  def mapOp(op: SqlBinaryOp): BinaryOp = op match
    case SqlBinaryOp.Eq => BinaryOp.Eq
    case SqlBinaryOp.Ne => BinaryOp.Ne
    case SqlBinaryOp.Lt => BinaryOp.Lt
    case SqlBinaryOp.Le => BinaryOp.Le
    case SqlBinaryOp.Gt => BinaryOp.Gt
    case SqlBinaryOp.Ge => BinaryOp.Ge
    case SqlBinaryOp.And => BinaryOp.And
    case SqlBinaryOp.Or => BinaryOp.Or

  /** Fails unless both operands have the same type; NULL literals (untyped) compare with anything. */
  def requireComparable(left: TypedExpr, right: TypedExpr): Unit =
    (left.dataType, right.dataType) match
      case (None, _) | (_, None) => ()
      case (Some(a), Some(b)) => require(a == b, s"incompatible types ${a.sqlName} and ${b.sqlName}")
