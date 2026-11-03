package io.adb.actuator

/** The result of asking the engine to carry out one action. */
sealed trait ActuationResult derives CanEqual

/** The statement ran and the schema or statistics now reflect it. */
case object Done extends ActuationResult

/** The statement failed. The engine runs each of these statements atomically, so a failure leaves
  * the schema exactly as it was.
  *
  * @param message what the engine said
  */
final case class ActuationFailed(message: String) extends ActuationResult

/** The only way the controller changes the database.
  *
  * The interface is deliberately tiny: run the statement of an action, and take an action back.
  * Everything that decides **whether** to act lives elsewhere ([[PolicyGate]], the
  * [[AdaptiveController]]); an implementation only acts. The gateway provides the real one
  * (`GatewayActuator`); tests use a fake that records the calls.
  *
  * Implementations must not throw for an engine failure; they return [[ActuationFailed]]. They are
  * called from the controller's thread only, one call at a time.
  */
trait Actuator:
  /** Runs `action.sql` (`CREATE INDEX`, `ANALYZE` or `DROP INDEX`). */
  def apply(action: Action): ActuationResult

  /** Takes `action` back by running its `undoSql`; fails if the action has none. */
  def undo(action: Action): ActuationResult
