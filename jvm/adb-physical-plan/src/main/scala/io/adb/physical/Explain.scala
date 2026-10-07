package io.adb.physical

import io.adb.model.*
import io.adb.physical.PhysicalPlan.*

/** Human-readable EXPLAIN rendering of physical plans: one operator per line, indented by depth,
  * with slots shown as `name#slot`.
  */
object Explain:
  /** Renders `plan`, naming slots with `names` (unknown slots render as `#n`). */
  def physical(plan: PhysicalPlan, names: Map[SlotId, String]): String =
    render(plan, names, 0)

  /** Renders one node and its inputs. */
  private def render(plan: PhysicalPlan, names: Map[SlotId, String], depth: Int): String =
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
    (("  " * depth + line) +: plan.children.map(render(_, names, depth + 1))).mkString("\n")
