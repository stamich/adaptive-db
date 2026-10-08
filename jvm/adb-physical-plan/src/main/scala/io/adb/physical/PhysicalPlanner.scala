package io.adb.physical

import io.adb.logical.*
import io.adb.logical.LogicalPlan as LPlan
import io.adb.model.*
import io.adb.optimizer.cardinality.{CardinalityEstimator, StatisticsStatus}
import io.adb.optimizer.cost.CostModel
import io.adb.physical.PhysicalPlan as PPlan

/** Estimated output and cost of one physical node.
  *
  * @param rows       estimated rows
  * @param cost       weighted cumulative cost of the node and its inputs
  * @param confidence how far the row estimate can be trusted, in `(0, 1]`
  * @param source     what the estimate rests on
  */
final case class NodeEstimate(rows: Double, cost: Double, confidence: Double, source: String) derives CanEqual

/** Estimator and cost model used to annotate a plan. */
final case class PlanEstimation(estimator: CardinalityEstimator, costModel: CostModel)

/** The result of physical planning.
  *
  * @param plan      executable plan
  * @param decisions strategy decisions with reasons (shown by EXPLAIN)
  * @param slotNames display name of every slot (EXPLAIN renders `name#slot`)
  * @param estimates estimates by pre-order node id (see [[PhysicalPlan.preorder]]); the engine's
  *                  runtime profile uses the same ids, so EXPLAIN ANALYZE can compare them
  * @param warnings  problems found while planning (missing or stale statistics, plans the
  *                  estimates say exceed an engine limit)
  */
final case class PlannedQuery(
    plan: PhysicalPlan,
    decisions: Vector[PlanDecision],
    slotNames: Map[SlotId, String],
    estimates: Map[Int, NodeEstimate] = Map.empty,
    warnings: Vector[String] = Vector.empty
)

/** Converts optimized logical plans into the physical plan vocabulary of the native engine.
  *
  *  - a table scan is a native `EntityScan` over the entity's key range, reading only the
  *    referenced fields into their slots;
  *  - a join becomes a `HashJoin` when its condition contains `left = right` equalities
  *    (the rest stays as residual) and a `NestedLoopJoin` otherwise, as chosen by the
  *    [[PlanningPolicy]];
  *  - `LIMIT` over `ORDER BY` (optionally with a projection in between) becomes a `TopK`;
  *  - with a [[PlanEstimation]], every physical node is annotated with the estimate of the
  *    logical operator it implements, and missing or stale statistics become warnings.
  *
  * The aliases `LPlan` and `PPlan` keep logical and physical operators in separate namespaces.
  */
