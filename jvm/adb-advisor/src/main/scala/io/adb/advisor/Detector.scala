package io.adb.advisor

import io.adb.telemetry.QueryShape

/** Thresholds of the detectors. Defaults are deliberately conservative: a finding should be
  * worth a person's attention.
  *
  * @param hotTimeShare           share of the workload's time that makes a shape "hot" (default 0.2)
  * @param scanAmplification      rows read per row returned from which a shape counts as wasteful (default 10)
  * @param qErrorThreshold        worst estimate error from which cardinality estimates count as wrong (default 10)
  * @param stalePendingChangeRatio changed rows per analyzed row from which statistics are stale (default 0.2)
  */
final case class DetectorConfig(
    hotTimeShare: Double = 0.2,
    scanAmplification: Double = 10.0,
    qErrorThreshold: Double = 10.0,
    stalePendingChangeRatio: Double = 0.2
) derives CanEqual

/** Everything a detector may read: immutable values only.
  *
  * @param workload the workload model snapshot
  * @param shapes   looks a shape up by id (the registry of the telemetry runtime)
  * @param design   the physical design now
  * @param model    scoring parameters
  * @param config   detector thresholds
  */
final case class DetectorContext(
    workload: WorkloadSnapshot,
    shapes: Long => Option[QueryShape],
    design: PhysicalDesign,
    model: CostBenefitModel,
    config: DetectorConfig
) derives CanEqual

/** One kind of finding. A detector is a pure function of its context: it holds no state, which
  * keeps hysteresis and the audit log in one place ([[Hysteresis]], [[AuditLog]]).
  */
trait Detector:
  /** Short name used in reports and the audit log. */
  def name: String

  /** The findings of this kind in `context`, unfiltered by hysteresis. */
  def detect(context: DetectorContext): Vector[Recommendation]
