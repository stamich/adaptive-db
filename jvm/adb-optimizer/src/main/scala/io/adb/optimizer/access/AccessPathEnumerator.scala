package io.adb.optimizer.access

import io.adb.catalog.IndexInfo
import io.adb.logical.*

/** Enumerates the ways one relation can be read under the conjuncts of its filter.
  *
  * Candidates are the sequential scan and, for every index of the relation's entity that serves
  * the query snapshot, an index scan when the index restricts the scan (an equality prefix or a
  * range) or covers every column the query reads (a narrow full scan). A backward scan is a
  * candidate as well when `backward` asks for one (the planner does when an ORDER BY wants the
  * reverse of an index order). Costs are the caller's business (see
  * [[io.adb.optimizer.cost.CostModel.accessCost]]).
  */
object AccessPathEnumerator:
  /** The access paths of `relation` under `conjuncts`.
    *
    * @param indexes  indexes known to the planner (any entity; others are skipped)
    * @param snapshot the query's `AS OF` snapshot, or `None` for the latest one (which every
    *                 listed index serves: indexes are maintained inside the commit)
    * @param ordered  also yield full scans of indexes that are neither restricting nor
    *                 covering, for their order
    * @param backward also yield backward variants of every index scan
    */
  def candidates(
      relation: BoundRelation,
      conjuncts: Vector[TypedExpr],
      indexes: Vector[IndexInfo],
      snapshot: Option[Long],
      ordered: Boolean = false,
      backward: Boolean = false
  ): Vector[AccessPath] =
    val usable = indexes.filter(index => index.entity == relation.entity.id && usableAt(index, snapshot))
    val scans = usable.flatMap { index =>
      val matched = IndexPredicateMatcher.matchIndex(relation, index, conjuncts)
      val covering = AccessPath.covers(relation, index)
      if matched.isRestricting || covering || ordered then
        val forward = AccessPath.IndexScan(relation, matched, backward = false, covering = covering)
        if backward then Vector(forward, forward.copy(backward = true)) else Vector(forward)
      else Vector.empty
    }
    AccessPath.SeqScan(relation, conjuncts) +: scans

  /** Whether `index` can answer a query at `snapshot` (`None` = the latest snapshot). */
  def usableAt(index: IndexInfo, snapshot: Option[Long]): Boolean = snapshot.forall(s => index.usableFromTs <= s)
