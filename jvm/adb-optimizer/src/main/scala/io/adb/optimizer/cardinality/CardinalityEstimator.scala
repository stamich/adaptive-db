package io.adb.optimizer.cardinality

import io.adb.catalog.Catalog
import io.adb.logical.*
import io.adb.logical.LogicalPlan.*
import io.adb.model.*
import io.adb.statistics.*
import scala.collection.mutable

/** Status of one relation's statistics, as reported by EXPLAIN. */
enum StatisticsStatus derives CanEqual:
  /** The entity was never analyzed. */
  case Missing
  /** Statistics exist and are fresh. */
  case Fresh(statistics: EntityStatistics)
  /** Statistics exist but too many rows changed since. */
  case Stale(statistics: EntityStatistics)

/** Estimates how many rows each logical operator produces.
  *
  * The rules, in the order the estimator applies them:
  *
  *  - '''scan''': the analyzed row count; without statistics [[EstimationConfig.defaultRows]]
  *    at confidence 0.1;
  *  - '''`column = value`''': `1 / rows` on a primary key; the sampled frequency of a most common
  *    value; otherwise the non-common rows spread evenly over the non-common distinct values,
  *    `(nonNull − mcvRows) / (distinct − mcvCount)`; zero outside `[min, max]`;
  *  - '''ranges''': the histogram fraction below the value (linear interpolation inside a
  *    numeric bucket, half a bucket otherwise); without a histogram, interpolation between
  *    min and max; the configured default otherwise;
  *  - '''`AND` / `OR` / `NOT`''': `a·b`, `a + b − a·b`, `1 − a` (independence);
  *  - '''inner join''': with a declared foreign key `child.fk = parent.pk`, every non-NULL child
  *    row matches at most one parent row, so `|child| · (1 − nullFraction) · |parent| / |parent table|`;
  *    otherwise `|L| · |R| / max(distinct(l), distinct(r))` per equality key, each distinct
  *    count capped by its side's rows; LEFT JOIN returns at least `|L|`;
  *  - '''aggregate''': the product of the grouping columns' distinct counts, capped by the
  *    input rows; exactly one row without GROUP BY;
  *  - '''limit''': `min(limit, input)`.
  *
  * Non-limit estimates never drop below one row when the input has rows: a zero would make
  * every plan above it look free. Estimates are memoized per plan node (by identity), and
  * statistics are fetched at most once per entity, so one estimator serves one planning run.
  *
  * @param statistics source of statistics
  * @param catalog    schemas (primary keys and foreign-key hints)
  * @param config     defaults
  */
