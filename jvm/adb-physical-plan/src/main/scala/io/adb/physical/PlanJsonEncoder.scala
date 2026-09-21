package io.adb.physical

import io.adb.model.*
import io.adb.physical.PhysicalPlan.*
import io.adb.physical.PhysicalExpr.*

/** Documents `PlanJsonEncoder` and its role in the Milestone 2.0.1 JVM control plane. */
object PlanJsonEncoder:
  /** Documents `encode` and its role in the Milestone 2.0.1 JVM control plane. */
  def encode(plan: PhysicalPlan): String = plan match
    case PointLookup(rowId) => s"{\"op\":\"point_lookup\",\"row_id\":${quote(rowId.toString)}}"
    case Scan => "{\"op\":\"scan\"}"
    case Filter(input, predicate) => s"{\"op\":\"filter\",\"input\":${encode(input)},\"predicate\":${encodeExpr(predicate)}}"
    case Project(input, fields) => s"{\"op\":\"project\",\"input\":${encode(input)},\"fields\":[${fields.map(_.value).mkString(",")}] }"
    case Limit(input, limit) => s"{\"op\":\"limit\",\"input\":${encode(input)},\"limit\":$limit}"

  /** Documents `encodeExpr` and its role in the Milestone 2.0.1 JVM control plane. */
  private def encodeExpr(expr: PhysicalExpr): String = expr match
    case Column(fieldId) => s"{\"kind\":\"column\",\"field_id\":${fieldId.value}}"
    case Literal(value) => s"{\"kind\":\"literal\",\"value\":${encodeRustValue(value)}}"
    case Not(inner) => s"{\"kind\":\"not\",\"expr\":${encodeExpr(inner)}}"
    case Binary(left, op, right) =>
      s"{\"kind\":\"binary\",\"left\":${encodeExpr(left)},\"op\":\"${op.toString.toLowerCase}\",\"right\":${encodeExpr(right)}}"

  /** Documents `encodeRustValue` and its role in the Milestone 2.0.1 JVM control plane. */
  def encodeRustValue(value: DbValue): String = value match
    case DbValue.NullValue => "\"Null\""
    case DbValue.BoolValue(v) => s"{\"Bool\":$v}"
    case DbValue.Int64Value(v) => s"{\"Int64\":$v}"
    case DbValue.Float64Value(v) => s"{\"Float64\":$v}"
    case DbValue.StringValue(v) => s"{\"String\":${quote(v)}}"
    case DbValue.BytesValue(v) => s"{\"Bytes\":[${v.map(b => b & 0xff).mkString(",")}] }"

  /** Documents `quote` and its role in the Milestone 2.0.1 JVM control plane. */
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
