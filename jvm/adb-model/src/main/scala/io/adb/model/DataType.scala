package io.adb.model

/** Documents `DataType` and its role in the Milestone 2.0.1 JVM control plane. */
sealed trait DataType derives CanEqual:
  /** Documents `sqlName` and its role in the Milestone 2.0.1 JVM control plane. */
  def sqlName: String

/** Documents `DataType` and its role in the Milestone 2.0.1 JVM control plane. */
object DataType:
  /** Documents `Bool` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Bool extends DataType:
    val sqlName = "BOOLEAN"
  /** Documents `Int64` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Int64 extends DataType:
    val sqlName = "BIGINT"
  /** Documents `Float64` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Float64 extends DataType:
    val sqlName = "DOUBLE"
  /** Documents `StringType` and its role in the Milestone 2.0.1 JVM control plane. */
  case object StringType extends DataType:
    val sqlName = "STRING"
  /** Documents `Bytes` and its role in the Milestone 2.0.1 JVM control plane. */
  case object Bytes extends DataType:
    val sqlName = "BYTES"

  /** Documents `parse` and its role in the Milestone 2.0.1 JVM control plane. */
  def parse(name: String): Option[DataType] =
    name.toUpperCase match
      case "BOOLEAN" | "BOOL" => Some(Bool)
      case "BIGINT" | "LONG" | "INT64" => Some(Int64)
      case "DOUBLE" | "FLOAT64" => Some(Float64)
      case "STRING" | "TEXT" | "VARCHAR" => Some(StringType)
      case "BYTES" | "BINARY" => Some(Bytes)
      case _ => None
