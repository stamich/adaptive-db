package io.adb.physical

import io.adb.model.*
import io.adb.physical.PhysicalPlan.*

/** What EXPLAIN shows of an index instead of its id: its name and the names of its key columns,
  * most significant first (used to print bounds as `balance >= 9000`).
  *
  * @param name       index name
  * @param keyColumns key column names (`DESC` columns without a suffix)
  */
final case class IndexLabel(name: String, keyColumns: Vector[String]) derives CanEqual

/** Human-readable EXPLAIN rendering of physical plans: one operator per line, indented by depth,
  * with slots shown as `name#slot` and, when estimates are given, each node's pre-order id and
  * estimate: `[2] HashJoin ... (est. rows=950 cost=4321.0 conf=0.95)`.
  *
  * Index scans name their index and bounds by column when an [[IndexLabel]] is given
  * (`IndexScan idx_balance [region = 2, balance >= 9000] covering ...`) and fall back to the
  * id and positional bounds otherwise; a scan without a `RowFetch` above it is marked
  * `covering` (it answers from index entries alone).
  */
object Explain:
  /** Renders `plan`, naming slots with `names` (unknown slots render as `#n`), annotating
    * nodes with `estimates` by pre-order id and naming indexes with `indexes`.
    */
  def physical(
      plan: PhysicalPlan,
      names: Map[SlotId, String],
      estimates: Map[Int, NodeEstimate] = Map.empty,
      indexes: Map[IndexId, IndexLabel] = Map.empty
  ): String =
    val ids = new java.util.IdentityHashMap[PhysicalPlan, Integer]()
    PhysicalPlan.preorder(plan).zipWithIndex.foreach((node, id) => ids.put(node, id))
    val fetched = java.util.Collections.newSetFromMap(new java.util.IdentityHashMap[PhysicalPlan, java.lang.Boolean]())
    PhysicalPlan.preorder(plan).foreach {
      case RowFetch(input, _) => fetched.add(input)
      case _ => ()
    }
    val context = RenderContext(names, indexes, node => fetched.contains(node))
    render(plan, context, 0, node => if estimates.isEmpty then ("", "") else annotation(ids.get(node), estimates))

  /** What rendering needs besides the plan.
    *
    * @param names   slot names
    * @param indexes index labels by id
    * @param fetched whether a node is the input of a `RowFetch`
    */
  private final case class RenderContext(names: Map[SlotId, String], indexes: Map[IndexId, IndexLabel], fetched: PhysicalPlan => Boolean)

  /** `[id] ` prefix and ` (est. rows=… cost=… conf=…)` suffix of one node. */
  private def annotation(id: Int, estimates: Map[Int, NodeEstimate]): (String, String) =
    val suffix = estimates.get(id).fold("") { e =>
      val basis = if e.basis.isEmpty then "" else e.basis.mkString(" basis=", ",", "")
      f" (est. rows=${e.rows}%.0f cost=${e.cost}%.1f conf=${e.confidence}%.2f$basis)"
    }
    (s"[$id] ", suffix)

  /** Renders one node and its inputs; `note` gives the prefix and suffix of a node. */
  private def render(plan: PhysicalPlan, context: RenderContext, depth: Int, note: PhysicalPlan => (String, String)): String =
    val names = context.names
    /** `name#slot`. */
    def slot(s: SlotId): String = s"${names.getOrElse(s, "")}#${s.value}"
    /** `[f1->name#slot, ...]`. */
    def columns(cs: Vector[ScanColumn]): String =
      cs.map(c => s"f${c.fieldId.value}->${slot(c.slot)}").mkString("[", ", ", "]")
    /** `[name#slot DESC, ...]`. */
    def keys(ks: Vector[SortKey]): String =
      ks.map(k => slot(k.slot) + (if k.descending then " DESC" else "")).mkString("[", ", ", "]")
    /** Infix rendering of an expression. */
    def expr(e: PhysicalExpr): String = e match
      case PhysicalExpr.Slot(s) => slot(s)
      case PhysicalExpr.Literal(value) => PlanJsonEncoder.encodeRustValue(value)
      case PhysicalExpr.Binary(l, op, r) => s"(${expr(l)} ${op.toString.toLowerCase} ${expr(r)})"
      case PhysicalExpr.Not(inner) => s"NOT ${expr(inner)}"
    val line = plan match
      case PointLookup(rowId, cs) => s"PointLookup row_id=$rowId ${columns(cs)}"
      case Scan(cs) => s"Scan ${columns(cs)}"
      case EntityScan(entity, cs) => s"EntityScan entity=${entity.value} ${columns(cs)}"
      case scan @ IndexScan(index, bounds, direction, cs, limit) =>
        val dir = if direction == ScanDirection.Backward then " backward" else ""
        val covering = if context.fetched(scan) then "" else " covering"
        val target = context.indexes.get(index) match
          case Some(label) => s"${label.name} ${renderNamedBounds(bounds, label.keyColumns)}"
          case None => s"index=${index.value} ${renderBounds(bounds)}"
        s"IndexScan $target$dir${limit.fold("")(l => s" limit=$l")}$covering ${columns(cs)}"
      case RowFetch(_, cs) => s"RowFetch ${columns(cs)}"
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
    (("  " * depth + annotated) +: plan.children.map(render(_, context, depth + 1, note))).mkString("\n")

  /** `prefix=[v, ...] range=[lo, hi)` (only the parts that are present). */
  def renderBounds(bounds: IndexBounds): String =
    val prefix = if bounds.prefix.isEmpty then Vector.empty else Vector(bounds.prefix.map(PlanJsonEncoder.encodeRustValue).mkString("prefix=[", ", ", "]"))
    val range =
      if bounds.lower.isEmpty && bounds.upper.isEmpty then Vector.empty
      else
        val lower = bounds.lower.fold("(-inf")(b => (if b.inclusive then "[" else "(") + PlanJsonEncoder.encodeRustValue(b.value))
        val upper = bounds.upper.fold("+inf)")(b => PlanJsonEncoder.encodeRustValue(b.value) + (if b.inclusive then "]" else ")"))
        Vector(s"range=$lower, $upper")
    val parts = prefix ++ range
    if parts.isEmpty then "full" else parts.mkString(" ")

  /** `v` as a SQL literal: `NULL`, `TRUE`, `42`, `1.5`, `'it''s'`, `X'0aff'`. */
  def sqlLiteral(v: DbValue): String = v match
    case DbValue.NullValue => "NULL"
    case DbValue.BoolValue(b) => if b then "TRUE" else "FALSE"
    case DbValue.Int64Value(n) => n.toString
    case DbValue.Float64Value(d) => d.toString
    case DbValue.StringValue(text) => "'" + text.replace("'", "''") + "'"
    case DbValue.BytesValue(bytes) => bytes.map(b => f"${b & 0xff}%02x").mkString("X'", "", "'")

  /** Bounds by column: `[region = 2, balance >= 9000]`, `[balance in [10, 20)]` or `full`.
    * The prefix values bind the first key columns; the range applies to the next one. Key
    * columns the label does not name render as `key<n>`.
    */
  def renderNamedBounds(bounds: IndexBounds, keyColumns: Vector[String]): String =
    /** The name of key column `n`. */
    def column(n: Int): String = keyColumns.lift(n).getOrElse(s"key$n")
    /** A value as a SQL literal. */
    def value(v: DbValue): String = sqlLiteral(v)
    val equalities = bounds.prefix.zipWithIndex.map((v, n) => s"${column(n)} = ${value(v)}")
    val rangeColumn = column(bounds.prefix.size)
    val range = (bounds.lower, bounds.upper) match
      case (None, None) => None
      case (Some(lo), None) => Some(s"$rangeColumn ${if lo.inclusive then ">=" else ">"} ${value(lo.value)}")
      case (None, Some(hi)) => Some(s"$rangeColumn ${if hi.inclusive then "<=" else "<"} ${value(hi.value)}")
      case (Some(lo), Some(hi)) =>
        Some(s"$rangeColumn in ${if lo.inclusive then "[" else "("}${value(lo.value)}, ${value(hi.value)}${if hi.inclusive then "]" else ")"}")
    val parts = equalities ++ range
    if parts.isEmpty then "full" else parts.mkString("[", ", ", "]")
