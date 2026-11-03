package io.adb.telemetry

/** Where the gateway puts the telemetry of finished queries.
  *
  * The contract is the 2.5.2 hot-path rule: both methods are cheap and **never block**.
  */
trait TelemetrySink:
  /** Whether telemetry is on at all. When false the caller must do no telemetry work. */
  def enabled: Boolean

  /** Offers an event; false means it was dropped (queue full) and the query was not delayed. */
  def offer(event: QueryTelemetry): Boolean
