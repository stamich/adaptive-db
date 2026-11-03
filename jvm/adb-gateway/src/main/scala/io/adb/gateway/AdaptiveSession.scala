package io.adb.gateway

import io.adb.consistency.*

/** One client's view of freshness: its consistency level, its token and its wait limit.
  *
  * Statements run through a session ([[AdaptiveDatabase.session]]) so that the session can
  * remember the position of its own writes (a DML statement's commit, or the head after DDL) and
  * later reads can be made to include them, even when they would be served by a projection that
  * lags behind. Sessions are independent: a write by one does not move another's token.
  *
  * A session is safe to use from several threads; the token only moves forward.
  *
  * @param database the database the session runs statements on
  * @param backend  the engine's commit order and projections
  */
final class AdaptiveSession private[gateway] (database: AdaptiveDatabase, backend: ConsistencyBackend):
  /** The position of the newest commit this session made or observed. */
  @volatile private var current: SessionConsistencyToken = SessionConsistencyToken.Initial
  /** The level reads use. */
  @volatile private var level: ReadConsistency = ReadConsistency.Default
  /** The longest a read may wait for a projection. */
  @volatile private var waitMillis: Long = SemanticReadContext.DefaultMaxWaitMillis

  /** Executes one SQL statement in this session. */
  def execute(sql: String): QueryResult = database.executeIn(sql, this)

  /** The session's token. */
  def token: SessionConsistencyToken = current

  /** Restores a token carried over from another connection or service. Tokens never move backward,
    * so this can only raise the session's position.
    */
  def observe(token: SessionConsistencyToken): Unit = observe(token.commitSeq)

  /** Records that this session made or saw commit `commitSeq`. */
  private[gateway] def observe(commitSeq: Long): Unit = synchronized { current = current.advancedTo(commitSeq) }

  /** The session's consistency level. */
  def consistency: ReadConsistency = level

  /** Changes the level (`SET SESSION CONSISTENCY`). */
  def consistency_=(newLevel: ReadConsistency): Unit = level = newLevel

  /** The longest a read may wait for a projection, in milliseconds. */
  def maxWaitMillis: Long = waitMillis

  /** Changes the wait limit. */
  def maxWaitMillis_=(millis: Long): Unit =
    require(millis >= 0 && millis <= ConsistencyBackend.MaxWaitMillis, s"wait limit must be 0..${ConsistencyBackend.MaxWaitMillis} ms")
    waitMillis = millis

  /** What a read planned now needs to know about freshness. */
  def readContext(): SemanticReadContext = SemanticReadContext(level, current, backend.latestCommitSeq(), waitMillis)
