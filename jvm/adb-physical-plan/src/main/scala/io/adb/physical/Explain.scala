package io.adb.physical

import io.adb.model.*
import io.adb.physical.PhysicalPlan.*

/** Human-readable EXPLAIN rendering of physical plans: one operator per line, indented by depth,
  * with slots shown as `name#slot` and, when estimates are given, each node's pre-order id and
  * estimate: `[2] HashJoin ... (est. rows=950 cost=4321.0 conf=0.95)`.
  */
object Explain:
  /** Renders `plan`, naming slots with `names` (unknown slots render as `#n`) and annotating
    * nodes with `estimates` by pre-order id.
    */
  def physical(plan: PhysicalPlan, names: Map[SlotId, String], estimates: Map[Int, NodeEstimate] = Map.empty): String =
    val ids = new java.util.IdentityHashMap[PhysicalPlan, Integer]()
    PhysicalPlan.preorder(plan).zipWithIndex.foreach((node, id) => ids.put(node, id))
    render(plan, names, 0, node => if estimates.isEmpty then ("", "") else annotation(ids.get(node), estimates))

  /** `[id] ` prefix and ` (est. rows=… cost=… conf=…)` suffix of one node. */
  private def annotation(id: Int, estimates: Map[Int, NodeEstimate]): (String, String) =
    (s"[$id] ", estimates.get(id).fold("")(e => f" (est. rows=${e.rows}%.0f cost=${e.cost}%.1f conf=${e.confidence}%.2f)"))

  /** Renders one node and its inputs; `note` gives the prefix and suffix of a node. */
  private def render(plan: PhysicalPlan, names: Map[SlotId, String], depth: Int, note: PhysicalPlan => (String, String)): String =
    def slot(s: SlotId): String = s"${names.getOrElse(s, "")}#${s.value}"
    def columns(cs: Vector[ScanColumn]): String =
      cs.map(c => s"f${c.fieldId.value}->${slot(c.slot)}").mkString("[", ", ", "]")
    def keys(ks: Vector[SortKey]): String =
      ks.map(k => slot(k.slot) + (if k.descending then " DESC" else "")).mkString("[", ", ", "]")
    def expr(e: PhysicalExpr): String = e match
      case PhysicalExpr.Slot(s) => slot(s)
      case PhysicalExpr.Literal(value) => PlanJsonEncoder.encodeRustValue(value)
      case PhysicalExpr.Binary(l, op, r) => s"(${expr(l)} ${op.toString.toLowerCase} ${expr(r)})"
      case PhysicalExpr.Not(inner) => s"NOT ${expr(inner)}"
    val line = plan match
      case PointLookup(rowId, cs) => s"PointLookup row_id=$rowId ${columns(cs)}"
      case Scan(cs) => s"Scan ${columns(cs)}"
      case EntityScan(entity, cs) => s"EntityScan entity=${entity.value} ${columns(cs)}"
      case Filter(_, predicate) => s"Filter ${expr(predicate)}"
      case Project(_, slots) => s"Project ${slots.map(slot).mkString("[", ", ", "]")}"
      case Limit(_, limit) => s"Limit $limit"
      case HashJoin(_, _, joinType, ks, residual) =>
        val keyText = ks.map(k => s"${slot(k.left)} = ${slot(k.right)}").mkString(", ")
        s"HashJoin ${joinType.toString.toUpperCase} keys=[$keyText]${residual.fold("")(r => s" residual=${expr(r)}")}"
      case NestedLoopJoin(_, _, joinType, predicate) =>
        s"NestedLoopJoin ${joinType.toString.toUpperCase}${predicate.fold(" (cross)")(p => s" on=${expr(p)}")}"
      case Aggregate(_, groupBy, aggregates) =>
        val aggs = aggregates.map(a => s"${a.function.sqlName}(${a.input.fold("*")(slot)}) -> ${slot(a.output)}")
        s"Aggregate group=${groupBy.map(slot).mkString("[", ", ", "]")} aggregates=${aggs.mkString("[", ", ", "]")}"
      case Sort(_, ks) => s"Sort ${keys(ks)}"
      case TopK(_, ks, limit) => s"TopK ${keys(ks)} limit=$limit"
    val (prefix, suffix) = note(plan)
    val annotated = prefix + line + suffix
    (("  " * depth + annotated) +: plan.children.map(render(_, names, depth + 1, note))).mkString("\n")
