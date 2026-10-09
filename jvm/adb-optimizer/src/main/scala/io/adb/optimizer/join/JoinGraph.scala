package io.adb.optimizer.join

import io.adb.logical.*
import io.adb.logical.LogicalPlan.*

/** One condition of an inner-join block and the relations it reads.
  *
  * @param expr      the conjunct
  * @param relations bit `i` set when the conjunct reads an attribute of relation `i`; 0 for a
  *                  conjunct that reads no relation (a constant)
  */
final case class JoinPredicate(expr: TypedExpr, relations: Long) derives CanEqual

/** A block of INNER and CROSS joins flattened into its relations and conditions, so the joins
  * can be performed in any order.
  *
  * A relation is any input that is not itself an INNER or CROSS join: a (filtered) scan, a
  * point lookup, a LEFT JOIN, an aggregate... LEFT JOINs are boundaries because moving a
  * relation across one changes which rows are null-filled.
  *
  * @param relations  inputs of the block in SQL order (at most 64)
  * @param predicates every conjunct of every join condition of the block
  */
final case class JoinGraph(relations: Vector[LogicalPlan], predicates: Vector[JoinPredicate]):
  require(relations.size >= 2 && relations.size <= 64, s"a join block has 2..64 relations, got ${relations.size}")

  /** Bit mask of every relation. */
  val all: Long = if relations.size == 64 then -1L else (1L << relations.size) - 1

  /** Whether some predicate connects `a` with `b` (disjoint masks) using only relations of `a | b`. */
  def connected(a: Long, b: Long): Boolean =
    predicates.exists(p => (p.relations & a) != 0 && (p.relations & b) != 0 && (p.relations & ~(a | b)) == 0)

  /** Predicates that become evaluable when `a` and `b` are joined: they read both sides and
    * nothing outside them.
    */
  def joining(a: Long, b: Long): Vector[TypedExpr] =
    predicates.collect { case p if (p.relations & a) != 0 && (p.relations & b) != 0 && (p.relations & ~(a | b)) == 0 => p.expr }

  /** Whether the relations of `mask` form one connected subgraph (predicates that read only
    * relations of `mask` link them).
    */
  def isConnected(mask: Long): Boolean =
    if mask == 0 then false
    else
      var reached = java.lang.Long.lowestOneBit(mask)
      var grown = true
      while grown do
        grown = false
        for p <- predicates if (p.relations & ~mask) == 0 && (p.relations & reached) != 0 && (p.relations & ~reached) != 0 do
          reached |= p.relations
          grown = true
      reached == mask

  /** The connected components of the whole graph, as relation masks, lowest relation first. */
  def components: Vector[Long] =
    var remaining = all
    val out = Vector.newBuilder[Long]
    while remaining != 0 do
      var component = java.lang.Long.lowestOneBit(remaining)
      var grown = true
      while grown do
        grown = false
        for p <- predicates if p.relations != 0 && (p.relations & component) != 0 && (p.relations & ~component) != 0 do
          component |= p.relations
          grown = true
      out += component
      remaining &= ~component
    out.result()

  /** Predicates that read no relation. */
  def constant: Vector[TypedExpr] = predicates.collect { case p if p.relations == 0 => p.expr }

/** Construction of join graphs. */
object JoinGraph:
  /** Whether `plan` is a join that can be reordered with its neighbours. */
  def isReorderable(plan: LogicalPlan): Boolean = plan match
    case Join(_, _, JoinType.Inner | JoinType.Cross, _) => true
    case _ => false

  /** Flattens the INNER/CROSS join block rooted at `root`; `relation` rewrites each relation
    * (to optimize the blocks nested inside it).
    */
  def of(root: Join, relation: LogicalPlan => LogicalPlan): JoinGraph =
    val relations = Vector.newBuilder[LogicalPlan]
    val conjuncts = Vector.newBuilder[TypedExpr]
    /** Collects the relations and conjuncts below `plan`. */
    def walk(plan: LogicalPlan): Unit = plan match
      case join: Join if isReorderable(join) =>
        walk(join.left)
        walk(join.right)
        conjuncts ++= join.condition.toVector.flatMap(TypedExpr.conjuncts)
      case other => relations += relation(other)
    walk(root)
    val inputs = relations.result()
    val slotsOf = inputs.map(_.output.map(_.slot).toSet)
    val predicates = conjuncts.result().map { expr =>
      val slots = expr.slots
      val mask = slotsOf.indices.foldLeft(0L)((mask, i) => if slots.exists(slotsOf(i)) then mask | (1L << i) else mask)
      JoinPredicate(expr, mask)
    }
    // A conjunct that reads one relation only (normally pushed down already) stays with it as a
    // filter, so no join order can lose it.
    val (single, multi) = predicates.partition(p => java.lang.Long.bitCount(p.relations) == 1)
    val filteredInputs = inputs.indices.toVector.map { i =>
      filtered(inputs(i), single.collect { case p if p.relations == (1L << i) => p.expr })
    }
    JoinGraph(filteredInputs, multi)
