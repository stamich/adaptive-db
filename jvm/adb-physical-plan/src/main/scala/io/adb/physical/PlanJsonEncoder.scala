package io.adb.physical

import io.adb.model.*
import io.adb.physical.PhysicalPlan.*
import io.adb.physical.PhysicalExpr.*

/** Encodes physical plans into the JSON wire format decoded by the native engine
  * (plan wire v2, docs/plan-wire-format.md): `{"wire_version":2,"plan":{...}}`.
  */
object PlanJsonEncoder:
  /** Plan wire version produced by this encoder; must match the engine's `PLAN_WIRE_VERSION`. */
  val WireVersion: Int = 2

  /** Encodes a plan inside the versioned envelope. */
  def encode(plan: PhysicalPlan): String =
    s"{\"wire_version\":$WireVersion,\"plan\":${encodeNode(plan)}}"

  /** Encodes one plan node (without envelope). */
  def encodeNode(plan: PhysicalPlan): String = plan match
    case PointLookup(rowId, columns) =>
      s"{\"op\":\"point_lookup\",\"row_id\":${quote(rowId.toString)},\"columns\":${encodeColumns(columns)}}"
    case Scan(columns) => s"{\"op\":\"scan\",\"columns\":${encodeColumns(columns)}}"
    case EntityScan(entityId, columns) =>
      s"{\"op\":\"entity_scan\",\"entity_id\":${java.lang.Long.toUnsignedString(entityId.value)},\"columns\":${encodeColumns(columns)}}"
    case Filter(input, predicate) => s"{\"op\":\"filter\",\"input\":${encodeNode(input)},\"predicate\":${encodeExpr(predicate)}}"
    case Project(input, slots) => s"{\"op\":\"project\",\"input\":${encodeNode(input)},\"slots\":${encodeSlots(slots)}}"
    case Limit(input, limit) => s"{\"op\":\"limit\",\"input\":${encodeNode(input)},\"limit\":$limit}"
    case HashJoin(left, right, joinType, keys, residual) =>
      val encodedKeys = keys.map(k => s"{\"left\":${k.left.value},\"right\":${k.right.value}}").mkString("[", ",", "]")
      s"{\"op\":\"hash_join\",\"left\":${encodeNode(left)},\"right\":${encodeNode(right)}," +
        s"\"join_type\":${encodeJoinType(joinType)},\"keys\":$encodedKeys${optionalExpr("residual", residual)}}"
    case NestedLoopJoin(left, right, joinType, predicate) =>
      s"{\"op\":\"nested_loop_join\",\"left\":${encodeNode(left)},\"right\":${encodeNode(right)}," +
        s"\"join_type\":${encodeJoinType(joinType)}${optionalExpr("predicate", predicate)}}"
    case Aggregate(input, groupBy, aggregates) =>
      val specs = aggregates.map { spec =>
        val input = spec.input.fold("")(slot => s",\"input\":${slot.value}")
        s"{\"function\":\"${spec.function.toString.toLowerCase}\"$input,\"output\":${spec.output.value}}"
      }.mkString("[", ",", "]")
      s"{\"op\":\"aggregate\",\"input\":${encodeNode(input)},\"group_by\":${encodeSlots(groupBy)},\"aggregates\":$specs}"
    case Sort(input, keys) => s"{\"op\":\"sort\",\"input\":${encodeNode(input)},\"keys\":${encodeSortKeys(keys)}}"
    case TopK(input, keys, limit) =>
      s"{\"op\":\"top_k\",\"input\":${encodeNode(input)},\"keys\":${encodeSortKeys(keys)},\"limit\":$limit}"

  /** Encodes an expression tree. */
  private def encodeExpr(expr: PhysicalExpr): String = expr match
    case Slot(slot) => s"{\"kind\":\"slot\",\"slot\":${slot.value}}"
    case Literal(value) => s"{\"kind\":\"literal\",\"value\":${encodeRustValue(value)}}"
    case Not(inner) => s"{\"kind\":\"not\",\"expr\":${encodeExpr(inner)}}"
    case Binary(left, op, right) =>
      s"{\"kind\":\"binary\",\"left\":${encodeExpr(left)},\"op\":\"${op.toString.toLowerCase}\",\"right\":${encodeExpr(right)}}"

  /** `,"name":expr` for a present expression, nothing otherwise. */
  private def optionalExpr(name: String, expr: Option[PhysicalExpr]): String =
    expr.fold("")(e => s",\"$name\":${encodeExpr(e)}")

  /** `[{"field_id":f,"slot":s},...]`. */
  private def encodeColumns(columns: Vector[ScanColumn]): String =
    columns.map(c => s"{\"field_id\":${c.fieldId.value},\"slot\":${c.slot.value}}").mkString("[", ",", "]")

  /** `[s1,s2,...]`. */
  private def encodeSlots(slots: Vector[SlotId]): String = slots.map(_.value).mkString("[", ",", "]")

  /** `[{"slot":s,"descending":b},...]`. */
  private def encodeSortKeys(keys: Vector[SortKey]): String =
    keys.map(k => s"{\"slot\":${k.slot.value},\"descending\":${k.descending}}").mkString("[", ",", "]")

  /** `"inner"` or `"left"`. */
  private def encodeJoinType(joinType: PhysicalJoinType): String = s"\"${joinType.toString.toLowerCase}\""

  /** Encodes a value in the externally tagged serde form of the Rust `Value` enum.
    *
    * @throws IllegalArgumentException for NaN and infinite doubles, which JSON cannot represent
    */
  def encodeRustValue(value: DbValue): String = value match
    case DbValue.NullValue => "\"Null\""
    case DbValue.BoolValue(v) => s"{\"Bool\":$v}"
    case DbValue.Int64Value(v) => s"{\"Int64\":$v}"
    case DbValue.Float64Value(v) =>
      require(!v.isNaN && !v.isInfinite, s"DOUBLE value $v cannot be encoded as JSON")
      s"{\"Float64\":$v}"
    case DbValue.StringValue(v) => s"{\"String\":${quote(v)}}"
    case DbValue.BytesValue(v) => s"{\"Bytes\":[${v.map(b => b & 0xff).mkString(",")}]}"

  /** JSON string literal with control characters escaped. */
  def quote(value: String): String =
    val b = new StringBuilder("\"")
    value.foreach {
      case '\\' => b.append("\\\\")
      case '\"' => b.append("\\\"")
      case '\n' => b.append("\\n")
      case '\r' => b.append("\\r")
      case '\t' => b.append("\\t")
      case c if c < ' ' => b.append(f"\\u${c.toInt}%04x")
      case c => b.append(c)
    }
    b.append('"').result()
