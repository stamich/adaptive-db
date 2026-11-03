package io.adb.actuator

import scala.collection.mutable

/** An [[Actuator]] for tests: it runs nothing, records every call, and can be told to fail.
  *
  * `onApply` and `onUndo` are hooks so a test can make the pretend database change (for example
  * add an index to the design the next evaluation sees).
  */
final class FakeActuator extends Actuator:
  /** Actions passed to `apply`, in call order. */
  val applied: mutable.ArrayBuffer[Action] = mutable.ArrayBuffer.empty
  /** Actions passed to `undo`, in call order. */
  val undone: mutable.ArrayBuffer[Action] = mutable.ArrayBuffer.empty
  /** When set, the next `apply` fails with this message (then it is cleared). */
  var failNextApply: Option[String] = None
  /** When set, the next `undo` fails with this message (then it is cleared). */
  var failNextUndo: Option[String] = None
  /** Called after a successful `apply`. */
  var onApply: Action => Unit = _ => ()
  /** Called after a successful `undo`. */
  var onUndo: Action => Unit = _ => ()

  def apply(action: Action): ActuationResult =
    applied += action
    failNextApply match
      case Some(message) => failNextApply = None; ActuationFailed(message)
      case None => onApply(action); Done

  def undo(action: Action): ActuationResult =
    undone += action
    if action.undoSql.isEmpty then ActuationFailed("no undo statement")
    else failNextUndo match
      case Some(message) => failNextUndo = None; ActuationFailed(message)
      case None => onUndo(action); Done
