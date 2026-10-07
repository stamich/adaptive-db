package io.adb.logical

import io.adb.catalog.Catalog
import io.adb.model.*
import io.adb.sql.*
import io.adb.sql.SqlExpr.*

/** Semantic analysis: resolves table and column names against the catalog, type-checks expressions and literals, and enforces the Milestone 2 statement restrictions.
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
      require(primaryKeys.size == 1, "Milestone 2 requires exactly one inline PRIMARY KEY")
      val fields = columns.map { c =>
        val tpe = DataType.parse(c.dataType).getOrElse(throw new IllegalArgumentException(s"unsupported type ${c.dataType}"))
        (c.name, tpe, c.nullable)
      }
      BoundCreateTable(name, fields, primaryKeys.head.name)

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

    case Select(columns, star, table, where, limit, asOf) =>
      val entity = requireEntity(table)
      val fields = if star then entity.fields else columns.map(requireField(entity, _))
      val predicate = where.map(bindExpr(entity, _))
      predicate.foreach(requireBoolean)
      BoundSelect(entity, fields, predicate, limit, asOf)

    case Update(table, assignments, where) =>
      val entity = requireEntity(table)
      val pk = extractPkEquality(entity, where)
      val resolved = assignments.map { case (name, expr) =>
        val field = requireField(entity, name)
        require(field.id != entity.primaryKey, "updating primary key is not supported in Milestone 2")
        field.id -> literalFor(field, expr)
      }.toMap
      BoundUpdate(entity, resolved, pk)

    case Delete(table, where) =>
      val entity = requireEntity(table)
      BoundDelete(entity, extractPkEquality(entity, where))

    case Explain(inner, analyze) => BoundExplain(bind(inner), analyze)

  /** Resolves a table name or fails with "unknown table". */
  private def requireEntity(name: String): Entity =
    catalog.entity(name).getOrElse(throw new IllegalArgumentException(s"unknown table $name"))

  /** Resolves a column of `entity` (case-insensitive) or fails with "unknown column". */
  private def requireField(entity: Entity, name: String): Field =
    entity.field(name).getOrElse(throw new IllegalArgumentException(s"unknown column ${entity.name}.$name"))

  /** Binds a WHERE expression to typed columns and literals, checking operand types. */
  private def bindExpr(entity: Entity, expr: SqlExpr): TypedExpr = expr match
    case Column(name) => TypedExpr.Column(requireField(entity, name))
    case LongLiteral(v) => TypedExpr.Literal(DbValue.Int64Value(v))
    case DoubleLiteral(v) => TypedExpr.Literal(DbValue.Float64Value(v))
    case StringLiteral(v) => TypedExpr.Literal(DbValue.StringValue(v))
    case BoolLiteral(v) => TypedExpr.Literal(DbValue.BoolValue(v))
    case NullLiteral => TypedExpr.Literal(DbValue.NullValue)
    case SqlExpr.Not(inner) =>
      val e = bindExpr(entity, inner); requireBoolean(e); TypedExpr.Not(e)
    case SqlExpr.Binary(left, op, right) =>
      val l = bindExpr(entity, left); val r = bindExpr(entity, right)
      val mapped = op match
        case SqlBinaryOp.Eq => BinaryOp.Eq; case SqlBinaryOp.Ne => BinaryOp.Ne
        case SqlBinaryOp.Lt => BinaryOp.Lt; case SqlBinaryOp.Le => BinaryOp.Le
        case SqlBinaryOp.Gt => BinaryOp.Gt; case SqlBinaryOp.Ge => BinaryOp.Ge
        case SqlBinaryOp.And => BinaryOp.And; case SqlBinaryOp.Or => BinaryOp.Or
      mapped match
        case BinaryOp.And | BinaryOp.Or => requireBoolean(l); requireBoolean(r)
        case _ => requireComparable(l, r)
      TypedExpr.Binary(l, mapped, r, Some(DataType.Bool))

  /** Converts a mutation value to a literal of the column's type; only literals are accepted and NULL only for nullable columns. */
  private def literalFor(field: Field, expr: SqlExpr): DbValue =
    val value = expr match
      case LongLiteral(v) => DbValue.Int64Value(v)
      case DoubleLiteral(v) => DbValue.Float64Value(v)
      case StringLiteral(v) => DbValue.StringValue(v)
      case BoolLiteral(v) => DbValue.BoolValue(v)
      case NullLiteral => DbValue.NullValue
      case _ => throw new IllegalArgumentException(s"Milestone 2 mutation values must be literals: $expr")
    if value == DbValue.NullValue then require(field.nullable, s"${field.name} is NOT NULL")
    else require(DbValue.dataType(value).contains(field.dataType), s"type mismatch for ${field.name}")
    value

  /** Extracts the key from a `WHERE pk = <BIGINT>` clause (either operand order), the only filter UPDATE and DELETE support. */
  private def extractPkEquality(entity: Entity, expr: SqlExpr): Long = expr match
    case SqlExpr.Binary(Column(name), SqlBinaryOp.Eq, LongLiteral(v)) if requireField(entity, name).id == entity.primaryKey => v
    case SqlExpr.Binary(LongLiteral(v), SqlBinaryOp.Eq, Column(name)) if requireField(entity, name).id == entity.primaryKey => v
    case _ => throw new IllegalArgumentException("UPDATE/DELETE in Milestone 2 require WHERE primary_key = BIGINT")

  /** Fails unless `expr` is boolean-typed. */
  private def requireBoolean(expr: TypedExpr): Unit =
    require(expr.dataType.contains(DataType.Bool), s"boolean expression required, got ${expr.dataType}")

  /** Fails unless both operands have the same type; NULL literals (untyped) compare with anything. */
  private def requireComparable(left: TypedExpr, right: TypedExpr): Unit =
    (left.dataType, right.dataType) match
      case (None, _) | (_, None) => ()
      case (Some(a), Some(b)) => require(a == b, s"incompatible types $a and $b")
