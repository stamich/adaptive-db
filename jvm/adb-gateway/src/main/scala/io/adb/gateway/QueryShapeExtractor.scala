package io.adb.gateway

import io.adb.logical.{BinaryOp, BoundSelect, TypedExpr}
import io.adb.model.{Attribute, ColumnOrigin}
import io.adb.telemetry.{FieldRef, JoinEdge, OrderRef, QueryShape}

/** Derives the [[QueryShape]] of a bound SELECT.
  *
  * This runs when a statement is **planned** (and its result is kept in the cached plan), never
  * per execution of a cached statement. It reads only the bound tree, so the shape contains ids and
  * flags and no literal value or SQL text.
  */
object QueryShapeExtractor:
  /** Orders field references by entity, then field. */
  private given Ordering[FieldRef] = Ordering.by(f => (f.entity, f.field))

  /** The shape of `select`. */
  def extract(select: BoundSelect): QueryShape =
    val entities = select.relations.map(_.entity.id.value).distinct.sorted
    val filters = select.predicate.toVector.flatMap(_.attributes).flatMap(stored).distinct.sorted
    val edges = select.joins.flatMap(_.condition).flatMap(TypedExpr.conjuncts).flatMap(edge).distinct.sortBy(e => (e.leftEntity, e.leftField, e.rightEntity, e.rightField))
    val order = select.orderBy.flatMap(o => stored(o.attribute).map(OrderRef(_, o.descending)))
    QueryShape(entities, filters, edges, order, select.isAggregate, select.limit.isDefined)

  /** The stored column behind `attribute`, if it is one (aggregate results are not). */
  private def stored(attribute: Attribute): Option[FieldRef] = attribute.origin match
    case ColumnOrigin.Stored(_, entity, field) => Some(FieldRef(entity.value, field.value))
    case ColumnOrigin.Computed(_) => None

  /** The equality join `a = b` between two stored columns, smaller side first; other conjuncts
    * (inequalities, constants) are not join edges.
    */
  private def edge(expr: TypedExpr): Option[JoinEdge] = expr match
    case TypedExpr.Binary(TypedExpr.Column(a), BinaryOp.Eq, TypedExpr.Column(b), _) =>
      for l <- stored(a); r <- stored(b) yield
        val (x, y) = if summon[Ordering[FieldRef]].lteq(l, r) then (l, r) else (r, l)
        JoinEdge(x.entity, x.field, y.entity, y.field)
    case _ => None
