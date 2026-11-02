package io.adb.consistency

/** What a query needs to know about freshness: the session's level and token, and the head of the
  * commit order at planning time.
  *
  * @param consistency   the session's consistency level
  * @param token         the session's token
  * @param headCommitSeq newest published commit when the query was planned
  * @param maxWaitMillis longest the query may wait for a projection to catch up
  */
final case class SemanticReadContext(
    consistency: ReadConsistency,
    token: SessionConsistencyToken,
    headCommitSeq: Long,
    maxWaitMillis: Long = SemanticReadContext.DefaultMaxWaitMillis
):
  require(headCommitSeq >= 0, "head must not be negative")
  require(maxWaitMillis >= 0, "wait limit must not be negative")

  /** The commit a projection must have consumed before it may serve the query, if any.
    *
    * Strong reads need the head, read-your-writes the session's token; bounded and eventual reads
    * need no particular commit (a bounded read checks the lag instead).
    */
  def requiredCommitSeq: Option[Long] = consistency match
    case ReadConsistency.StrongLatest => Some(headCommitSeq)
    case ReadConsistency.ReadYourWrites => Some(token.commitSeq)
    case _: ReadConsistency.BoundedStaleness | ReadConsistency.Eventual => None

  /** Whether the session's token is beyond what this database has published: the token comes
    * from another database, or from a commit that did not survive a crash. Waiting cannot help.
    */
  def tokenIsAhead: Boolean = !token.isSatisfiedBy(headCommitSeq)

/** Defaults of [[SemanticReadContext]]. */
object SemanticReadContext:
  /** Longest wait for a projection when the caller names none: 2 seconds. */
  val DefaultMaxWaitMillis: Long = 2000

  /** A context for a session with no writes and the default level. */
  def initial(headCommitSeq: Long): SemanticReadContext =
    SemanticReadContext(ReadConsistency.Default, SessionConsistencyToken.Initial, headCommitSeq)
