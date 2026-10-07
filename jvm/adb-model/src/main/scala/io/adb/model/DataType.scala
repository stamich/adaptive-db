package io.adb.model

/** Logical column type known to the SQL layer; each maps to one engine physical type. */
sealed trait DataType derives CanEqual:
  /** Canonical SQL spelling of the type, used in DDL output and error messages. */
  def sqlName: String

/** The supported column types and a parser for their SQL names. */
object DataType:
  /** Boolean column (`BOOLEAN`). */
  case object Bool extends DataType:
    val sqlName = "BOOLEAN"
  /** Signed 64-bit integer column (`BIGINT`); the only primary-key type. */
  case object Int64 extends DataType:
    val sqlName = "BIGINT"
  /** IEEE-754 double column (`DOUBLE`). */
  case object Float64 extends DataType:
    val sqlName = "DOUBLE"
  /** UTF-8 string column (`STRING`). */
  case object StringType extends DataType:
    val sqlName = "STRING"
  /** Opaque byte-array column (`BYTES`). */
  case object Bytes extends DataType:
    val sqlName = "BYTES"

  /** Parses a SQL type name, case-insensitively, accepting common aliases.
    *
    * @param name type name as written in DDL
    * @return the type, or `None` if the name is unknown
    */
  def parse(name: String): Option[DataType] =
    name.toUpperCase match
      case "BOOLEAN" | "BOOL" => Some(Bool)
      case "BIGINT" | "LONG" | "INT64" => Some(Int64)
      case "DOUBLE" | "FLOAT64" => Some(Float64)
      case "STRING" | "TEXT" | "VARCHAR" => Some(StringType)
      case "BYTES" | "BINARY" => Some(Bytes)
      case _ => None
