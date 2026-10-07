package io.adb.physical

import io.adb.logical.*
import io.adb.logical.LogicalPlan as LPlan
import io.adb.model.*
import io.adb.physical.PhysicalPlan as PPlan

/** The result of physical planning.
  *
  * @param plan      executable plan
  * @param decisions strategy decisions with reasons (shown by EXPLAIN)
  * @param slotNames display name of every slot (EXPLAIN renders `name#slot`)
  */
final case class PlannedQuery(plan: PhysicalPlan, decisions: Vector[PlanDecision], slotNames: Map[SlotId, String])

/** Converts optimized logical plans into the physical plan vocabulary of the native engine.
  *
  *  - a table scan is a native `EntityScan` over the entity's key range, reading only the
  *    referenced fields into their slots;
  *  - a join becomes a `HashJoin` when its condition contains `left = right` equalities
  *    (the rest stays as residual) and a `NestedLoopJoin` otherwise, as chosen by the
  *    [[PlanningPolicy]];
  *  - `LIMIT` over `ORDER BY` (optionally with a projection in between) becomes a `TopK`.
  *
  * The aliases `LPlan` and `PPlan` keep logical and physical operators in separate namespaces.
  */
object PhysicalPlanner:
  /** Builds an executable physical plan from a bound and optimized logical plan. */
  def plan(logical: LPlan, policy: PlanningPolicy = DefaultPlanningPolicy): PlannedQuery =
    val run = Run(policy)
    val physical = run.translate(logical)
    PlannedQuery(physical, run.decisions.result(), slotNames(logical))

  /** Composes the 128-bit storage RowId from a 64-bit entity id and an unsigned 64-bit primary key. */
  def composeRowId(entityId: EntityId, primaryKey: Long): BigInt =
    (BigInt(entityId.value) << 64) | BigInt(java.lang.Long.toUnsignedString(primaryKey))

  /** One planning run, collecting decisions. */
  private final class Run(policy: PlanningPolicy):
    /** Decisions in planning order. */
    val decisions = Vector.newBuilder[PlanDecision]

    /** Translates one logical operator (and its inputs). */
    def translate(logical: LPlan): PPlan = logical match
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
      case LPlan.Limit(LPlan.Sort(input, keys), limit) => orderedLimit(input, keys, limit, None)
      case LPlan.Limit(LPlan.Project(LPlan.Sort(input, keys), attributes), limit) =>
        orderedLimit(input, keys, limit, Some(attributes))
      case LPlan.Sort(input, keys) => PPlan.Sort(translate(input), sortKeys(keys))
      case LPlan.Project(input, attributes) => PPlan.Project(translate(input), projectedSlots(attributes))
      case LPlan.Limit(input, limit) => PPlan.Limit(translate(input), limit)

    /** `LIMIT` over `ORDER BY`: TopK or Sort + Limit as the policy decides; a projection between
      * them is kept above the result.
      */
    private def orderedLimit(input: LPlan, keys: Vector[BoundOrder], limit: Int, project: Option[Vector[Attribute]]): PPlan =
      val translated = translate(input)
      val (topK, reason) = policy.chooseTopK(limit, keys)
      decisions += PlanDecision(
        s"ORDER BY ${keys.map(_.display).mkString(", ")} LIMIT $limit",
        if topK then "top_k" else "sort + limit",
        reason
      )
      val ordered =
        if topK then PPlan.TopK(translated, sortKeys(keys), limit)
        else PPlan.Limit(PPlan.Sort(translated, sortKeys(keys)), limit)
      project.fold(ordered)(attributes => PPlan.Project(ordered, projectedSlots(attributes)))

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
      val (strategy, reason) = policy.chooseJoin(JoinRequest(join.joinType, equiKeys, residual, join.left, join.right))
      val useHash = strategy == JoinStrategy.Hash && equiKeys.nonEmpty
      decisions += PlanDecision(
        s"${join.joinType.toString.toUpperCase} JOIN ${aliases(join.left)} with ${aliases(join.right)}",
        if useHash then "hash_join" else "nested_loop_join",
        reason
      )
      val joinType = if join.joinType == JoinType.Left then PhysicalJoinType.Left else PhysicalJoinType.Inner
      if useHash then PPlan.HashJoin(left, right, joinType, equiKeys.map((l, r) => JoinKey(l.slot, r.slot)), residual.map(expr))
      else PPlan.NestedLoopJoin(left, right, joinType, join.condition.map(expr))

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
