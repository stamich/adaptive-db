package io.adb.logical

import io.adb.catalog.Catalog
import io.adb.model.*
import io.adb.sql.*
import io.adb.sql.SqlExpr.*
import scala.collection.mutable

/** Binds one SELECT: relation scope, alias scope, slot allocation, qualified and ambiguous
  * column references, join conditions, aggregates and GROUP BY rules, ORDER BY and LIMIT.
  *
  * The result is a [[BoundSelect]] in which every column is an [[Attribute]] with its own slot.
  *
  * @param catalog schema registry used to resolve tables
  */
final class SelectBinder(catalog: Catalog):
  /** Binds `select`.
    *
    * @throws IllegalArgumentException on unknown or ambiguous names, type errors, invalid
    *         aggregate usage, or limits the engine would refuse
    */
  def bind(select: Select): BoundSelect = new Run(select).result

  /** State of one binding run. */
  private final class Run(select: Select):
    /** Relations and slots. */
    private val scope = SelectScope()
    /** Aggregates, deduplicated by function and input slot. */
    private val aggregates = mutable.LinkedHashMap.empty[(AggregateFunction, Option[SlotId]), BoundAggregate]

    /** The bound statement. */
    val result: BoundSelect =
      // Register every FROM item first, so a reference to a table joined later is reported as
      // "not visible" rather than "unknown".
      val base = scope.addRelation(requireEntity(select.from.base.table), aliasOf(select.from.base), outer = false)
      val joined = select.from.joins.map { join =>
        join -> scope.addRelation(requireEntity(join.table.table), aliasOf(join.table), outer = join.kind == JoinKind.Left)
      }
      // The ON condition of the n-th join sees the base relation and the first n joined ones.
      val joins = joined.zipWithIndex.map { case ((join, relation), index) =>
        val condition = join.on.map { on =>
          val bound = bindExpr(on, index + 2, "ON")
          requireBoolean(bound)
          bound
        }
        (relation, join.kind, condition)
      }
      val everything = scope.all.size

      val predicate = select.where.map { where =>
        val bound = bindExpr(where, everything, "WHERE")
        requireBoolean(bound)
        bound
      }

      val aggregateMode = select.groupBy.nonEmpty
        || select.items.exists(item => containsAggregate(item.expr))
        || select.orderBy.exists(item => containsAggregate(item.expr))
      val groupBy = select.groupBy.map(scope.resolve(_, everything)).distinctBy(_.slot)

      val output =
        if select.star then
          if aggregateMode then throw new IllegalArgumentException("SELECT * cannot be combined with GROUP BY or aggregates")
          val qualify = scope.all.size > 1
          scope.all.flatMap { relation =>
            relation.entity.fields.map { field =>
              val attribute = scope.attribute(relation, field)
              OutputColumn(if qualify then s"${relation.alias}.${field.name}" else field.name, attribute)
            }
          }
        else select.items.map(item => bindOutput(item, groupBy, aggregateMode))

      val orderBy = select.orderBy.map(item => BoundOrder(bindOrderKey(item.expr, output, groupBy, aggregateMode), item.descending))

      select.limit.foreach { limit =>
        if limit > Binder.MaxLimit then throw new IllegalArgumentException(s"LIMIT $limit exceeds ${Binder.MaxLimit}")
      }

      // Relations are materialized last: their column lists are complete only now.
      BoundSelect(
        base = base.bound,
        joins = joins.map { case (relation, kind, condition) => BoundJoin(joinType(kind), relation.bound, condition) },
        predicate = predicate,
        groupBy = groupBy,
        aggregates = aggregates.values.toVector,
        output = output,
        orderBy = orderBy,
        limit = select.limit,
        asOfVersion = select.asOfVersion
      )

    /** Binds one select-list entry to an output column. */
    private def bindOutput(item: SelectItem, groupBy: Vector[Attribute], aggregateMode: Boolean): OutputColumn =
      item.expr match
        case column: Column =>
          val attribute = scope.resolve(column, scope.all.size)
          if aggregateMode then requireGroupKey(attribute, column.display, groupBy)
          OutputColumn(item.alias.getOrElse(column.name), attribute)
        case call: AggregateCall =>
          val aggregate = bindAggregate(call)
          OutputColumn(item.alias.getOrElse(call.function.toString.toLowerCase), aggregate.output)
        case other =>
          throw new IllegalArgumentException(
            s"SELECT supports column references and aggregate functions only, got ${SqlExpr.render(other)}"
          )

    /** Binds an ORDER BY expression. An unqualified name that is an output column name (an alias
      * or a selected column) refers to that output column, as in standard SQL; otherwise the
      * expression must be an aggregate call or a column of the FROM relations.
      */
    private def bindOrderKey(
        expr: SqlExpr,
        output: Vector[OutputColumn],
        groupBy: Vector[Attribute],
        aggregateMode: Boolean
    ): Attribute = expr match
      case Column(name, None) if output.exists(_.name.equalsIgnoreCase(name)) =>
        output.filter(_.name.equalsIgnoreCase(name)).map(_.attribute).distinctBy(_.slot) match
          case Vector(attribute) => attribute
          case _ => throw new IllegalArgumentException(s"ORDER BY $name is ambiguous: several output columns are named $name")
      case call: AggregateCall => bindAggregate(call).output
      case column: Column =>
        val attribute = scope.resolve(column, scope.all.size)
        if aggregateMode then requireGroupKey(attribute, column.display, groupBy)
        attribute
      case other =>
        throw new IllegalArgumentException(
          s"ORDER BY supports columns, output names and aggregates only, got ${SqlExpr.render(other)}"
        )

    /** Binds an aggregate call, reusing an identical earlier one. */
    private def bindAggregate(call: AggregateCall): BoundAggregate =
      val function = call.function match
        case SqlAggregate.Count => AggregateFunction.Count
        case SqlAggregate.Sum => AggregateFunction.Sum
        case SqlAggregate.Min => AggregateFunction.Min
        case SqlAggregate.Max => AggregateFunction.Max
        case SqlAggregate.Avg => AggregateFunction.Avg
      val input = call.argument.map {
        case column: Column => scope.resolve(column, scope.all.size)
        case other =>
          throw new IllegalArgumentException(s"aggregate arguments must be column references, got ${SqlExpr.render(other)}")
      }
      aggregates.getOrElseUpdate(
        (function, input.map(_.slot)), {
          val (dataType, nullable) = resultType(function, input, call.display)
          BoundAggregate(function, input, scope.computed(call.display, dataType, nullable))
        }
      )

    /** Result type and nullability of an aggregate, or a type error. */
    private def resultType(function: AggregateFunction, input: Option[Attribute], display: String): (DataType, Boolean) =
      (function, input.map(_.dataType)) match
        case (AggregateFunction.Count, _) => (DataType.Int64, false)
        case (AggregateFunction.Sum, Some(DataType.Int64)) => (DataType.Int64, true)
        case (AggregateFunction.Sum, Some(DataType.Float64)) => (DataType.Float64, true)
        case (AggregateFunction.Avg, Some(DataType.Int64 | DataType.Float64)) => (DataType.Float64, true)
        case (AggregateFunction.Min | AggregateFunction.Max, Some(dataType)) => (dataType, true)
        case (_, dataType) =>
          throw new IllegalArgumentException(s"$display is not defined for ${dataType.fold("no argument")(_.sqlName)}")

    /** Fails unless `attribute` is a grouping attribute. */
    private def requireGroupKey(attribute: Attribute, written: String, groupBy: Vector[Attribute]): Unit =
      if !groupBy.exists(_.slot == attribute.slot) then
        throw new IllegalArgumentException(s"column $written must appear in GROUP BY or be used in an aggregate function")

    /** Binds a WHERE/ON expression against the first `visible` relations. */
    private def bindExpr(expr: SqlExpr, visible: Int, clause: String): TypedExpr = expr match
      case column: Column => TypedExpr.Column(scope.resolve(column, visible))
      case LongLiteral(v) => TypedExpr.Literal(DbValue.Int64Value(v))
      case DoubleLiteral(v) => TypedExpr.Literal(DbValue.Float64Value(v))
      case StringLiteral(v) => TypedExpr.Literal(DbValue.StringValue(v))
      case BoolLiteral(v) => TypedExpr.Literal(DbValue.BoolValue(v))
      case NullLiteral => TypedExpr.Literal(DbValue.NullValue)
      case SqlExpr.Not(inner) =>
        val bound = bindExpr(inner, visible, clause)
        requireBoolean(bound)
        TypedExpr.Not(bound)
      case SqlExpr.Binary(left, op, right) =>
        val l = bindExpr(left, visible, clause)
        val r = bindExpr(right, visible, clause)
        val mapped = Binder.mapOp(op)
        mapped match
          case BinaryOp.And | BinaryOp.Or => requireBoolean(l); requireBoolean(r)
          case _ => Binder.requireComparable(l, r)
        TypedExpr.Binary(l, mapped, r, Some(DataType.Bool))
      case call: AggregateCall =>
        throw new IllegalArgumentException(s"aggregate functions are not allowed in $clause: ${call.display}")

    /** Fails unless `expr` is boolean-typed. */
    private def requireBoolean(expr: TypedExpr): Unit =
      require(expr.dataType.contains(DataType.Bool), s"boolean expression required, got ${expr.dataType.fold("NULL")(_.sqlName)}")

    /** Whether an expression contains an aggregate call. */
    private def containsAggregate(expr: SqlExpr): Boolean = expr match
      case _: AggregateCall => true
      case SqlExpr.Binary(left, _, right) => containsAggregate(left) || containsAggregate(right)
      case SqlExpr.Not(inner) => containsAggregate(inner)
      case _ => false

    /** Resolves a table name or fails with "unknown table". */
    private def requireEntity(name: String): Entity =
      catalog.entity(name).getOrElse(throw new IllegalArgumentException(s"unknown table $name"))

    /** Name a FROM item is referred to by: its alias, else its table name. */
    private def aliasOf(table: TableRef): String = table.alias.getOrElse(table.table)

    /** Maps the SQL join kind to the logical join type. */
    private def joinType(kind: JoinKind): JoinType = kind match
      case JoinKind.Inner => JoinType.Inner
      case JoinKind.Left => JoinType.Left
      case JoinKind.Cross => JoinType.Cross
