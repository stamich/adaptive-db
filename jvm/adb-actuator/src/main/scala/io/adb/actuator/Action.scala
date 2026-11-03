package io.adb.actuator

/** Identity of one action; numbered from 1 in the order the actions were proposed. */
final case class ActionId(value: Long) derives CanEqual:
  override def toString: String = value.toString

/** What an action does to the database. Only the three statements the engine already has. */
enum ActionKind derives CanEqual:
  /** `CREATE INDEX` (advisor recommendation [[io.adb.advisor.CreateIndex]]). */
  case CreateIndex
  /** `ANALYZE` (advisor recommendation [[io.adb.advisor.RunAnalyze]]). */
  case Analyze
  /** `DROP INDEX` of an index the advisor created: the undo of [[CreateIndex]], or the removal of an idle one. */
  case DropIndex

  /** Lower-case label for reports (`create_index`). */
  def label: String = this match
    case CreateIndex => "create_index"
    case Analyze => "analyze"
    case DropIndex => "drop_index"

/** What an action is about, so that cooldown and the flap guard can speak of "the same target".
  *
  * @param entityId entity concerned
  * @param key      stable text identifying the thing acted on: `index:<entity>:<field,...>` for an
  *                 index, `analyze:<entity>` for statistics (the advisor's own
  *                 [[io.adb.advisor.Recommendation.key]], so a recommendation and its action agree)
  */
final case class ActionTarget(entityId: Long, key: String) derives CanEqual

/** Where an action is in its life. See the lifecycle diagram of the 2.5.4 proposal.
  *
  * {{{
  *  Proposed --gate--> Rejected
  *  Proposed --gate--> Applied --verify--> Kept
  *                       |  \------------> RolledBack
  *                       \---(build fails)-> Failed
  * }}}
  */
enum ActionState derives CanEqual:
  /** Chosen by the controller; the gate has not decided yet. */
  case Proposed
  /** The gate refused it; nothing was changed. */
  case Rejected
  /** Carried out; being measured (an index) or done (an `ANALYZE`, which cannot be undone). */
  case Applied
  /** The statement failed; the schema is as before. */
  case Failed
  /** Verified (or needing no verification): it stays. */
  case Kept
  /** Verified and found not to pay, or idle: the advisor-owned index was dropped again. */
  case RolledBack

  /** Lower-case label for reports. */
  def label: String = this match
    case RolledBack => "rolled_back"
    case other => other.toString.toLowerCase

  /** Whether the action will not change state again. */
  def isFinal: Boolean = this match
    case Rejected | Failed | Kept | RolledBack => true
    case Proposed | Applied => false

/** One step of the advisor acting on the database.
  *
  * @param id             identity
  * @param kind           what it does
  * @param target         what it is about
  * @param entityName     table name, for texts and SQL
  * @param sql            the statement that carries it out
  * @param undoSql        the statement that takes it back, if it can be taken back (`DROP INDEX` for a created index)
  * @param indexName      for index actions, the index created or dropped
  * @param state          current state
  * @param reason         why it is in this state (rejection reason, failure message, verdict)
  * @param shapeIds       query shapes whose evidence justified it
  * @param proposedAtNanos clock reading when it was proposed
  * @param updatedAtNanos clock reading of the latest state change
  */
final case class Action(
    id: ActionId,
    kind: ActionKind,
    target: ActionTarget,
    entityName: String,
    sql: String,
    undoSql: Option[String],
    indexName: Option[String],
    state: ActionState,
    reason: String,
    shapeIds: Vector[Long],
    proposedAtNanos: Long,
    updatedAtNanos: Long
) derives CanEqual:
  /** The same action in `next` state, with the reason and time of the change. */
  def moved(next: ActionState, why: String, atNanos: Long): Action =
    copy(state = next, reason = why, updatedAtNanos = atNanos)

  /** One-line description for `\actions`. */
  def summary: String = s"${kind.label} ${indexName.getOrElse(entityName)}"
