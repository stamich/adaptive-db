package io.adb.plancache

import io.adb.sql.{Token, Tokenizer}

/** Identity of a cacheable statement.
  *
  * The key is the statement's **token stream** plus the optimizer mode, not its text. Whitespace,
  * comments, and the case of nothing else are normalised away by the lexer, so
  * `SELECT  a FROM t -- x` and `SELECT a\nFROM t` share a key. Literals stay part of the key (a
  * different literal is a different plan: literal parameterisation is out of scope), and keyword
  * case is preserved by the lexer, so `select` and `SELECT` are distinct keys. That costs a few
  * duplicate entries and can never merge two different statements.
  *
  * @param tokens the statement's tokens, without the end sentinel
  * @param mode   optimizer mode the plan was made under (`cost` or `rule`)
  */
final case class StatementKey(tokens: Vector[Token], mode: String):
  /** Approximate heap bytes this key holds, used for the cache's byte budget. */
  def weight: Long =
    tokens.foldLeft(48L) { (sum, token) =>
      sum + 24L + (token match
        case Token.Word(v)        => v.length.toLong * 2
        case Token.Number(v)      => v.length.toLong * 2
        case Token.StringToken(v) => v.length.toLong * 2
        case Token.Symbol(v)      => v.length.toLong * 2
        case Token.End            => 0L)
    }

/** Construction of statement keys. */
object StatementKey:
  /** Statements with more tokens than this are never cached (bounds the cost of one key). */
  val MaxTokens: Int = 2048

  /** The key of `sql` under `mode`, or `None` if the statement does not lex or is too long to cache. */
  def of(sql: String, mode: String): Option[StatementKey] =
    try
      val tokens = Tokenizer.tokenize(sql).filter(_ != Token.End)
      if tokens.isEmpty || tokens.size > MaxTokens then None else Some(StatementKey(tokens, mode))
    catch case _: IllegalArgumentException => None
