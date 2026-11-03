package io.adb.consistency

/** How fresh the data a query reads must be, relative to the commit order.
  *
  * The levels differ only in the commit a projection has to have reached before it may serve the
  * query (see [[SemanticReadContext.requiredCommitSeq]]); they say nothing about isolation, which
  * stays snapshot/serializable as before.
  */
sealed trait ReadConsistency derives CanEqual:
  /** The SQL spelling accepted by `SET SESSION CONSISTENCY`. */
  def sql: String

/** The read consistency levels. */
object ReadConsistency:
  /** Everything committed when the query starts is visible, including other sessions' writes. */
  case object StrongLatest extends ReadConsistency:
    /** @inheritdoc */
    def sql: String = "STRONG"

  /** The session sees all of its own writes (and everything it observed); other sessions' newer
    * commits may or may not be visible.
    */
  case object ReadYourWrites extends ReadConsistency:
    /** @inheritdoc */
    def sql: String = "READ_YOUR_WRITES"

  /** A projection may trail the head, as long as it stays within `bound`. */
  final case class BoundedStaleness(bound: StalenessBound) extends ReadConsistency:
    /** @inheritdoc */
    def sql: String = s"BOUNDED ${bound.sql}"

  /** Whatever a projection currently holds is acceptable; the query never waits. */
  case object Eventual extends ReadConsistency:
    /** @inheritdoc */
    def sql: String = "EVENTUAL"

  /** The session default: a session reads its own writes. */
  val Default: ReadConsistency = ReadYourWrites

/** How far behind the head a projection may be under [[ReadConsistency.BoundedStaleness]]. */
sealed trait StalenessBound derives CanEqual:
  /** The SQL spelling (`500 MS`, `10 COMMITS`). */
  def sql: String

/** The bounds. */
object StalenessBound:
  /** At most `value` milliseconds behind. A projection reports its lag in commits; the time is
    * estimated from the lag and the projection's observed consumption rate, and a projection
    * whose rate is unknown cannot prove a time bound unless it is fully caught up.
    */
  final case class Millis(value: Long) extends StalenessBound:
    require(value >= 0, "a staleness bound must not be negative")
    /** @inheritdoc */
    def sql: String = s"$value MS"

  /** At most `value` commits behind. */
  final case class Commits(value: Long) extends StalenessBound:
    require(value >= 0, "a staleness bound must not be negative")
    /** @inheritdoc */
    def sql: String = s"$value COMMITS"
