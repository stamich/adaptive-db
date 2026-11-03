package io.adb.actuator

/** What happened, as the action log records it. */
enum ActionEvent derives CanEqual:
  /** The statement ran. */
  case Applied
  /** Verified, or needing no verification: the action stays. */
  case Kept
  /** Taken back (did not pay, or idle). */
  case RolledBack
  /** The policy gate refused a recommendation. */
  case Rejected
  /** The statement failed. */
  case Failed
  /** An index the advisor owned was dropped by a person. */
  case ExternallyDropped
  /** An `adv_*` index was taken over after a restart. */
  case Adopted
  /** The verification ended without enough evidence either way; the index stays under the idle rule. */
  case Unverified

  /** Lower-case label for reports (`rolled_back`). */
  def label: String = toString.flatMap(c => if c.isUpper then s"_${c.toLower}" else c.toString).drop(1)

/** One line of the action log.
  *
  * @param sequence 1-based position since the log was created (never reused)
  * @param atNanos  clock reading
  * @param event    what happened
  * @param actionId the action it concerns, if there is one (a rejection has none)
  * @param action   one-line description of what the action is about (`create_index adv_orders_customer_id`)
  * @param detail   the reason or the figures
  */
final case class ActionLogEntry(
    sequence: Long,
    atNanos: Long,
    event: ActionEvent,
    actionId: Option[ActionId],
    action: String,
    detail: String
) derives CanEqual

/** A bounded, sequence-numbered record of everything the actuator did and refused, next to the
  * advisor's [[io.adb.advisor.AuditLog]] (which records what was advised). The oldest entries are
  * dropped once `capacity` is reached; sequence numbers are never reused, so a reader can tell that
  * entries were dropped. Thread-safe.
  *
  * @param capacity entries kept (at least 1)
  */
final class ActionLog(capacity: Int):
  require(capacity >= 1, "capacity must be at least 1")

  /** Entries, oldest first. */
  private var kept: Vector[ActionLogEntry] = Vector.empty
  /** Sequence number of the next entry. */
  private var next: Long = 1L

  /** Appends an entry. */
  def record(event: ActionEvent, atNanos: Long, actionId: Option[ActionId], action: String, detail: String): ActionLogEntry = synchronized {
    val entry = ActionLogEntry(next, atNanos, event, actionId, action, detail)
    next += 1
    kept = (kept :+ entry).takeRight(capacity)
    entry
  }

  /** The retained entries, oldest first. */
  def entries: Vector[ActionLogEntry] = synchronized(kept)

  /** Number of entries retained. */
  def size: Int = synchronized(kept.size)
