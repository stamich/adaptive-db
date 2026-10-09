package io.adb.model

/** Minimal strict JSON reader for engine-produced documents (runtime profiles, statistics).
  *
  * Objects become `Map[String, Any]`, arrays `Vector[Any]`, integers `Long` (or `BigInt` when
  * they do not fit), other numbers `Double`, plus `String`, `Boolean` and `null`. The JVM
  * modules have no JSON dependency; the documents they read are small and produced by the
  * engine, but the reader still bounds nesting and reports every malformation as an
  * `IllegalArgumentException`.
  */
object JsonReader:
  /** Deepest accepted nesting of arrays and objects. */
  val MaxDepth: Int = 64

  /** Parses one complete JSON document.
    *
    * @throws IllegalArgumentException on malformed input
    */
  def parse(text: String): Any =
    val reader = new Cursor(text)
    val value = reader.value(0)
    reader.skipWhitespace()
    if reader.position != text.length then reader.fail("trailing characters")
    value

  /** Position-tracking recursive-descent reader. */
  private final class Cursor(text: String):
    /** Index of the next character. */
    var position = 0

    /** Reads any value nested `depth` levels deep. */
    def value(depth: Int): Any =
      if depth > MaxDepth then fail(s"nesting deeper than $MaxDepth")
      skipWhitespace()
      peek match
        case '{' => obj(depth)
        case '[' => array(depth)
        case '"' => string()
        case 't' => literal("true", true)
        case 'f' => literal("false", false)
        case 'n' => literal("null", null)
        case _ => numberValue()

    /** Reads an object. */
    private def obj(depth: Int): Map[String, Any] =
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
        out += key -> value(depth + 1)
        skipWhitespace()
        if peek == ',' then position += 1 else { expect('}'); more = false }
      out.result()

    /** Reads an array. */
    private def array(depth: Int): Vector[Any] =
      position += 1
      val out = Vector.newBuilder[Any]
      skipWhitespace()
      if peek == ']' then { position += 1; return out.result() }
      var more = true
      while more do
        out += value(depth + 1)
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
          val escaped = peek
          position += 1
          escaped match
            case 'n' => out += '\n'
            case 't' => out += '\t'
            case 'r' => out += '\r'
            case 'b' => out += '\b'
            case 'f' => out += '\f'
            case 'u' =>
              if position + 4 > text.length then fail("truncated \\u escape")
              val hex = text.substring(position, position + 4)
              out += Integer.parseInt(hex, 16).toChar
              position += 4
            case '"' | '\\' | '/' => out += escaped
            case other => fail(s"invalid escape \\$other")
        else out += c
      position += 1
      out.result()

    /** Reads a number: `Long` when integral and in range, `BigInt` when integral and larger,
      * `Double` otherwise.
      */
    private def numberValue(): Any =
      val start = position
      while position < text.length && "+-0123456789.eE".contains(text(position)) do position += 1
      val token = text.substring(start, position)
      if token.isEmpty then fail("unexpected character")
      token.toLongOption
        .orElse(Option.when(token.matches("-?[0-9]+"))(BigInt(token)))
        .orElse(token.toDoubleOption.filter(d => !d.isNaN && !d.isInfinite))
        .getOrElse(fail(s"bad number $token"))

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
