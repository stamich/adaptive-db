package io.adb.sql

import io.adb.sql.SqlExpr.*
import io.adb.sql.SqlBinaryOp.*

/** Documents `SqlParser` and its role in the Milestone 2.0.1 JVM control plane. */
final class SqlParser:
  /** Documents `parse` and its role in the Milestone 2.0.1 JVM control plane. */
  def parse(sql: String): Statement =
    val p = ParserState(Tokenizer.tokenize(sql))
    val statement = p.parseStatement()
    p.acceptSymbol(";")
    p.expectEnd()
    statement

/** Documents `ParserState` and its role in the Milestone 2.0.1 JVM control plane. */
private final case class ParserState(tokens: Vector[Token], var pos: Int = 0):
  /** Documents `current` and its role in the Milestone 2.0.1 JVM control plane. */
  private def current: Token = tokens(pos)
  /** Documents `advance` and its role in the Milestone 2.0.1 JVM control plane. */
  private def advance(): Token = { val t = current; pos += 1; t }

  /** Documents `parseStatement` and its role in the Milestone 2.0.1 JVM control plane. */
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

  /** Documents `parseCreate` and its role in the Milestone 2.0.1 JVM control plane. */
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

  /** Documents `parseInsert` and its role in the Milestone 2.0.1 JVM control plane. */
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

  /** Documents `parseSelect` and its role in the Milestone 2.0.1 JVM control plane. */
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

  /** Documents `parseUpdate` and its role in the Milestone 2.0.1 JVM control plane. */
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

  /** Documents `parseDelete` and its role in the Milestone 2.0.1 JVM control plane. */
  private def parseDelete(): Statement =
    expectWord("FROM")
    val table = expectIdentifier()
    expectWord("WHERE")
    Delete(table, parseExpr())

  /** Documents `parseExpr` and its role in the Milestone 2.0.1 JVM control plane. */
  private def parseExpr(): SqlExpr = parseOr()
  /** Documents `parseOr` and its role in the Milestone 2.0.1 JVM control plane. */
  private def parseOr(): SqlExpr =
    var left = parseAnd()
    while acceptWord("OR") do left = Binary(left, Or, parseAnd())
    left
  /** Documents `parseAnd` and its role in the Milestone 2.0.1 JVM control plane. */
  private def parseAnd(): SqlExpr =
    var left = parseComparison()
    while acceptWord("AND") do left = Binary(left, And, parseComparison())
    left
  /** Documents `parseComparison` and its role in the Milestone 2.0.1 JVM control plane. */
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
  /** Documents `parsePrimary` and its role in the Milestone 2.0.1 JVM control plane. */
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

  /** Documents `parseIdentifierList` and its role in the Milestone 2.0.1 JVM control plane. */
  private def parseIdentifierList(): Vector[String] =
    val out = Vector.newBuilder[String]
    out += expectIdentifier()
    while acceptSymbol(",") do out += expectIdentifier()
    out.result()

  /** Documents `parseExprList` and its role in the Milestone 2.0.1 JVM control plane. */
  private def parseExprList(): Vector[SqlExpr] =
    val out = Vector.newBuilder[SqlExpr]
    out += parsePrimary()
    while acceptSymbol(",") do out += parsePrimary()
    out.result()

  /** Documents `acceptWord` and its role in the Milestone 2.0.1 JVM control plane. */
  def acceptWord(word: String): Boolean = current match
    case Token.Word(value) if value.equalsIgnoreCase(word) => advance(); true
    case _ => false
  /** Documents `expectWord` and its role in the Milestone 2.0.1 JVM control plane. */
  def expectWord(word: String): Unit = if !acceptWord(word) then throw error(s"expected $word")
  /** Documents `acceptSymbol` and its role in the Milestone 2.0.1 JVM control plane. */
  def acceptSymbol(symbol: String): Boolean = current match
    case Token.Symbol(value) if value == symbol => advance(); true
    case _ => false
  /** Documents `expectSymbol` and its role in the Milestone 2.0.1 JVM control plane. */
  def expectSymbol(symbol: String): Unit = if !acceptSymbol(symbol) then throw error(s"expected '$symbol'")
  /** Documents `expectIdentifier` and its role in the Milestone 2.0.1 JVM control plane. */
  def expectIdentifier(): String = current match
    case Token.Word(value) => advance(); value
    case other => throw error(s"expected identifier, got $other")
  /** Documents `expectLong` and its role in the Milestone 2.0.1 JVM control plane. */
  def expectLong(): Long = current match
    case Token.Number(value) if !value.contains('.') => advance(); value.toLong
    case other => throw error(s"expected integer, got $other")
  /** Documents `expectEnd` and its role in the Milestone 2.0.1 JVM control plane. */
  def expectEnd(): Unit = current match
    case Token.End => ()
    case other => throw error(s"unexpected trailing token $other")
  /** Documents `error` and its role in the Milestone 2.0.1 JVM control plane. */
  private def error(message: String): IllegalArgumentException = new IllegalArgumentException(s"SQL parse error at token $pos: $message")
