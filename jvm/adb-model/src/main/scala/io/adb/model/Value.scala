package io.adb.model

/** A single SQL value as seen by the JVM control plane. */
sealed trait DbValue derives CanEqual
/** The value variants and their type mapping. */
object DbValue:
  /** SQL NULL; compatible with every nullable column. */
  case object NullValue extends DbValue
  /** A `BOOLEAN` value. */
  final case class BoolValue(value: Boolean) extends DbValue
  /** A `BIGINT` value. */
  final case class Int64Value(value: Long) extends DbValue
  /** A `DOUBLE` value. */
  final case class Float64Value(value: Double) extends DbValue
  /** A `STRING` value. */
  final case class StringValue(value: String) extends DbValue
  /** A `BYTES` value. */
  final case class BytesValue(value: Array[Byte]) extends DbValue

  /** Returns the type of a value.
    *
    * @param value the value
    * @return its data type, or `None` for NULL (which has no type of its own)
    */
  def dataType(value: DbValue): Option[DataType] = value match
    case NullValue => None
    case BoolValue(_) => Some(DataType.Bool)
    case Int64Value(_) => Some(DataType.Int64)
    case Float64Value(_) => Some(DataType.Float64)
    case StringValue(_) => Some(DataType.StringType)
    case BytesValue(_) => Some(DataType.Bytes)
