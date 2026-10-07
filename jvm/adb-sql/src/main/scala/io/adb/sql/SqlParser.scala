package io.adb.sql

import io.adb.sql.SqlExpr.*
import io.adb.sql.SqlBinaryOp.*

/** Hand-written recursive-descent parser for the Milestone 2 SQL subset (CREATE TABLE, INSERT, SELECT, UPDATE, DELETE, EXPLAIN [ANALYZE]). */
final class SqlParser:
  /** Parses exactly one statement, optionally terminated by `;`.
    *
    * @param sql statement text
    * @return the parsed AST
    * @throws IllegalArgumentException with the failing token position on any syntax error
    */
  def parse(sql: String): Statement =
    val p = ParserState(Tokenizer.tokenize(sql))
    val statement = p.parseStatement()
    p.acceptSymbol(";")
    p.expectEnd()
    statement

/** Mutable cursor over the token stream of one statement.
  *
  * @param tokens tokens ending with `Token.End`
  * @param pos    index of the current token
  */
private final case class ParserState(tokens: Vector[Token], var pos: Int = 0):
  /** The token at the cursor. */
  private def current: Token = tokens(pos)
  /** Consumes and returns the current token. */
  private def advance(): Token = { val t = current; pos += 1; t }

  /** Parses an optional chain of `EXPLAIN [ANALYZE]` prefixes (at most 64) followed by one base statement. */
  def parseStatement(): Statement =
    val explainModes = Vector.newBuilder[Boolean]
    var explainDepth = 0
    while acceptWord("EXPLAIN") do
      explainDepth += 1
      if explainDepth > 64 then throw error("EXPLAIN nesting exceeds 64")
      explainModes += acceptWord("ANALYZE")

    val base =
      if acceptWord("CREATE") then parseCreate()
      else if acceptWord("INSERT") then parseInsert()
      else if acceptWord("SELECT") then parseSelect()
      else if acceptWord("UPDATE") then parseUpdate()
      else if acceptWord("DELETE") then parseDelete()
      else throw error(s"expected statement, got $current")

    explainModes.result().reverse.foldLeft(base) { case (inner, analyze) => Explain(inner, analyze) }

  /** Parses the rest of `CREATE TABLE name (column type [NOT NULL] [PRIMARY KEY], ...)`. */
  private def parseCreate(): Statement =
    expectWord("TABLE")
    val name = expectIdentifier()
    expectSymbol("(")
    val columns = Vector.newBuilder[ColumnDef]
    var done = false
    while !done do
      val columnName = expectIdentifier()
      val dataType = expectIdentifier()
      var nullable = true
      var primaryKey = false
      var modifiers = true
      while modifiers do
        if acceptWord("NOT") then { expectWord("NULL"); nullable = false }
        else if acceptWord("PRIMARY") then { expectWord("KEY"); primaryKey = true; nullable = false }
        else modifiers = false
      columns += ColumnDef(columnName, dataType, nullable, primaryKey)
      if acceptSymbol(",") then () else { expectSymbol(")"); done = true }
    CreateTable(name, columns.result())

  /** Parses the rest of `INSERT INTO table [(columns)] VALUES (values)`. */
  private def parseInsert(): Statement =
    expectWord("INTO")
    val table = expectIdentifier()
    val columns =
      if acceptSymbol("(") then
        val names = parseIdentifierList()
        expectSymbol(")")
        Some(names)
      else None
    expectWord("VALUES")
    expectSymbol("(")
    val values = parseExprList()
    expectSymbol(")")
    Insert(table, columns, values)

  /** Parses the rest of `SELECT ... FROM table [AS OF VERSION n] [WHERE expr] [LIMIT n]`; LIMIT must fit in an `Int`. */
  private def parseSelect(): Statement =
    val (columns, star) =
      if acceptSymbol("*") then (Vector.empty, true)
      else (parseIdentifierList(), false)
    expectWord("FROM")
    val table = expectIdentifier()
    val asOf =
      if acceptWord("AS") then { expectWord("OF"); expectWord("VERSION"); Some(expectLong()) }
      else None
    val where = if acceptWord("WHERE") then Some(parseExpr()) else None
    val limit =
      if acceptWord("LIMIT") then
        val value = expectLong()
        if value < 0 || value > Int.MaxValue then throw error("LIMIT must be between 0 and 2147483647")
        Some(value.toInt)
      else None
    Select(columns, star, table, where, limit, asOf)

  /** Parses the rest of `UPDATE table SET column = value, ... WHERE expr`. */
  private def parseUpdate(): Statement =
    val table = expectIdentifier()
    expectWord("SET")
    val assignments = Vector.newBuilder[(String, SqlExpr)]
    var done = false
    while !done do
      val name = expectIdentifier()
      expectSymbol("=")
      assignments += name -> parsePrimary()
      if acceptSymbol(",") then () else done = true
    expectWord("WHERE")
    Update(table, assignments.result(), parseExpr())

  /** Parses the rest of `DELETE FROM table WHERE expr`. */
  private def parseDelete(): Statement =
    expectWord("FROM")
    val table = expectIdentifier()
    expectWord("WHERE")
    Delete(table, parseExpr())

  /** Parses an expression (entry point of the precedence chain OR < AND < comparison < primary). */
  private def parseExpr(): SqlExpr = parseOr()
  /** Parses left-associative `OR` chains. */
  private def parseOr(): SqlExpr =
    var left = parseAnd()
    while acceptWord("OR") do left = Binary(left, Or, parseAnd())
    left
  /** Parses left-associative `AND` chains. */
  private def parseAnd(): SqlExpr =
    var left = parseComparison()
    while acceptWord("AND") do left = Binary(left, And, parseComparison())
    left
  /** Parses an optional single comparison between two primaries (comparisons do not chain). */
  private def parseComparison(): SqlExpr =
    var left = parsePrimary()
    current match
      case Token.Symbol(op @ ("=" | "!=" | "<>" | "<" | "<=" | ">" | ">=")) =>
        advance()
        val mapped = op match
          case "=" => Eq; case "!=" | "<>" => Ne; case "<" => Lt; case "<=" => Le
          case ">" => Gt; case ">=" => Ge
        left = Binary(left, mapped, parsePrimary())
      case _ => ()
    left
  /** Parses a literal, column reference, `NOT` primary, or parenthesized expression. */
  private def parsePrimary(): SqlExpr = current match
    case Token.Number(value) =>
      advance(); if value.contains('.') then DoubleLiteral(value.toDouble) else LongLiteral(value.toLong)
    case Token.StringToken(value) => advance(); StringLiteral(value)
    case Token.Word(value) if value.equalsIgnoreCase("TRUE") => advance(); BoolLiteral(true)
    case Token.Word(value) if value.equalsIgnoreCase("FALSE") => advance(); BoolLiteral(false)
    case Token.Word(value) if value.equalsIgnoreCase("NULL") => advance(); NullLiteral
    case Token.Word(value) if value.equalsIgnoreCase("NOT") => advance(); Not(parsePrimary())
    case Token.Word(value) => advance(); Column(value)
    case Token.Symbol("(") => advance(); val e = parseExpr(); expectSymbol(")"); e
    case other => throw error(s"expected expression, got $other")

  /** Parses a non-empty comma-separated identifier list. */
  private def parseIdentifierList(): Vector[String] =
    val out = Vector.newBuilder[String]
    out += expectIdentifier()
    while acceptSymbol(",") do out += expectIdentifier()
    out.result()

  /** Parses a non-empty comma-separated list of value expressions. */
  private def parseExprList(): Vector[SqlExpr] =
    val out = Vector.newBuilder[SqlExpr]
    out += parsePrimary()
    while acceptSymbol(",") do out += parsePrimary()
    out.result()

  /** Consumes the current token if it is the keyword `word` (case-insensitive).
    *
    * @return whether the keyword was consumed
    */
  def acceptWord(word: String): Boolean = current match
    case Token.Word(value) if value.equalsIgnoreCase(word) => advance(); true
    case _ => false
  /** Consumes the keyword `word` or fails. */
  def expectWord(word: String): Unit = if !acceptWord(word) then throw error(s"expected $word")
  /** Consumes the current token if it is the symbol `symbol`.
    *
    * @return whether the symbol was consumed
    */
  def acceptSymbol(symbol: String): Boolean = current match
    case Token.Symbol(value) if value == symbol => advance(); true
    case _ => false
  /** Consumes the symbol `symbol` or fails. */
  def expectSymbol(symbol: String): Unit = if !acceptSymbol(symbol) then throw error(s"expected '$symbol'")
  /** Consumes and returns an identifier (any word token) or fails. */
  def expectIdentifier(): String = current match
    case Token.Word(value) => advance(); value
    case other => throw error(s"expected identifier, got $other")
  /** Consumes and returns an integer literal or fails. */
  def expectLong(): Long = current match
    case Token.Number(value) if !value.contains('.') => advance(); value.toLong
    case other => throw error(s"expected integer, got $other")
  /** Fails unless the whole input has been consumed. */
  def expectEnd(): Unit = current match
    case Token.End => ()
    case other => throw error(s"unexpected trailing token $other")
  /** Builds a parse error that names the current token position. */
  private def error(message: String): IllegalArgumentException = new IllegalArgumentException(s"SQL parse error at token $pos: $message")
