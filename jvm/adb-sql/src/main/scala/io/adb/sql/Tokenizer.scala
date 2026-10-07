package io.adb.sql

/** Token produced by the SQL tokenizer. */
enum Token derives CanEqual:
  /** SQL identifier or keyword. */
  case Word(value: String)
  /** Numeric literal in source form. */
  case Number(value: String)
  /** SQL single-quoted string literal after quote unescaping. */
  case StringToken(value: String)
  /** Punctuation (`(`, `)`, `,`, `*`, `;`, `.`) or comparison operator. */
  case Symbol(value: String)
  /** Sentinel marking the end of the token stream. */
  case End

/** Bounded tokenizer for the intentionally small SQL grammar (Milestone 2.1). */
object Tokenizer:
  /** Maximum SQL statement length accepted by the JVM control plane. */
  val MaxSqlChars: Int = 1024 * 1024
  /** Maximum number of lexical tokens accepted from one statement. */
  val MaxTokens: Int = 100000
  /** Maximum identifier/keyword length. */
  val MaxIdentifierChars: Int = 256
  /** Maximum decoded string-literal length. */
  val MaxStringLiteralChars: Int = 64 * 1024
  /** Maximum parenthesis nesting accepted before parsing. */
  val MaxParenthesisDepth: Int = 256

  /** Tokenizes one SQL statement while enforcing input, token and nesting limits. */
  def tokenize(input: String): Vector[Token] =
    require(input != null, "SQL input cannot be null")
    require(input.length <= MaxSqlChars, s"SQL exceeds $MaxSqlChars characters")

    val out = Vector.newBuilder[Token]
    var tokenCount = 0
    var depth = 0
    var i = 0

    /** Emits one token while enforcing the global token-count budget. */
    def emit(token: Token): Unit =
      tokenCount += 1
      if tokenCount > MaxTokens then
        throw new IllegalArgumentException(s"SQL exceeds $MaxTokens tokens")
      out += token

    while i < input.length do
      input(i) match
        case ch if ch.isWhitespace => i += 1
        case '-' if i + 1 < input.length && input(i + 1) == '-' =>
          i += 2
          while i < input.length && input(i) != '\n' do i += 1
        case '\'' =>
          val b = new StringBuilder
          i += 1
          var done = false
          while i < input.length && !done do
            if input(i) == '\'' then
              if i + 1 < input.length && input(i + 1) == '\'' then
                b += '\''; i += 2
              else
                done = true; i += 1
            else
              if b.length >= MaxStringLiteralChars then
                throw new IllegalArgumentException(s"string literal exceeds $MaxStringLiteralChars characters")
              b += input(i); i += 1
          if !done then throw new IllegalArgumentException("unterminated string literal")
          emit(Token.StringToken(b.result()))
        case ch if ch.isLetter || ch == '_' =>
          val start = i
          i += 1
          while i < input.length && (input(i).isLetterOrDigit || input(i) == '_') do i += 1
          val length = i - start
          if length > MaxIdentifierChars then
            throw new IllegalArgumentException(s"identifier exceeds $MaxIdentifierChars characters")
          emit(Token.Word(input.substring(start, i)))
        case ch if ch.isDigit || (ch == '-' && i + 1 < input.length && input(i + 1).isDigit) =>
          val start = i
          i += 1
          while i < input.length && (input(i).isDigit || input(i) == '.') do i += 1
          emit(Token.Number(input.substring(start, i)))
        case '<' | '>' | '!' | '=' =>
          val start = i
          i += 1
          if i < input.length && input(i) == '=' then i += 1
          else if input(start) == '<' && i < input.length && input(i) == '>' then i += 1
          emit(Token.Symbol(input.substring(start, i)))
        case '(' =>
          depth += 1
          if depth > MaxParenthesisDepth then
            throw new IllegalArgumentException(s"parenthesis nesting exceeds $MaxParenthesisDepth")
          emit(Token.Symbol("(")); i += 1
        case ')' =>
          depth -= 1
          if depth < 0 then throw new IllegalArgumentException("unmatched closing parenthesis")
          emit(Token.Symbol(")")); i += 1
        case ch @ (',' | '*' | ';' | '.') =>
          emit(Token.Symbol(ch.toString)); i += 1
        case other => throw new IllegalArgumentException(s"unexpected character '$other'")

    if depth != 0 then throw new IllegalArgumentException("unclosed parenthesis")
    emit(Token.End)
    out.result()
