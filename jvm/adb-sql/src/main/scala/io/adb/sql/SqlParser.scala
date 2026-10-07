package io.adb.sql

import io.adb.sql.SqlExpr.*
import io.adb.sql.SqlBinaryOp.*

/** Hand-written recursive-descent parser for the Adaptive DB SQL subset.
  *
  * Statements: CREATE TABLE, INSERT, SELECT, UPDATE, DELETE and EXPLAIN [ANALYZE]. Since 2.1 a
  * SELECT may join tables (INNER / LEFT / CROSS), alias them, qualify columns, aggregate with
  * COUNT / SUM / MIN / MAX / AVG, GROUP BY and ORDER BY:
  *
  * {{{
  * SELECT c.name, SUM(o.amount) AS total
  * FROM customer c JOIN orders o ON o.customer_id = c.id
  * WHERE o.amount > 10
  * GROUP BY c.name ORDER BY total DESC LIMIT 3
  * }}}
  */
final class SqlParser:
  /** Parses exactly one statement, optionally terminated by `;`.
    *
    * @param sql statement text
    * @return the parsed AST
    * @throws IllegalArgumentException with the failing token position on any syntax error or
    *         exceeded parser budget
    */
  def parse(sql: String): Statement =
    val p = ParserState(Tokenizer.tokenize(sql))
    val statement = p.parseStatement()
    p.acceptSymbol(";")
    p.expectEnd()
    statement

/** Size budgets of one statement (on top of the tokenizer's limits). */
object SqlParser:
  /** Select-list entries per SELECT. */
  val MaxSelectItems: Int = 4096
  /** JOIN clauses per SELECT (so at most 64 relations). */
  val MaxJoins: Int = 63
  /** GROUP BY entries per SELECT. */
  val MaxGroupBy: Int = 4096
  /** ORDER BY entries per SELECT. */
  val MaxOrderBy: Int = 4096
  /** Words that end a FROM item and therefore can never be an implicit alias. */
  private[sql] val ClauseKeywords: Set[String] =
    Set("WHERE", "JOIN", "INNER", "LEFT", "OUTER", "CROSS", "ON", "GROUP", "ORDER", "LIMIT", "AS", "BY", "FROM")

/** Mutable cursor over the token stream of one statement.
  *
  * @param tokens tokens ending with `Token.End`
  * @param pos    index of the current token
  */
