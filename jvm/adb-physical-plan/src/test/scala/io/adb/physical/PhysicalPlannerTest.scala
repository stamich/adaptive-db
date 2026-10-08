package io.adb.physical

import io.adb.logical.*
import io.adb.physical.PhysicalPlan.*
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Physical planning: strategy choices, their reasons, and the policy seam. */
final class PhysicalPlannerTest:
  /** Finds the first node matching `pf` in pre-order. */
  private def find[A](plan: PhysicalPlan)(pf: PartialFunction[PhysicalPlan, A]): Option[A] =
    pf.lift(plan).orElse(plan.children.iterator.flatMap(find(_)(pf)).nextOption())

  /** An equality condition becomes a hash join with keys oriented left-to-right; other
    * conjuncts stay as the residual; the decision explains the choice.
    */
  @Test def equalityJoinUsesHashJoin(): Unit =
    val q = PlanningFixtures.planned("SELECT c.name FROM customer c JOIN orders o ON c.id = o.customer_id AND o.amount > c.id")
    val join = find(q.plan) { case j: HashJoin => j }.getOrElse(fail("no hash join:\n" + Explain.physical(q.plan, q.slotNames)))
    val leftSlots = find(join.left) { case EntityScan(_, columns) => columns.map(_.slot).toSet }.get
    assertTrue(join.keys.forall(k => leftSlots.contains(k.left)), "keys are oriented left = right")
    assertTrue(join.residual.isDefined)
    assertEquals("hash_join", q.decisions.head.choice)
    assertTrue(q.decisions.head.reason.contains("c.id = o.customer_id"))

  /** Without an equality the planner falls back to a nested-loop join; CROSS JOIN has no predicate. */
  @Test def nonEqualityJoinsUseNestedLoop(): Unit =
    val q = PlanningFixtures.planned("SELECT c.name FROM customer c JOIN orders o ON o.amount > c.id")
    assertTrue(find(q.plan) { case NestedLoopJoin(_, _, PhysicalJoinType.Inner, Some(_)) => true }.isDefined)
    assertEquals("nested_loop_join", q.decisions.head.choice)
    val cross = PlanningFixtures.planned("SELECT c.name FROM customer c CROSS JOIN orders o")
    assertTrue(find(cross.plan) { case NestedLoopJoin(_, _, _, None) => true }.isDefined)

  /** LIMIT over ORDER BY (with the projection in between) becomes a TopK under the projection;
    * ORDER BY alone stays a full sort.
    */
  @Test def orderedLimitBecomesTopK(): Unit =
    val q = PlanningFixtures.planned("SELECT name FROM customer ORDER BY city DESC LIMIT 5")
    q.plan match
      case Project(TopK(_, Vector(SortKey(_, true)), 5), _) => ()
      case other => fail("unexpected plan:\n" + Explain.physical(other, q.slotNames))
    assertEquals("top_k", q.decisions.head.choice)
    val sorted = PlanningFixtures.planned("SELECT name FROM customer ORDER BY city")
    assertTrue(find(sorted.plan) { case s: Sort => s }.isDefined)

  /** Scans read only referenced fields; aggregates carry their slots; slot names are known. */
  @Test def aggregatesAndPrunedScans(): Unit =
    val q = PlanningFixtures.planned("SELECT city, COUNT(*), MAX(name) FROM customer GROUP BY city")
    val agg = find(q.plan) { case a: Aggregate => a }.get
    assertEquals(1, agg.groupBy.size)
    assertEquals(Vector(io.adb.model.AggregateFunction.Count, io.adb.model.AggregateFunction.Max), agg.aggregates.map(_.function))
    assertEquals(None, agg.aggregates.head.input)
    val scan = find(q.plan) { case s: EntityScan => s }.get
    assertEquals(2, scan.columns.size, "id is never read")
    assertTrue(q.slotNames.values.exists(_ == "COUNT(*)"))

  /** Selecting the same column or aggregate twice projects its slot once (the engine rejects
    * duplicate projected slots; the gateway shows the value in both output columns).
    */
  @Test def repeatedOutputColumnsProjectTheirSlotOnce(): Unit =
    for sql <- Vector("SELECT id, id FROM customer", "SELECT COUNT(*), COUNT(*) AS c2 FROM customer", "SELECT name AS a, name AS b FROM customer ORDER BY city LIMIT 3") do
      val q = PlanningFixtures.planned(sql)
      val slots = find(q.plan) { case Project(_, slots) => slots }.get
      assertEquals(slots.distinct, slots, sql)

  /** A custom policy replaces the strategy choice without touching the planner (the seam for
    * statistics- or intent-driven planning).
    */
  @Test def policyIsPluggable(): Unit =
    val nestedLoopOnly = new PlanningPolicy:
      def chooseJoin(request: JoinRequest) = JoinChoice(JoinStrategy.NestedLoop, false, "test policy")
      def chooseTopK(limit: Int, keys: Vector[BoundOrder], input: LogicalPlan) = (false, "test policy")
    val q = PlanningFixtures.planned(
      "SELECT c.name FROM customer c JOIN orders o ON c.id = o.customer_id ORDER BY c.name LIMIT 2",
      nestedLoopOnly
    )
    assertTrue(find(q.plan) { case j: NestedLoopJoin => j }.isDefined)
    assertTrue(find(q.plan) { case Limit(Sort(_, _), 2) => true }.isDefined)
    assertTrue(q.decisions.forall(_.reason == "test policy"))
