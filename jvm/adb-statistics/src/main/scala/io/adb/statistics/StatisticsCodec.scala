package io.adb.statistics

import io.adb.model.*

/** Decodes the engine's statistics document (`adb_statistics_json`, format version 1).
  *
  * The decoder is strict: a missing or mistyped field, an unknown format version or an
  * inconsistent count fails with `IllegalArgumentException`, so a planner never runs on a
  * misread document. Values use the engine's JSON form (`"Null"`, `{"Int64": 5}`, ...).
  */
object StatisticsCodec:
  /** The only document format this decoder understands. */
  val FormatVersion: Long = 1

  /** Decodes one document.
    *
    * @throws IllegalArgumentException if the document is malformed
    */
  def decode(json: String): TableStatistics =
    val root = obj(JsonReader.parse(json), "statistics")
    val version = long(root, "format_version")
    require(version == FormatVersion, s"unsupported statistics format version $version")
    val rowCount = long(root, "row_count")
    val columns = array(root, "columns").map(column => decodeColumn(obj(column, "column"), rowCount))
    require(columns.map(_.fieldId).distinct.size == columns.size, "duplicate column statistics")
    TableStatistics(
      entityId = EntityId(long(root, "entity_id")),
      rowCount = rowCount,
      avgRowBytes = double(root, "avg_row_bytes"),
      analyzedAtTs = long(root, "analyzed_at_ts"),
      modificationsAtAnalyze = long(root, "modifications_at_analyze"),
      sampledRows = long(root, "sampled_rows"),
      exact = bool(root, "exact"),
      columns = columns.map(c => c.fieldId -> c).toMap
    )

  /** Decodes one column and checks its counts against the table's row count. */
  private def decodeColumn(column: Map[String, Any], rowCount: Long): ColumnStatistics =
    val fieldId = long(column, "field_id")
    require(fieldId >= 0 && fieldId <= Int.MaxValue, s"field id $fieldId out of range")
    val decoded = ColumnStatistics(
      fieldId = FieldId(fieldId.toInt),
      nullCount = long(column, "null_count"),
      distinctCount = long(column, "distinct_count"),
      distinctExact = bool(column, "distinct_exact"),
      min = optionalValue(column, "min"),
      max = optionalValue(column, "max"),
      avgWidthBytes = double(column, "avg_width_bytes"),
      histogram = array(column, "histogram").map { item =>
        val bucket = obj(item, "bucket")
        HistogramBucket(value(bucket("lower")), value(bucket("upper")), long(bucket, "rows"), long(bucket, "distinct"))
      },
      mostCommon = array(column, "most_common").map { item =>
        val common = obj(item, "most common value")
        MostCommonValue(value(common("value")), long(common, "rows"))
      }
    )
    require(decoded.nullCount <= rowCount, s"field $fieldId has more NULLs than rows")
    require(decoded.distinctCount <= rowCount - decoded.nullCount, s"field $fieldId has more distinct values than non-NULL rows")
    decoded

  /** Decodes an engine value. */
  def value(raw: Any): DbValue = raw match
    case "Null" => DbValue.NullValue
    case tagged: Map[?, ?] if tagged.size == 1 =>
      val (tag, payload) = tagged.head.asInstanceOf[(Any, Any)]
      (tag, payload) match
        case ("Bool", v: Boolean) => DbValue.BoolValue(v)
        case ("Int64", v: Long) => DbValue.Int64Value(v)
        case ("Float64", v: Double) => DbValue.Float64Value(v)
        case ("Float64", v: Long) => DbValue.Float64Value(v.toDouble)
        case ("String", v: String) => DbValue.StringValue(v)
        case ("Bytes", v: Vector[?]) => DbValue.BytesValue(v.map(byte).toArray)
        case _ => throw new IllegalArgumentException(s"invalid value $raw")
    case _ => throw new IllegalArgumentException(s"invalid value $raw")

  /** One byte of a `Bytes` value. */
  private def byte(raw: Any): Byte = raw match
    case b: Long if b >= 0 && b <= 255 => b.toByte
    case _ => throw new IllegalArgumentException(s"invalid byte $raw")

  /** An optional value: absent or JSON null both mean "none". */
  private def optionalValue(map: Map[String, Any], key: String): Option[DbValue] =
    map.get(key).flatMap(Option(_)).map(value)

  /** A JSON object or a decode error naming `what`. */
  private def obj(raw: Any, what: String): Map[String, Any] = raw match
    case map: Map[?, ?] => map.asInstanceOf[Map[String, Any]]
    case other => throw new IllegalArgumentException(s"$what must be an object, got $other")

  /** A required array field. */
  private def array(map: Map[String, Any], key: String): Vector[Any] = map.get(key) match
    case Some(values: Vector[?]) => values
    case other => throw new IllegalArgumentException(s"$key must be an array, got $other")

  /** A required non-negative integer field. */
  private def long(map: Map[String, Any], key: String): Long = map.get(key) match
    case Some(v: Long) if v >= 0 => v
    case other => throw new IllegalArgumentException(s"$key must be a non-negative integer, got $other")

  /** A required finite, non-negative number field. */
  private def double(map: Map[String, Any], key: String): Double = map.get(key) match
    case Some(v: Double) if v >= 0 => v
    case Some(v: Long) if v >= 0 => v.toDouble
    case other => throw new IllegalArgumentException(s"$key must be a non-negative number, got $other")

  /** A required boolean field. */
  private def bool(map: Map[String, Any], key: String): Boolean = map.get(key) match
    case Some(v: Boolean) => v
    case other => throw new IllegalArgumentException(s"$key must be a boolean, got $other")
