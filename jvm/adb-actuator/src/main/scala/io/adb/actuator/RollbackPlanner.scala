package io.adb.actuator

/** Turns "this advisor index should go" into a `DROP INDEX` action, and only for an index the
  * advisor owns.
  *
  * The ownership test is made twice on purpose: here, when the action is planned, and again by
  * [[OwnedOnlyActuator]] just before the statement runs. A bug in one place cannot drop a user's index.
  */
object RollbackPlanner:
  /** Plans the drop of `indexName`.
    *
    * @param registry  what the advisor owns
    * @param indexName index to drop
    * @param why       the reason, shown in `\\actions` and the action log
    * @param id        identity for the new action
    * @param nowNanos  clock reading
    * @return the action, or why none can be made (the index is not advisor-owned)
    */
  def plan(registry: OwnershipRegistry, indexName: String, why: String, id: ActionId, nowNanos: Long): Either[String, Action] =
    registry.get(indexName) match
      case None => Left(s"$indexName is not owned by the advisor, so the advisor will not drop it")
      case Some(owned) =>
        Right(Action(
          id, ActionKind.DropIndex, owned.target, owned.entityName,
          s"DROP INDEX ${owned.name}", None, Some(owned.name),
          ActionState.Proposed, why, Vector.empty, nowNanos, nowNanos))

/** An [[Actuator]] that refuses to drop anything the advisor does not own.
  *
  * It wraps the real actuator. A `DROP INDEX` action, or the undo of a `CREATE INDEX`, runs only if the
  * index is in the [[OwnershipRegistry]]; otherwise the call fails without reaching the engine. This is
  * the last line of the safety rule "never drops a user's index".
  *
  * @param inner    the actuator that really acts
  * @param registry what the advisor owns
  */
final class OwnedOnlyActuator(inner: Actuator, registry: OwnershipRegistry) extends Actuator:
  def apply(action: Action): ActuationResult =
    if action.kind == ActionKind.DropIndex && !owns(action) then refusal(action) else inner(action)

  def undo(action: Action): ActuationResult =
    if !owns(action) then refusal(action) else inner.undo(action)

  /** Whether the action's index is advisor-owned. An action without an index name is never owned. */
  private def owns(action: Action): Boolean = action.indexName.exists(registry.mayDrop)

  private def refusal(action: Action): ActuationResult =
    ActuationFailed(s"${action.indexName.getOrElse(action.entityName)} is not owned by the advisor")
