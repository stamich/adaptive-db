package io.adb.optimizer.access

import io.adb.catalog.{IndexInfo, IndexKeyColumn}
import io.adb.logical.*
import io.adb.model.*

/** One end of a range a matched predicate puts on a key column.
  *
  * @param value     bounding value (typed like the column, never NULL)
  * @param inclusive whether the value itself matches
  */
final case class RangeEnd(value: DbValue, inclusive: Boolean) derives CanEqual

/** What an index can do for the conjuncts of a filter over one relation.
  *
  * An index scan is exact for an *equality prefix* (the leading key columns compared with `=`
  * to constants) followed by at most one *range* on the next key column; every other conjunct
  * stays a residual filter above the scan.
  *
  * @param index      the index
  * @param equalities one `(key column, conjunct, value)` per leading key column bound by `=`
  * @param lower      lower end of the range on the next key column, with its conjunct
  * @param upper      upper end of that range, with its conjunct
  * @param residual   conjuncts the scan does not apply
  */
final case class IndexMatch(
    index: IndexInfo,
    equalities: Vector[(IndexKeyColumn, TypedExpr, DbValue)],
    lower: Option[(RangeEnd, TypedExpr)],
    upper: Option[(RangeEnd, TypedExpr)],
    residual: Vector[TypedExpr]
) derives CanEqual:
  /** Whether the scan narrows the index at all (otherwise it reads every entry). */
  def isRestricting: Boolean = equalities.nonEmpty || lower.isDefined || upper.isDefined

  /** The equality conjuncts, in key order. */
  def equalityConjuncts: Vector[TypedExpr] = equalities.map(_._2)

  /** The range conjuncts (lower, then upper). */
  def rangeConjuncts: Vector[TypedExpr] = lower.map(_._2).toVector ++ upper.map(_._2)

  /** The key column the range applies to, if any. */
  def rangeColumn: Option[IndexKeyColumn] =
    Option.when(lower.isDefined || upper.isDefined)(index.columns(equalities.size))

/** Matches the conjuncts of a filter over one relation against an index (see [[IndexMatch]]).
  *
  * Only comparisons between a stored column of the relation and a non-NULL literal are used; a
  * literal of another numeric type is converted when that is exact (an integer compared with a
  * DOUBLE column), otherwise the conjunct stays residual. Of several bounds on the range column
  * the tightest one is kept on each side; the others stay residual (they are implied, but
  * keeping them is always correct).
  */
