package io.adb.advisor

/** What happened to a recommendation. */
enum AuditKind derives CanEqual:
  /** Found often enough in a row to be shown. */
  case Confirmed
  /** Stopped being found for long enough in a row; no longer shown. */
  case Withdrawn
  /** The advice was carried out (the index now exists); no longer shown. */
  case Applied

  /** Lower-case label for reports. */
  def label: String = toString.toLowerCase

/** One line of the audit log.
  *
  * @param sequence  1-based position in the log since it was created (never reused)
  * @param atNanos   monotonic clock reading when it happened
  * @param kind      what happened
  * @param key       the recommendation's [[Recommendation.key]]
  * @param summary   the recommendation's one-line description
  * @param sql       the statement that carries the advice out, if any
  */
final case class AuditEntry(sequence: Long, atNanos: Long, kind: AuditKind, key: String, summary: String, sql: Option[String]) derives CanEqual

/** A bounded record of every state change of a recommendation, so a person can see afterwards what
  * the advisor advised and when, even if it has withdrawn the advice since. The oldest entries are
  * dropped once `capacity` is reached. Thread-safe.
  *
  * @param capacity entries kept (at least 1)
  */
final class AuditLog(capacity: Int):
  require(capacity >= 1, "capacity must be at least 1")

  /** Entries, oldest first. */
  private var kept: Vector[AuditEntry] = Vector.empty
  /** Sequence number of the next entry. */
  private var next: Long = 1L

  /** Appends an entry for `recommendation`. */
  def record(kind: AuditKind, recommendation: Recommendation, atNanos: Long): AuditEntry = synchronized {
    val entry = AuditEntry(next, atNanos, kind, recommendation.key, recommendation.summary, recommendation.sql)
    next += 1
    kept = (kept :+ entry).takeRight(capacity)
    entry
  }

  /** The retained entries, oldest first. */
  def entries: Vector[AuditEntry] = synchronized(kept)

  /** Number of entries retained. */
  def size: Int = synchronized(kept.size)
