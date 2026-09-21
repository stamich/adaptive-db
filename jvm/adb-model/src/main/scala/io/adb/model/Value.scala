package io.adb.model

/** Documents `DbValue` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait DbValue derives CanEqual
/** Documents `DbValue` and its role in the Milestone 2.0.1 JVM control plane. */
object DbValue:
  /** Documents `NullValue` and its role in the Milestone 2.0.1 JVM control plane. */
  case object NullValue extends DbValue
  /** Documents `BoolValue` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class BoolValue(value: Boolean) extends DbValue
  /** Documents `Int64Value` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Int64Value(value: Long) extends DbValue
  /** Documents `Float64Value` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class Float64Value(value: Double) extends DbValue
  /** Documents `StringValue` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class StringValue(value: String) extends DbValue
  /** Documents `BytesValue` and its role in the Milestone 2.0.1 JVM control plane. */
  final case class BytesValue(value: Array[Byte]) extends DbValue

  /** Documents `dataType` and its role in the Milestone 2.0.1 JVM control plane. */
  def dataType(value: DbValue): Option[DataType] = value match
    case NullValue => None
    case BoolValue(_) => Some(DataType.Bool)
    case Int64Value(_) => Some(DataType.Int64)
    case Float64Value(_) => Some(DataType.Float64)
    case StringValue(_) => Some(DataType.StringType)
    case BytesValue(_) => Some(DataType.Bytes)