final class CardinalityEstimator(statistics: StatisticsProvider, catalog: Catalog, val config: EstimationConfig = EstimationConfig()):
  /** Statistics fetched so far, by entity. */
  private val fetched = mutable.Map.empty[EntityId, Option[EntityStatistics]]
  /** Estimates computed so far, by plan node identity. */
  private val memo = new java.util.IdentityHashMap[LogicalPlan, Estimate]()

  /** Statistics of `entity` (fetched once). */
  def entityStatistics(entity: EntityId): Option[EntityStatistics] =
    fetched.getOrElseUpdate(entity, statistics.statistics(entity))

  /** Freshness of the statistics behind `relation`. */
  def status(relation: BoundRelation): StatisticsStatus =
    entityStatistics(relation.entity.id) match
      case None => StatisticsStatus.Missing
      case Some(s) if s.freshness(config.staleThreshold) == Freshness.Stale => StatisticsStatus.Stale(s)
      case Some(s) => StatisticsStatus.Fresh(s)

  /** Rows of the whole entity (analyzed count, or the default). */
  def tableRows(entity: EntityId): Double =
    entityStatistics(entity).fold(config.defaultRows)(_.table.rowCount.toDouble)

  /** Estimated output of `plan`. */
  def estimate(plan: LogicalPlan): Estimate =
    val known = memo.get(plan)
    if known != null then known
    else
      val computed = compute(plan)
      memo.put(plan, computed)
      computed

  /** Estimates one operator from its inputs' estimates. */
  private def compute(plan: LogicalPlan): Estimate = plan match
    case TableScan(relation) => scan(relation)
    case PointLookup(relation, _) =>
      val base = scan(relation)
      Estimate.bounded(math.min(1.0, base.rows), base.confidence * 0.9, base.source)
    case Filter(input, predicate) =>
      val in = estimate(input)
      val informed = predicate.attributes.forall(columnStatistics(_).isDefined)
      Estimate.bounded(atLeastOne(in.rows * selectivity(predicate), in.rows), in.confidence * (if informed then 0.9 else 0.5), in.source)
    case Join(left, right, joinType, condition) =>
      join(joinType, estimate(left), estimate(right), left.output.map(_.slot).toSet, right.output.map(_.slot).toSet,
        condition.toVector.flatMap(TypedExpr.conjuncts))
    case Aggregate(input, groupBy, _) =>
      val in = estimate(input)
      if groupBy.isEmpty then Estimate.bounded(1, in.confidence, in.source)
      else
        val groups = groupBy.map(attribute => distinct(attribute, in.rows)).product
        val informed = groupBy.forall(columnStatistics(_).isDefined)
        Estimate.bounded(atLeastOne(math.min(groups, in.rows), in.rows), in.confidence * (if informed then 0.8 else 0.5), in.source)
    case Sort(input, _) => estimate(input)
    case Project(input, _) => estimate(input)
    case Limit(input, limit) =>
      val in = estimate(input)
      Estimate.bounded(math.min(limit.toDouble, in.rows), in.confidence, in.source)

  /** Estimate of one relation scan. */
  private def scan(relation: BoundRelation): Estimate = status(relation) match
    case StatisticsStatus.Missing => Estimate.bounded(config.defaultRows, 0.1, EstimateSource.Default)
    case StatisticsStatus.Fresh(s) => Estimate.bounded(s.table.rowCount.toDouble, s.confidence(config.staleThreshold), EstimateSource.Statistics)
    case StatisticsStatus.Stale(s) => Estimate.bounded(s.table.rowCount.toDouble, s.confidence(config.staleThreshold), EstimateSource.Stale)

  /** Output estimate of a join of `left` and `right` under `conjuncts`.
    *
    * Used both for existing joins and by join ordering for candidate joins of relation sets.
    *
    * @param leftSlots  slots the left input produces
    * @param rightSlots slots the right input produces
    */
  def join(
      joinType: JoinType,
      left: Estimate,
      right: Estimate,
      leftSlots: Set[SlotId],
      rightSlots: Set[SlotId],
      conjuncts: Vector[TypedExpr]
  ): Estimate =
    val source = EstimateSource.weakest(left.source, right.source)
    val confidence = math.min(left.confidence, right.confidence)
    val cross = left.rows * right.rows
    if joinType == JoinType.Cross || conjuncts.isEmpty then Estimate.bounded(cross, confidence, source)
    else
      val keys = conjuncts.flatMap(part => equiKey(part, leftSlots, rightSlots).map(part -> _))
      val others = conjuncts.filterNot(part => keys.exists(_._1 eq part))
      val othersSelectivity = others.map(selectivity).product
      val foreignKey = keys.iterator.flatMap((part, key) => foreignKeyRows(key, left.rows, right.rows).map(part -> _)).nextOption()
      val (rows, factor) = foreignKey match
        case Some((used, matched)) =>
          val rest = keys.filterNot(_._1 eq used).map((_, key) => keySelectivity(key, left.rows, right.rows)).product
          (matched * rest * othersSelectivity, 0.95)
        case None =>
          val keyed = keys.map((_, key) => keySelectivity(key, left.rows, right.rows)).product
          val informed = keys.nonEmpty && keys.forall((_, key) => columnStatistics(key._1).isDefined && columnStatistics(key._2).isDefined)
          (cross * keyed * othersSelectivity, if informed then 0.8 else 0.5)
      val inner = atLeastOne(rows, math.min(left.rows, right.rows))
      val result = if joinType == JoinType.Left then math.max(inner, left.rows) else inner
      Estimate.bounded(result, confidence * factor, source)

  /** Fraction of rows satisfying `predicate` (independence between conjuncts). */
  def selectivity(predicate: TypedExpr): Double = clamp(predicate match
    case TypedExpr.Binary(left, BinaryOp.And, right, _) => selectivity(left) * selectivity(right)
    case TypedExpr.Binary(left, BinaryOp.Or, right, _) =>
      val (a, b) = (selectivity(left), selectivity(right))
      a + b - a * b
    case TypedExpr.Not(inner) => 1 - selectivity(inner)
    case TypedExpr.Literal(DbValue.BoolValue(value)) => if value then 1.0 else 0.0
    case TypedExpr.Literal(_) => 0.0
    case TypedExpr.Column(attribute) => 0.5 * (1 - nullFraction(attribute))
    case TypedExpr.Binary(TypedExpr.Column(attribute), op, TypedExpr.Literal(value), _) => compare(attribute, op, value)
    case TypedExpr.Binary(TypedExpr.Literal(value), op, TypedExpr.Column(attribute), _) => compare(attribute, flip(op), value)
    case TypedExpr.Binary(TypedExpr.Column(a), BinaryOp.Eq, TypedExpr.Column(b), _) => columnEquality(a, b)
    case TypedExpr.Binary(TypedExpr.Column(a), BinaryOp.Ne, TypedExpr.Column(b), _) => 1 - columnEquality(a, b)
    case _ => config.defaultRangeSelectivity)

  /** What the selectivity of each comparison in `predicate` rests on, for EXPLAIN:
    * `primary key`, `mcv` (a most common value), `ndv` (distinct count), `histogram`,
    * `min/max`, `null` (comparison with NULL), `constant` or `default`.
    */
  def basis(predicate: TypedExpr): Vector[String] = (predicate match
    case TypedExpr.Binary(left, BinaryOp.And | BinaryOp.Or, right, _) => basis(left) ++ basis(right)
    case TypedExpr.Not(inner) => basis(inner)
    case TypedExpr.Binary(TypedExpr.Column(attribute), op, TypedExpr.Literal(value), _) => Vector(comparisonBasis(attribute, op, value))
    case TypedExpr.Binary(TypedExpr.Literal(value), op, TypedExpr.Column(attribute), _) => Vector(comparisonBasis(attribute, flip(op), value))
    case TypedExpr.Binary(TypedExpr.Column(a), BinaryOp.Eq | BinaryOp.Ne, TypedExpr.Column(b), _) =>
      Vector(if columnStatistics(a).isDefined || columnStatistics(b).isDefined then "ndv" else "default")
    case TypedExpr.Literal(_) => Vector("constant")
    case _ => Vector("default")).distinct

  /** What the estimate of `join` rests on: `foreign key`, `ndv`, `default` or `cross product`. */
  def joinBasis(join: Join): String =
    val conjuncts = join.condition.toVector.flatMap(TypedExpr.conjuncts)
    val leftSlots = join.left.output.map(_.slot).toSet
    val rightSlots = join.right.output.map(_.slot).toSet
    val keys = conjuncts.flatMap(equiKey(_, leftSlots, rightSlots))
    if join.joinType == JoinType.Cross || conjuncts.isEmpty then "cross product"
    else if keys.exists(key => foreignKeyRows(key, 1, 1).isDefined) then "foreign key"
    else if keys.exists((l, r) => columnStatistics(l).isDefined || columnStatistics(r).isDefined || isKey(l) || isKey(r)) then "ndv"
    else "default"

  /** Basis of one `attribute op value` comparison (see [[basis]]). */
  private def comparisonBasis(attribute: Attribute, op: BinaryOp, value: DbValue): String =
    if value == DbValue.NullValue then "null"
    else if isKey(attribute) && (op == BinaryOp.Eq || op == BinaryOp.Ne) then "primary key"
    else columnStatistics(attribute) match
      case None => "default"
      case Some((_, column)) if op == BinaryOp.Eq || op == BinaryOp.Ne =>
        if column.mostCommon.exists(common => same(common.value, value)) then "mcv" else if outside(column, value) then "min/max" else "ndv"
      case Some((_, column)) =>
        if column.histogram.nonEmpty then "histogram" else if column.min.isDefined && column.max.isDefined then "min/max" else "default"

  /** Whether `attribute` is a stored primary-key column. */
  private def isKey(attribute: Attribute): Boolean = attribute.origin match
    case ColumnOrigin.Stored(_, entity, field) => isPrimaryKey(entity, field)
    case ColumnOrigin.Computed(_) => false

  /** Distinct non-NULL values of `attribute` among `rows` rows. */
  def distinct(attribute: Attribute, rows: Double): Double =
    val all = attribute.origin match
      case ColumnOrigin.Stored(_, entity, field) if isPrimaryKey(entity, field) => tableRows(entity)
      case _ => columnStatistics(attribute).fold(config.defaultDistinct)(_._2.distinctCount.toDouble)
    math.max(1.0, math.min(all, rows))

  /** Fraction of NULLs of `attribute` (0 without statistics). */
  def nullFraction(attribute: Attribute): Double = columnStatistics(attribute) match
    case Some((table, column)) if table.rowCount > 0 => column.nullCount.toDouble / table.rowCount
    case _ => 0.0

  /** `(table, column)` statistics behind a stored attribute. */
  private def columnStatistics(attribute: Attribute): Option[(TableStatistics, ColumnStatistics)] = attribute.origin match
    case ColumnOrigin.Stored(_, entity, field) =>
      entityStatistics(entity).flatMap(s => s.table.column(field).map(s.table -> _))
    case ColumnOrigin.Computed(_) => None

  /** Selectivity of `attribute op value`. */
  private def compare(attribute: Attribute, op: BinaryOp, value: DbValue): Double =
    if value == DbValue.NullValue then 0.0
    else
      val primaryKey = attribute.origin match
        case ColumnOrigin.Stored(_, entity, field) if isPrimaryKey(entity, field) => Some(entity)
        case _ => None
      (op, primaryKey, columnStatistics(attribute)) match
        case (BinaryOp.Eq, Some(entity), _) => 1.0 / math.max(1.0, tableRows(entity))
        case (BinaryOp.Ne, Some(entity), _) => 1.0 - 1.0 / math.max(1.0, tableRows(entity))
        case (_, _, None) => if op == BinaryOp.Eq then config.defaultEqualitySelectivity else config.defaultRangeSelectivity
        case (BinaryOp.Eq, _, Some((table, column))) => equality(table, column, value)
        case (BinaryOp.Ne, _, Some((table, column))) => nonNullFraction(table, column) - equality(table, column, value)
        case (BinaryOp.Lt, _, Some((table, column))) => below(table, column, value, inclusive = false)
        case (BinaryOp.Le, _, Some((table, column))) => below(table, column, value, inclusive = true)
        case (BinaryOp.Gt, _, Some((table, column))) => nonNullFraction(table, column) - below(table, column, value, inclusive = true)
        case (BinaryOp.Ge, _, Some((table, column))) => nonNullFraction(table, column) - below(table, column, value, inclusive = false)
        case _ => config.defaultRangeSelectivity

  /** Selectivity of `column = value` from statistics. */
  private def equality(table: TableStatistics, column: ColumnStatistics, value: DbValue): Double =
    if table.rowCount == 0 then 0.0
    else
      val rows = table.rowCount.toDouble
      column.mostCommon.find(common => same(common.value, value)) match
        case Some(common) => common.rows / rows
        case None if outside(column, value) => 0.0
        case None =>
          val rest = (table.rowCount - column.nullCount - column.mostCommonRows).max(0).toDouble
          val restDistinct = (column.distinctCount - column.mostCommon.size).max(1).toDouble
          rest / restDistinct / rows

  /** Fraction of rows whose value is below `value` (or equal when `inclusive`). */
  private def below(table: TableStatistics, column: ColumnStatistics, value: DbValue, inclusive: Boolean): Double =
    val nonNull = nonNullFraction(table, column)
    val histogramRows = column.histogram.iterator.map(_.rows).sum
    if column.histogram.nonEmpty && histogramRows > 0 then
      val covered = column.histogram.iterator.map { bucket =>
        val share =
          if order(bucket.upper, value).exists(c => c < 0 || (inclusive && c == 0)) then 1.0
          else if order(bucket.lower, value).exists(c => c > 0 || (!inclusive && c == 0)) then 0.0
          else interpolate(bucket.lower, bucket.upper, value).getOrElse(0.5)
        share * bucket.rows
      }.sum
      nonNull * covered / histogramRows
    else
      (column.min, column.max) match
        case (Some(min), Some(max)) =>
          if order(value, min).exists(c => c < 0 || (!inclusive && c == 0)) then 0.0
          else if order(value, max).exists(c => c > 0 || (inclusive && c == 0)) then nonNull
          else nonNull * interpolate(min, max, value).getOrElse(config.defaultRangeSelectivity)
        case _ => config.defaultRangeSelectivity

  /** Position of `value` inside `[lower, upper]` for numeric values. */
  private def interpolate(lower: DbValue, upper: DbValue, value: DbValue): Option[Double] =
    (numeric(lower), numeric(upper), numeric(value)) match
      case (Some(lo), Some(hi), Some(v)) if hi > lo => Some(((v - lo) / (hi - lo)).max(0).min(1))
      case (Some(lo), Some(hi), Some(_)) if hi == lo => Some(0.5)
      case _ => None

  /** Whether `value` lies outside the column's `[min, max]`. */
  private def outside(column: ColumnStatistics, value: DbValue): Boolean =
    column.min.flatMap(order(value, _)).exists(_ < 0) || column.max.flatMap(order(value, _)).exists(_ > 0)

  /** Fraction of non-NULL rows. */
  private def nonNullFraction(table: TableStatistics, column: ColumnStatistics): Double =
    if table.rowCount == 0 then 0.0 else (table.rowCount - column.nullCount).toDouble / table.rowCount

  /** `a = b` between two columns: `1 / max(distinct)`, scaled by both non-NULL fractions. */
  private def columnEquality(a: Attribute, b: Attribute): Double =
    val distinctA = distinct(a, Estimate.MaxRows)
    val distinctB = distinct(b, Estimate.MaxRows)
    (1 - nullFraction(a)) * (1 - nullFraction(b)) / math.max(distinctA, distinctB)

  /** Selectivity of one equality join key over inputs of `leftRows` and `rightRows`. */
  private def keySelectivity(key: (Attribute, Attribute), leftRows: Double, rightRows: Double): Double =
    val (l, r) = key
    (1 - nullFraction(l)) * (1 - nullFraction(r)) / math.max(distinct(l, leftRows), distinct(r, rightRows))

  /** Join rows implied by a foreign-key hint on `key`, if one side references the other's
    * primary key: each non-NULL child row matches one parent row, scaled by the parent side's
    * rows per parent-table row. That ratio is below one for a filtered parent side and above
    * one when the parent side is itself a join that repeats parent rows.
    */
  private def foreignKeyRows(key: (Attribute, Attribute), leftRows: Double, rightRows: Double): Option[Double] =
    val (l, r) = key
    /** Rows when `child` references `parent`'s primary key. */
    def rows(child: Attribute, parent: Attribute, childRows: Double, parentRows: Double): Option[Double] =
      (child.origin, parent.origin) match
        case (ColumnOrigin.Stored(_, childEntity, childField), ColumnOrigin.Stored(_, parentEntity, parentField))
            if references(childEntity, childField).contains(ForeignKeyRef(parentEntity, parentField)) =>
          val parentShare = parentRows / math.max(1.0, tableRows(parentEntity))
          Some(childRows * (1 - nullFraction(child)) * parentShare)
        case _ => None
    rows(l, r, leftRows, rightRows).orElse(rows(r, l, rightRows, leftRows))

  /** Foreign-key hint declared on a field. */
  private def references(entity: EntityId, field: FieldId): Option[ForeignKeyRef] =
    catalog.entity(entity).flatMap(_.fieldsById.get(field)).flatMap(_.references)

  /** Whether `field` is the primary key of `entity`. */
  private def isPrimaryKey(entity: EntityId, field: FieldId): Boolean =
    catalog.entity(entity).exists(_.primaryKey == field)

  /** `left = right` with one attribute from each side, oriented left-to-right. */
  private def equiKey(part: TypedExpr, leftSlots: Set[SlotId], rightSlots: Set[SlotId]): Option[(Attribute, Attribute)] =
    part match
      case TypedExpr.Binary(TypedExpr.Column(a), BinaryOp.Eq, TypedExpr.Column(b), _) =>
        if leftSlots(a.slot) && rightSlots(b.slot) then Some(a -> b)
        else if leftSlots(b.slot) && rightSlots(a.slot) then Some(b -> a)
        else None
      case _ => None

  /** `rows`, but at least one when the input has rows. */
  private def atLeastOne(rows: Double, inputRows: Double): Double =
    if inputRows >= 1 then math.max(1.0, rows) else rows

  /** A selectivity forced into `[0, 1]`. */
  private def clamp(value: Double): Double = if value.isNaN then 1.0 else value.max(0).min(1)

  /** The comparison operator with its operands swapped. */
  private def flip(op: BinaryOp): BinaryOp = op match
    case BinaryOp.Lt => BinaryOp.Gt
    case BinaryOp.Le => BinaryOp.Ge
    case BinaryOp.Gt => BinaryOp.Lt
    case BinaryOp.Ge => BinaryOp.Le
    case other => other

  /** Numeric view of a value. */
  private def numeric(value: DbValue): Option[Double] = value match
    case DbValue.Int64Value(v) => Some(v.toDouble)
    case DbValue.Float64Value(v) => Some(v)
    case _ => None

  /** Value equality as SQL sees it: numerically across BIGINT and DOUBLE, by content for BYTES. */
  private def same(a: DbValue, b: DbValue): Boolean = (a, b) match
    case (DbValue.BytesValue(x), DbValue.BytesValue(y)) => java.util.Arrays.equals(x, y)
    case _ => order(a, b).contains(0)

  /** Order of two values of the same comparable type (`None` across types). */
  private def order(a: DbValue, b: DbValue): Option[Int] = (a, b) match
    case (DbValue.Int64Value(x), DbValue.Int64Value(y)) => Some(java.lang.Long.compare(x, y))
    case (DbValue.Float64Value(x), DbValue.Float64Value(y)) => Some(java.lang.Double.compare(x, y))
    case (DbValue.Int64Value(x), DbValue.Float64Value(y)) => Some(java.lang.Double.compare(x.toDouble, y))
    case (DbValue.Float64Value(x), DbValue.Int64Value(y)) => Some(java.lang.Double.compare(x, y.toDouble))
    case (DbValue.StringValue(x), DbValue.StringValue(y)) => Some(Integer.signum(x.compareTo(y)))
    case (DbValue.BoolValue(x), DbValue.BoolValue(y)) => Some(java.lang.Boolean.compare(x, y))
    case _ => None
