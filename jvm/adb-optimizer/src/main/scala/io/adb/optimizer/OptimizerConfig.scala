package io.adb.optimizer

import io.adb.optimizer.cardinality.EstimationConfig
import io.adb.optimizer.cost.CostWeights

/** Resource limits of the native engine (`ExecutionLimits::default()` in Rust). The planner
  * checks its estimates against them so it avoids plans the engine would abort.
  *
  * @param queryMemoryBytes         bytes a query may hold in blocking operators
  * @param maxMaterializedRows      rows a blocking operator may materialize
  * @param maxJoinFanout            matches a single left row may produce
  * @param maxNestedLoopComparisons predicate evaluations of one nested-loop join
  */
final case class EngineLimits(
    queryMemoryBytes: Long = 256L * 1024 * 1024,
    maxMaterializedRows: Long = 1_000_000,
    maxJoinFanout: Long = 16_384,
    maxNestedLoopComparisons: Long = 10_000_000
) derives CanEqual

/** Configuration of the cost-based optimizer.
  *
  * @param estimation         cardinality defaults
  * @param weights            how cost components add up
  * @param limits             engine limits the plan must respect
  * @param maxDpRelations     inner-join blocks up to this size are ordered exactly (dynamic
  *                           programming over subsets, `3^n` splits)
  * @param maxGreedyRelations larger blocks up to this size are ordered greedily; beyond it the
  *                           SQL order is kept
  */
final case class OptimizerConfig(
    estimation: EstimationConfig = EstimationConfig(),
    weights: CostWeights = CostWeights(),
    limits: EngineLimits = EngineLimits(),
    maxDpRelations: Int = 10,
    maxGreedyRelations: Int = 32
) derives CanEqual:
  require(maxDpRelations >= 2 && maxDpRelations <= 16, "maxDpRelations must be 2..16")
  require(maxGreedyRelations >= maxDpRelations && maxGreedyRelations <= 64, "maxGreedyRelations must be maxDpRelations..64")

/** Default configuration. */
object OptimizerConfig:
  /** The 2.2.3 defaults. */
  val Default: OptimizerConfig = OptimizerConfig()