object IndexPredicateMatcher:
  /** The match of `conjuncts` (over `relation`) against `index`; an index that matches nothing
    * still yields a match (a full scan, useful for its order or as a covering scan).
    */
  def matchIndex(relation: BoundRelation, index: IndexInfo, conjuncts: Vector[TypedExpr]): IndexMatch =
    val comparisons = conjuncts.map(conjunct => conjunct -> comparison(relation, conjunct))
    val used = scala.collection.mutable.Set.empty[TypedExpr]
    val equalities = Vector.newBuilder[(IndexKeyColumn, TypedExpr, DbValue)]
    var prefix = 0
    var searching = true
    while searching && prefix < index.columns.size do
      val column = index.columns(prefix)
      comparisons.collectFirst {
        case (conjunct, Some(Comparison(field, BinaryOp.Eq, value))) if field == column.field && !used.contains(conjunct) => conjunct -> value
      } match
        case Some((conjunct, value)) =>
          used += conjunct
          equalities += ((column, conjunct, value))
          prefix += 1
        case None => searching = false
    var lower = Option.empty[(RangeEnd, TypedExpr)]
    var upper = Option.empty[(RangeEnd, TypedExpr)]
    if prefix < index.columns.size then
      val column = index.columns(prefix)
      for (conjunct, found) <- comparisons; Comparison(field, op, value) <- found if field == column.field && !used.contains(conjunct) do
        op match
          case BinaryOp.Gt | BinaryOp.Ge =>
            val end = RangeEnd(value, op == BinaryOp.Ge)
            if lower.forall((current, _) => tighterLower(end, current)) then lower = Some(end -> conjunct)
          case BinaryOp.Lt | BinaryOp.Le =>
            val end = RangeEnd(value, op == BinaryOp.Le)
            if upper.forall((current, _) => tighterUpper(end, current)) then upper = Some(end -> conjunct)
          case _ => ()
      lower.foreach(used += _._2)
      upper.foreach(used += _._2)
    IndexMatch(index, equalities.result(), lower, upper, conjuncts.filterNot(used.contains))

  /** `column op literal` over a stored column of `relation`, normalized so the column is on the
    * left and the literal is typed like the column.
    */
  final case class Comparison(field: FieldId, op: BinaryOp, value: DbValue)

  /** The comparison `conjunct` expresses, if it is one an index can apply. */
  def comparison(relation: BoundRelation, conjunct: TypedExpr): Option[Comparison] = conjunct match
    case TypedExpr.Binary(TypedExpr.Column(attribute), op, TypedExpr.Literal(value), _) => normalized(relation, attribute, op, value)
    case TypedExpr.Binary(TypedExpr.Literal(value), op, TypedExpr.Column(attribute), _) => normalized(relation, attribute, flip(op), value)
    case _ => None

  /** The comparison of a stored column of `relation` with a convertible, non-NULL value. */
  private def normalized(relation: BoundRelation, attribute: Attribute, op: BinaryOp, value: DbValue): Option[Comparison] =
    val comparable = op match
      case BinaryOp.Eq | BinaryOp.Lt | BinaryOp.Le | BinaryOp.Gt | BinaryOp.Ge => true
      case _ => false
    attribute.origin match
      case ColumnOrigin.Stored(instance, _, field) if comparable && instance == relation.id =>
        convert(value, attribute.dataType).map(Comparison(field, op, _))
      case _ => None

  /** `value` typed as `dataType` when the conversion is exact; never NULL or NaN. */
  def convert(value: DbValue, dataType: DataType): Option[DbValue] = (value, dataType) match
    case (DbValue.NullValue, _) => None
    case (DbValue.Float64Value(v), _) if v.isNaN => None
    case (DbValue.Int64Value(v), DataType.Float64) if v.toDouble.toLong == v && math.abs(v) <= (1L << 53) => Some(DbValue.Float64Value(v.toDouble))
    case (v, t) if DbValue.dataType(v).contains(t) => Some(v)
    case _ => None

  /** Whether lower end `a` is at least as tight as `b`. */
  private def tighterLower(a: RangeEnd, b: RangeEnd): Boolean = compare(a.value, b.value) match
    case Some(c) => c > 0 || (c == 0 && !a.inclusive)
    case None => false

  /** Whether upper end `a` is at least as tight as `b`. */
  private def tighterUpper(a: RangeEnd, b: RangeEnd): Boolean = compare(a.value, b.value) match
    case Some(c) => c < 0 || (c == 0 && !a.inclusive)
    case None => false

  /** Order of two values of one type (strings and bytes by unsigned bytes, as the engine). */
  def compare(a: DbValue, b: DbValue): Option[Int] = (a, b) match
    case (DbValue.Int64Value(x), DbValue.Int64Value(y)) => Some(x.compare(y))
    case (DbValue.Float64Value(x), DbValue.Float64Value(y)) => Some(java.lang.Double.compare(x, y))
    case (DbValue.BoolValue(x), DbValue.BoolValue(y)) => Some(x.compare(y))
    case (DbValue.StringValue(x), DbValue.StringValue(y)) =>
      Some(java.util.Arrays.compareUnsigned(x.getBytes("UTF-8"), y.getBytes("UTF-8")))
    case (DbValue.BytesValue(x), DbValue.BytesValue(y)) => Some(java.util.Arrays.compareUnsigned(x, y))
    case _ => None

  /** The operator with its operands swapped (`5 < x` is `x > 5`). */
  private def flip(op: BinaryOp): BinaryOp = op match
    case BinaryOp.Lt => BinaryOp.Gt
    case BinaryOp.Le => BinaryOp.Ge
    case BinaryOp.Gt => BinaryOp.Lt
    case BinaryOp.Ge => BinaryOp.Le
    case other => other
