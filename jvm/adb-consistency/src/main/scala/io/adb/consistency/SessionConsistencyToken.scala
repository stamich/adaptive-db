package io.adb.consistency

/** What a session remembers to read its own writes: the position, in the engine's global commit
  * order, of the newest commit it made or observed.
  *
  * Data and DDL commits share one order, so "everything up to `commitSeq`" is well defined. A
  * token only moves forward: [[advancedTo]] and [[merge]] keep the larger number, so tokens of
  * several statements, connections or services can be combined without losing a write.
  *
  * @param commitSeq commit sequence number; 0 means "no commit yet", which every database satisfies
  */
final case class SessionConsistencyToken(commitSeq: Long) derives CanEqual:
  require(commitSeq >= 0, s"commit sequence must not be negative, got $commitSeq")

  /** The token after observing commit `seq`: the larger of the two positions. */
  def advancedTo(seq: Long): SessionConsistencyToken =
    if seq > commitSeq then SessionConsistencyToken(seq) else this

  /** The token that covers everything either token covers. */
  def merge(other: SessionConsistencyToken): SessionConsistencyToken = advancedTo(other.commitSeq)

  /** Whether a database that has published commits up to `head` already contains every write
    * this token stands for.
    */
  def isSatisfiedBy(head: Long): Boolean = commitSeq <= head

/** The starting token. */
object SessionConsistencyToken:
  /** A session that has written and observed nothing. */
  val Initial: SessionConsistencyToken = SessionConsistencyToken(0)
