package io.adb.bench

/** A minimal JSON writer for benchmark reports.
  *
  * The benchmark modules build their documents as strings, so no JSON library is needed. Values
  * are passed around as already-encoded fragments: build them with [[Json.obj]], [[Json.arr]],
  * [[Json.str]] and [[Json.num]].
  */
object Json:
  /** An object of already-encoded values, in the given order. */
  def obj(fields: (String, String)*): String =
    fields.map((key, value) => s"${quote(key)}:$value").mkString("{", ",", "}")

  /** An array of already-encoded values. */
  def arr(values: Iterable[String]): String = values.mkString("[", ",", "]")

  /** A JSON string with the escapes JSON requires. */
  def str(value: String): String = quote(value)

  /** A JSON string, or `null` for `None`. */
  def strOpt(value: Option[String]): String = value.fold("null")(quote)

  /** A JSON integer. */
  def num(value: Long): String = value.toString

  /** A JSON number; `null` when it is NaN or infinite, an integer when it has no fraction. */
  def num(value: Double): String =
    if value.isNaN || value.isInfinite then "null"
    else if value == math.rint(value) && math.abs(value) < 1e15 then value.toLong.toString
    else value.toString

  /** A JSON number, or `null` for `None`. */
  def numOpt(value: Option[Double]): String = value.fold("null")(num)

  /** A JSON boolean. */
  def bool(value: Boolean): String = value.toString

  /** `s` as a quoted JSON string. */
  private def quote(s: String): String =
    val out = new StringBuilder("\"")
    s.foreach {
      case '"' => out.append("\\\"")
      case '\\' => out.append("\\\\")
      case '\n' => out.append("\\n")
      case '\r' => out.append("\\r")
      case '\t' => out.append("\\t")
      case c if c < ' ' => out.append(f"\\u${c.toInt}%04x")
      case c => out.append(c)
    }
    out.append('"').toString
