package io.adb.actuator

/** How much the database may do on its own (`SET adaptive = off | advisory | auto_safe`).
  *
  * The default is [[Off]]; nothing acts unless a person sets [[AutoSafe]].
  */
enum AdaptiveMode derives CanEqual:
  /** No telemetry-driven work: the advisor is not evaluated. */
  case Off
  /** The 2.5.3 advisor: it evaluates and recommends, and never acts. */
  case Advisory
  /** The advisor evaluates, and the controller carries out safe actions inside the budget,
    * verifies them and undoes what does not pay.
    */
  case AutoSafe

  /** The SQL spelling (`auto_safe`). */
  def sql: String = this match
    case Off => "off"
    case Advisory => "advisory"
    case AutoSafe => "auto_safe"

object AdaptiveMode:
  /** The mode spelled `name` (case-insensitive), if there is one. */
  def parse(name: String): Option[AdaptiveMode] = values.find(_.sql.equalsIgnoreCase(name))

/** All knobs of the controller.
  *
  * @param policy      thresholds
  * @param budget      limits
  * @param logCapacity entries kept in the action log
  * @param maxActions  actions kept for `\\actions` (older final ones are dropped)
  */
final case class AdaptiveConfig(
    policy: Policy = Policy(),
    budget: Budget = Budget(),
    logCapacity: Int = 200,
    maxActions: Int = 200
) derives CanEqual