object PhysicalPlanner:
  /** Builds an executable physical plan from a bound and optimized logical plan.
    *
    * @param logical    optimized logical plan
    * @param policy     strategy choices
    * @param estimation estimator and cost model to annotate the plan with, if any
    */
  def plan(logical: LPlan, policy: PlanningPolicy = DefaultPlanningPolicy, estimation: Option[PlanEstimation] = None): PlannedQuery =
    val run = Run(policy, estimation)
    val physical = run.translate(logical)
    val estimates = PPlan.preorder(physical).zipWithIndex.flatMap((node, id) => Option(run.annotations.get(node)).map(id -> _)).toMap
    val statisticsWarnings = estimation.fold(Vector.empty)(e => PhysicalPlanner.statisticsWarnings(logical, e.estimator))
    PlannedQuery(physical, run.decisions.result(), slotNames(logical), estimates, statisticsWarnings ++ run.warnings.result())

  /** One warning per relation whose entity has no statistics or stale ones. */
  def statisticsWarnings(logical: LPlan, estimator: CardinalityEstimator): Vector[String] =
    relations(logical).distinctBy(_.entity.id).flatMap { relation =>
      val name = relation.entity.name
      estimator.status(relation) match
        case StatisticsStatus.Missing =>
          Some(f"no statistics for $name: estimates assume ${estimator.config.defaultRows}%.0f rows; run ANALYZE $name")
        case StatisticsStatus.Stale(s) =>
          Some(f"statistics of $name are stale (${s.modificationsSinceAnalyze} modifications since ANALYZE, ${s.changeRatio * 100}%.0f%% of its rows); run ANALYZE $name")
        case StatisticsStatus.Fresh(_) => None
    }

  /** Every relation read by `plan`, in plan order. */
  def relations(plan: LPlan): Vector[BoundRelation] = plan match
    case LPlan.TableScan(relation) => Vector(relation)
    case LPlan.PointLookup(relation, _) => Vector(relation)
    case other => other.children.flatMap(relations)

  /** Composes the 128-bit storage RowId from a 64-bit entity id and an unsigned 64-bit primary key. */
  def composeRowId(entityId: EntityId, primaryKey: Long): BigInt =
    (BigInt(entityId.value) << 64) | BigInt(java.lang.Long.toUnsignedString(primaryKey))

  /** One planning run, collecting decisions, warnings and annotations. */
  private final class Run(policy: PlanningPolicy, estimation: Option[PlanEstimation]):
    /** Decisions in planning order. */
    val decisions = Vector.newBuilder[PlanDecision]
    /** Warnings raised by the policy. */
    val warnings = Vector.newBuilder[String]
    /** Estimate of every physical node, by node identity. */
    val annotations = new java.util.IdentityHashMap[PPlan, NodeEstimate]()

    /** Translates one logical operator (and its inputs) and annotates the result. */
    def translate(logical: LPlan): PPlan = annotate(translateNode(logical), logical)

    /** Records the estimate of `logical` for `physical` unless it already has one (when
      * estimating); returns `physical`.
      */
    private def annotate(physical: PPlan, logical: LPlan, cost: Option[Double] = None): PPlan =
      estimation.foreach { e =>
        if !annotations.containsKey(physical) then
          val estimate = e.estimator.estimate(logical)
          val total = cost.getOrElse(e.costModel.total(e.costModel.cost(logical)))
          annotations.put(physical, NodeEstimate(estimate.rows, total, estimate.confidence, estimate.source.label))
      }
      physical

    /** Translates one logical operator without annotating it. */
    private def translateNode(logical: LPlan): PPlan = logical match
      case LPlan.TableScan(relation) => PPlan.EntityScan(relation.entity.id, columns(relation))
      case LPlan.PointLookup(relation, key) => PPlan.PointLookup(composeRowId(relation.entity.id, key), columns(relation))
      case LPlan.Filter(input, predicate) => PPlan.Filter(translate(input), expr(predicate))
      case join: LPlan.Join => translateJoin(join)
      case LPlan.Aggregate(input, groupBy, aggregates) =>
        PPlan.Aggregate(
          translate(input),
          groupBy.map(_.slot),
          aggregates.map(a => AggregateSpec(a.function, a.input.map(_.slot), a.output.slot))
        )
      case node @ LPlan.Limit(LPlan.Sort(input, keys), limit) => orderedLimit(node, input, keys, limit, None)
      case node @ LPlan.Limit(LPlan.Project(LPlan.Sort(input, keys), attributes), limit) =>
        orderedLimit(node, input, keys, limit, Some(attributes))
      case LPlan.Sort(input, keys) => PPlan.Sort(translate(input), sortKeys(keys))
      case LPlan.Project(input, attributes) => PPlan.Project(translate(input), projectedSlots(attributes))
      case LPlan.Limit(input, limit) => PPlan.Limit(translate(input), limit)

    /** `LIMIT` over `ORDER BY`: TopK or Sort + Limit as the policy decides; a projection between
      * them is kept above the result. `node` is the logical Limit being translated; the TopK
      * (or Limit) below the projection carries its estimate.
      */
    private def orderedLimit(node: LPlan, input: LPlan, keys: Vector[BoundOrder], limit: Int, project: Option[Vector[Attribute]]): PPlan =
      val translated = translate(input)
      val (topK, reason) = policy.chooseTopK(limit, keys, input)
      decisions += PlanDecision(
        s"ORDER BY ${keys.map(_.display).mkString(", ")} LIMIT $limit",
        if topK then "top_k" else "sort + limit",
        reason
      )
      val ordered =
        if topK then
          val topKCost = estimation.map(e => e.costModel.total(e.costModel.cost(input) + e.costModel.topKCost(input, limit)))
          annotate(PPlan.TopK(translated, sortKeys(keys), limit), node, topKCost)
        else
          val sort = LPlan.Sort(input, keys)
          annotate(PPlan.Limit(annotate(PPlan.Sort(translated, sortKeys(keys)), sort), limit), node)
      project.fold(ordered) { attributes =>
        val projected = PPlan.Project(ordered, projectedSlots(attributes))
        Option(annotations.get(ordered)).foreach(annotations.put(projected, _))
        projected
      }

    /** Splits the join condition into equality keys and a residual and lets the policy choose. */
    private def translateJoin(join: LPlan.Join): PPlan =
      val leftSlots = join.left.output.map(_.slot).toSet
      val rightSlots = join.right.output.map(_.slot).toSet
      val conjuncts = join.condition.toVector.flatMap(TypedExpr.conjuncts)
      val keyed = conjuncts.map(part => part -> equiKey(part, leftSlots, rightSlots))
      val equiKeys = keyed.flatMap(_._2)
      val residual = TypedExpr.conjunction(keyed.collect { case (part, None) => part })
      // Inputs first, so decisions are listed bottom-up (innermost join first).
      val left = translate(join.left)
      val right = translate(join.right)
      val choice = policy.chooseJoin(JoinRequest(join, equiKeys, residual))
      val useHash = choice.strategy == JoinStrategy.Hash && equiKeys.nonEmpty
      // The engine materializes its right input; LEFT JOIN semantics forbid exchanging them.
      val swap = choice.swapInputs && join.joinType != JoinType.Left
      decisions += PlanDecision(
        s"${join.joinType.toString.toUpperCase} JOIN ${aliases(join.left)} with ${aliases(join.right)}",
        if useHash then "hash_join" else "nested_loop_join",
        choice.reason
      )
      warnings ++= choice.warnings
      val joinType = if join.joinType == JoinType.Left then PhysicalJoinType.Left else PhysicalJoinType.Inner
      val (probe, build) = if swap then (right, left) else (left, right)
      if useHash then
        val keys = equiKeys.map((l, r) => if swap then JoinKey(r.slot, l.slot) else JoinKey(l.slot, r.slot))
        PPlan.HashJoin(probe, build, joinType, keys, residual.map(expr))
      else PPlan.NestedLoopJoin(probe, build, joinType, join.condition.map(expr))

    /** `left = right` with one attribute from each input and matching types, oriented left-to-right. */
    private def equiKey(part: TypedExpr, leftSlots: Set[SlotId], rightSlots: Set[SlotId]): Option[(Attribute, Attribute)] =
      part match
        case TypedExpr.Binary(TypedExpr.Column(a), BinaryOp.Eq, TypedExpr.Column(b), _) if a.dataType == b.dataType =>
          if leftSlots(a.slot) && rightSlots(b.slot) then Some(a -> b)
          else if leftSlots(b.slot) && rightSlots(a.slot) then Some(b -> a)
          else None
        case _ => None

  /** Field-to-slot mapping of a relation's referenced columns. */
  private def columns(relation: BoundRelation): Vector[ScanColumn] =
    relation.columns.map { attribute =>
      attribute.origin match
        case ColumnOrigin.Stored(_, _, field) => ScanColumn(field, attribute.slot)
        case ColumnOrigin.Computed(_) => throw new IllegalStateException(s"scan of computed attribute ${attribute.display}")
    }

  /** Slots of a projection, each once: `SELECT id, id` or a repeated aggregate shows one slot in
    * several output columns, and the gateway maps output columns to batch columns by slot.
    */
  private def projectedSlots(attributes: Vector[Attribute]): Vector[SlotId] = attributes.map(_.slot).distinct

  /** Sort keys of ORDER BY items. */
  private def sortKeys(keys: Vector[BoundOrder]): Vector[SortKey] = keys.map(k => SortKey(k.attribute.slot, k.descending))

  /** Short description of a join input for decisions: the aliases of the relations it reads. */
  private def aliases(plan: LPlan): String = plan match
    case LPlan.TableScan(relation) => relation.alias
    case LPlan.PointLookup(relation, _) => relation.alias
    case other => other.children.map(aliases).mkString(",")

  /** Display name of every attribute the plan mentions. */
  private def slotNames(plan: LPlan): Map[SlotId, String] =
    val own = plan match
      case LPlan.Aggregate(_, _, aggregates) => aggregates.map(a => a.output.slot -> a.output.name)
      case _ => Vector.empty
    (plan.output.map(a => a.slot -> a.name) ++ own).toMap ++ plan.children.flatMap(slotNames)

  /** Converts one typed logical expression into its physical execution representation. */
  private def expr(e: TypedExpr): PhysicalExpr = e match
    case TypedExpr.Column(attribute) => PhysicalExpr.Slot(attribute.slot)
    case TypedExpr.Literal(value) => PhysicalExpr.Literal(value)
    case TypedExpr.Not(inner) => PhysicalExpr.Not(expr(inner))
    case TypedExpr.Binary(left, op, right, _) =>
      val mapped = op match
        case BinaryOp.Eq => PhysicalBinaryOp.Eq
        case BinaryOp.Ne => PhysicalBinaryOp.Ne
        case BinaryOp.Lt => PhysicalBinaryOp.Lt
        case BinaryOp.Le => PhysicalBinaryOp.Le
        case BinaryOp.Gt => PhysicalBinaryOp.Gt
        case BinaryOp.Ge => PhysicalBinaryOp.Ge
        case BinaryOp.And => PhysicalBinaryOp.And
        case BinaryOp.Or => PhysicalBinaryOp.Or
      PhysicalExpr.Binary(expr(left), mapped, expr(right))
