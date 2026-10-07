package io.adb.gateway

/** Renders the native query profile (`adb_query_profile_json`) as an indented operator tree:
  *
  * {{{
  * peak memory 18.2 KiB of 256.0 MiB
  * top_k rows=3 batches=1 time=0.21ms rows_in=4 compactions=1 peak_memory_bytes=1296
  *   aggregate rows=4 ...
  * }}}
  */
object ProfileRenderer:
  /** Renders a profile document. */
  def render(json: String): String =
    val profile = JsonReader.parse(json).asInstanceOf[Map[String, Any]]
    val peak = number(profile("peak_memory_bytes"))
    val limit = number(profile("memory_limit_bytes"))
    (s"peak memory ${bytes(peak)} of ${bytes(limit)}" +: renderOperator(profile("root"), 0)).mkString("\n")

  /** Lines of one operator and its inputs. */
  private def renderOperator(node: Any, depth: Int): Vector[String] =
    val operator = node.asInstanceOf[Map[String, Any]]
    val counters = operator.get("counters").map(_.asInstanceOf[Map[String, Any]]).getOrElse(Map.empty)
    val micros = number(operator("elapsed_us"))
    val line = ("  " * depth) + s"${operator("operator")} rows=${number(operator("rows_out"))} " +
      s"batches=${number(operator("batches_out"))} time=${f"${micros / 1000.0}%.2f"}ms" +
      counters.toVector.sortBy(_._1).map((name, value) => s" $name=${number(value)}").mkString
    val children = operator.get("children").map(_.asInstanceOf[Vector[Any]]).getOrElse(Vector.empty)
    line +: children.flatMap(renderOperator(_, depth + 1))

  /** A JSON number as `Long`. */
  private def number(value: Any): Long = value match
    case n: Long => n
    case n: Double => n.toLong
    case other => throw new IllegalArgumentException(s"expected a number, got $other")

  /** Human-readable byte size. */
  private def bytes(value: Long): String =
    if value < 1024 then s"$value B"
    else if value < 1024 * 1024 then f"${value / 1024.0}%.1f KiB"
    else f"${value / (1024.0 * 1024)}%.1f MiB"

/** Minimal strict JSON reader for engine-produced documents (objects become `Map[String, Any]`,
  * arrays `Vector[Any]`, integers `Long`, other numbers `Double`, plus `String`, `Boolean` and
  * `null`). The JVM modules have no JSON dependency and the profile is the only JSON they read.
  */
object JsonReader:
  /** Parses one complete JSON document.
    *
    * @throws IllegalArgumentException on malformed input
    */
  def parse(text: String): Any =
    val reader = new Cursor(text)
    val value = reader.value()
    reader.skipWhitespace()
    if reader.position != text.length then reader.fail("trailing characters")
    value

  /** Position-tracking recursive-descent reader. */
  private final class Cursor(text: String):
    /** Index of the next character. */
    var position = 0

    /** Reads any value. */
    def value(): Any =
      skipWhitespace()
      if position >= text.length then fail("unexpected end")
      text(position) match
        case '{' => obj()
        case '[' => array()
        case '"' => string()
        case 't' => literal("true", true)
        case 'f' => literal("false", false)
        case 'n' => literal("null", null)
        case _ => numberValue()

    /** Reads an object. */
    private def obj(): Map[String, Any] =
      position += 1
      val out = Map.newBuilder[String, Any]
      skipWhitespace()
      if peek == '}' then { position += 1; return out.result() }
      var more = true
      while more do
        skipWhitespace()
        val key = string()
        skipWhitespace()
        expect(':')
        out += key -> value()
        skipWhitespace()
        if peek == ',' then position += 1 else { expect('}'); more = false }
      out.result()

    /** Reads an array. */
    private def array(): Vector[Any] =
      position += 1
      val out = Vector.newBuilder[Any]
      skipWhitespace()
      if peek == ']' then { position += 1; return out.result() }
      var more = true
      while more do
        out += value()
        skipWhitespace()
        if peek == ',' then position += 1 else { expect(']'); more = false }
      out.result()

    /** Reads a string with the standard escapes. */
    private def string(): String =
      expect('"')
      val out = new StringBuilder
      while peek != '"' do
        val c = text(position)
        position += 1
        if c == '\\' then
          val escaped = text(position)
          position += 1
          escaped match
            case 'n' => out += '\n'
            case 't' => out += '\t'
            case 'r' => out += '\r'
            case 'b' => out += '\b'
            case 'f' => out += '\f'
            case 'u' =>
              out += Integer.parseInt(text.substring(position, position + 4), 16).toChar
              position += 4
            case other => out += other
        else out += c
      position += 1
      out.result()

    /** Reads a number: `Long` when integral, `Double` otherwise. */
    private def numberValue(): Any =
      val start = position
      while position < text.length && "+-0123456789.eE".contains(text(position)) do position += 1
      val token = text.substring(start, position)
      if token.isEmpty then fail("unexpected character")
      token.toLongOption.getOrElse(token.toDoubleOption.getOrElse(fail(s"bad number $token")))

    /** Reads an exact keyword. */
    private def literal(word: String, result: Any): Any =
      if !text.startsWith(word, position) then fail(s"expected $word")
      position += word.length
      result

    /** Skips JSON whitespace. */
    def skipWhitespace(): Unit = while position < text.length && text(position).isWhitespace do position += 1

    /** The next character (fails at the end). */
    private def peek: Char =
      if position >= text.length then fail("unexpected end")
      text(position)

    /** Consumes `c` or fails. */
    private def expect(c: Char): Unit =
      if peek != c then fail(s"expected '$c'")
      position += 1

    /** Throws a parse error at the current position. */
    def fail(message: String): Nothing = throw new IllegalArgumentException(s"invalid JSON at $position: $message")