private final case class ParserState(tokens: Vector[Token], var pos: Int = 0):
  /** The token at the cursor. */
  private def current: Token = tokens(pos)
  /** The token `offset` positions after the cursor (`Token.End` past the end). */
  private def peek(offset: Int): Token = tokens.lift(pos + offset).getOrElse(Token.End)
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

  /** Parses the rest of a SELECT: select list, FROM with joins, `AS OF VERSION n`, WHERE,
    * GROUP BY, ORDER BY and LIMIT (which must fit in an `Int`).
    */
  private def parseSelect(): Statement =
    val (items, star) =
      if acceptSymbol("*") then (Vector.empty, true)
      else (commaSeparated("select list", SqlParser.MaxSelectItems)(parseSelectItem()), false)
    expectWord("FROM")
    val from = parseFrom()
    val asOf =
      if acceptWord("AS") then { expectWord("OF"); expectWord("VERSION"); Some(expectLong()) }
      else None
    val where = if acceptWord("WHERE") then Some(parseExpr()) else None
    val groupBy =
      if acceptWord("GROUP") then
        expectWord("BY")
        commaSeparated("GROUP BY", SqlParser.MaxGroupBy)(parseColumnRef())
      else Vector.empty
    val orderBy =
      if acceptWord("ORDER") then
        expectWord("BY")
        commaSeparated("ORDER BY", SqlParser.MaxOrderBy) {
          val expr = parsePrimary()
          val descending = if acceptWord("DESC") then true else { acceptWord("ASC"); false }
          OrderItem(expr, descending)
        }
      else Vector.empty
    val limit =
      if acceptWord("LIMIT") then
        val value = expectLong()
        if value < 0 || value > Int.MaxValue then throw error("LIMIT must be between 0 and 2147483647")
        Some(value.toInt)
      else None
    Select(items, star, from, where, groupBy, orderBy, limit, asOf)

  /** Parses one select-list entry: an expression with an optional `[AS] alias`. */
  private def parseSelectItem(): SelectItem =
    val expr = parsePrimary()
    val alias =
      if acceptWord("AS") then Some(expectIdentifier())
      else current match
        case Token.Word(word) if !SqlParser.ClauseKeywords.contains(word.toUpperCase) => advance(); Some(word)
        case _ => None
    SelectItem(expr, alias)

  /** Parses `table [alias] { [INNER|LEFT [OUTER]|CROSS] JOIN table [alias] [ON expr] }`. */
  private def parseFrom(): FromClause =
    val base = parseTableRef()
    val joins = Vector.newBuilder[JoinClause]
    var count = 0
    var more = true
    while more do
      val kind =
        if acceptWord("JOIN") then Some(JoinKind.Inner)
        else if acceptWord("INNER") then { expectWord("JOIN"); Some(JoinKind.Inner) }
        else if acceptWord("LEFT") then { acceptWord("OUTER"); expectWord("JOIN"); Some(JoinKind.Left) }
        else if acceptWord("CROSS") then { expectWord("JOIN"); Some(JoinKind.Cross) }
        else None
      kind match
        case None => more = false
        case Some(kind) =>
          count += 1
          if count > SqlParser.MaxJoins then throw error(s"more than ${SqlParser.MaxJoins} joins")
          val table = parseTableRef()
          val on =
            if kind == JoinKind.Cross then None
            else { expectWord("ON"); Some(parseExpr()) }
          joins += JoinClause(kind, table, on)
    FromClause(base, joins.result())

  /** Parses `table [AS alias | alias]`; `AS OF` is left for the snapshot clause. */
  private def parseTableRef(): TableRef =
    val table = expectIdentifier()
    val alias = (current, peek(1)) match
      case (Token.Word(as), Token.Word(next)) if as.equalsIgnoreCase("AS") && !next.equalsIgnoreCase("OF") =>
        advance(); Some(expectIdentifier())
      case (Token.Word(word), _) if !SqlParser.ClauseKeywords.contains(word.toUpperCase) =>
        advance(); Some(word)
      case _ => None
    TableRef(table, alias)

  /** Parses `name` or `qualifier.name`. */
  private def parseColumnRef(): Column =
    val first = expectIdentifier()
    if acceptSymbol(".") then Column(expectIdentifier(), Some(first)) else Column(first)

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
  /** Parses a literal, (qualified) column reference, aggregate call, `NOT` primary, or
    * parenthesized expression.
    */
  private def parsePrimary(): SqlExpr = current match
    case Token.Number(value) =>
      advance(); if value.contains('.') then DoubleLiteral(value.toDouble) else LongLiteral(value.toLong)
    case Token.StringToken(value) => advance(); StringLiteral(value)
    case Token.Word(value) if value.equalsIgnoreCase("TRUE") => advance(); BoolLiteral(true)
    case Token.Word(value) if value.equalsIgnoreCase("FALSE") => advance(); BoolLiteral(false)
    case Token.Word(value) if value.equalsIgnoreCase("NULL") => advance(); NullLiteral
    case Token.Word(value) if value.equalsIgnoreCase("NOT") => advance(); Not(parsePrimary())
    case Token.Word(value) if peek(1) == Token.Symbol("(") && SqlAggregate.named(value).isDefined =>
      parseAggregateCall(SqlAggregate.named(value).get)
    case Token.Word(_) => parseColumnRef()
    case Token.Symbol("(") => advance(); val e = parseExpr(); expectSymbol(")"); e
    case other => throw error(s"expected expression, got $other")

  /** Parses `FUNCTION(*)` (COUNT only) or `FUNCTION(argument)`. */
  private def parseAggregateCall(function: SqlAggregate): SqlExpr =
    advance()
    expectSymbol("(")
    val argument =
      if acceptSymbol("*") then
        if function != SqlAggregate.Count then throw error(s"${function.toString.toUpperCase}(*) is not valid")
        None
      else Some(parseExpr())
    expectSymbol(")")
    AggregateCall(function, argument)

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

  /** Parses a non-empty comma-separated list of at most `max` entries produced by `item`. */
  private def commaSeparated[A](what: String, max: Int)(item: => A): Vector[A] =
    val out = Vector.newBuilder[A]
    var count = 1
    out += item
    while acceptSymbol(",") do
      count += 1
      if count > max then throw error(s"$what has more than $max entries")
      out += item
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
