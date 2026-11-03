package io.adb.gateway

import io.adb.actuator.{ActuationFailed, ActuationResult, Action, Actuator, Done}

/** The real [[Actuator]]: carries an action out through the same SQL entry point a user's DDL uses.
  *
  * - **Same path as a user.** The action's statement (`CREATE INDEX`, `ANALYZE`, `DROP INDEX`) is
  *   parsed, bound and executed exactly like a typed one, so the engine's own atomicity applies: a
  *   failed statement leaves the schema as it was, and the plan cache invalidates on the schema epoch.
  * - **Serialised with user DDL.** The statement runs while holding the database's schema-change
  *   lock, so it never overlaps a user's `CREATE`, `ALTER` or `DROP`.
  * - **Its own session.** The actuator's commits do not move the token of the user's default session.
  * - **Never throws for an engine failure.** The failure comes back as [[ActuationFailed]].
  *
  * It decides nothing: whether to act, and whether an index may be dropped, is the controller's
  * business (see [[io.adb.actuator.OwnershipRegistry]]).
  *
  * @param database the database to act on
  */
final class GatewayActuator(database: AdaptiveDatabase) extends Actuator:
  /** The actuator's own session. */
  private val session = database.session()

  /** Runs the action's SQL (`CREATE INDEX`, `ANALYZE` or `DROP INDEX`) through the actuator's own session. */
  def apply(action: Action): ActuationResult = run(action.sql)

  /** Runs the action's inverse statement (`DROP INDEX` for a created index) through the same session. */
  def undo(action: Action): ActuationResult =
    action.undoSql.fold[ActuationResult](ActuationFailed("no undo statement"))(run)

  /** Runs one statement under the schema-change lock and turns a failure into a result. */
  private def run(sql: String): ActuationResult =
    try
      database.withDdlLock(session.execute(sql))
      Done
    catch case e: Exception => ActuationFailed(Option(e.getMessage).getOrElse(e.getClass.getSimpleName))
