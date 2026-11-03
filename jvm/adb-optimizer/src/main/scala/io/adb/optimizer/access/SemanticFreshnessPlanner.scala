package io.adb.optimizer.access

import io.adb.catalog.IndexInfo
import io.adb.consistency.*
import io.adb.model.IndexId

/** An access path together with the freshness decision for the projection behind it.
  *
  * @param path       the candidate
  * @param projection the projection the path reads, when it reads one (`None` for a sequential
  *                   scan, which uses the canonical row path and is always fresh)
  * @param resolution what to do about that projection
  */
final case class FreshAccessPath(path: AccessPath, projection: Option[ProjectionInfo], resolution: FreshnessResolution)

/** The freshness decision for one index.
  *
  * @param index      the index
  * @param projection the engine's report for the index's projection, if it reported one
  * @param resolution what to do about it
  */
final case class IndexDecision(index: IndexInfo, projection: Option[ProjectionInfo], resolution: FreshnessResolution)

/** Applies a session's read context to the access paths of a relation.
  *
  * The enumerator knows which paths are possible; this planner knows which of them may be used
  * for the session's consistency level. A sequential scan always reads the canonical rows, so it
  * is always allowed and is the fallback for every projection-backed path.
  */
object SemanticFreshnessPlanner:
  /** Name under which the engine registers the projection of index `name`. */
  def indexProjectionName(name: String): String = s"index:$name"

  /** Decides, for every index, whether the session may read it.
    *
    * An index without a projection report is treated as a fresh synchronous index (2.4.0
    * behaviour).
    */
  def resolveIndexes(indexes: Vector[IndexInfo], projections: Vector[ProjectionInfo], context: SemanticReadContext): Vector[IndexDecision] =
    val byName = projections.map(p => p.name -> p).toMap
    indexes.map { index =>
      byName.get(indexProjectionName(index.name)) match
        case Some(info) => IndexDecision(index, Some(info), FreshnessResolver.resolve(context, info, fallbackAvailable = true))
        case None => IndexDecision(index, None, FreshnessResolution.Direct)
    }

  /** Whether a decision lets the planner use the index. */
  def isUsable(resolution: FreshnessResolution): Boolean = resolution match
    case FreshnessResolution.Fallback(_) | FreshnessResolution.Infeasible(_) => false
    case _ => true

  /** Annotates each candidate with a freshness decision.
    *
    * An index scan whose projection is unknown (not reported by the engine) is treated as
    * synchronous and fresh, as in 2.4.0, so planning against an engine that reports no
    * projections behaves as before.
    *
    * @param candidates  paths from [[AccessPathEnumerator.candidates]]
    * @param projections the engine's projection report
    * @param context     the session's read context
    */
  def annotate(
      candidates: Vector[AccessPath],
      projections: Vector[ProjectionInfo],
      context: SemanticReadContext
  ): Vector[FreshAccessPath] =
    val byName = projections.map(p => p.name -> p).toMap
    candidates.map {
      case scan: AccessPath.IndexScan =>
        byName.get(indexProjectionName(scan.index.name)) match
          case Some(info) =>
            FreshAccessPath(scan, Some(info), FreshnessResolver.resolve(context, info, fallbackAvailable = true))
          case None => FreshAccessPath(scan, None, FreshnessResolution.Direct)
      case other => FreshAccessPath(other, None, FreshnessResolution.Direct)
    }

  /** The paths the planner may cost: everything the resolver did not turn away.
    *
    * `Direct`, `AllowStale` and `WaitUntil` paths stay; `Fallback` and `Infeasible` paths are
    * dropped (the sequential scan remains). The result always contains a sequential scan if the
    * input did.
    */
  def usable(annotated: Vector[FreshAccessPath]): Vector[FreshAccessPath] =
    annotated.filter(path => isUsable(path.resolution))

  /** The waits the indexes in `used` need before the plan can run: for each such index whose
    * decision is `WaitUntil`, its projection id and the commit to wait for.
    *
    * @param allowed the decisions of the indexes the plan was made with
    * @param used    ids of the indexes the plan reads
    */
  def waitsFor(allowed: Vector[IndexDecision], used: Set[IndexId]): Vector[(IndexDecision, Long, Long)] =
    allowed.filter(d => used.contains(d.index.id)).flatMap { d =>
      (d.projection, d.resolution) match
        case (Some(info), FreshnessResolution.WaitUntil(seq, _)) => Some((d, info.id, seq))
        case _ => None
    }

  /** The projections a chosen path must wait for before it runs, as (projection id, commit). */
  def waits(chosen: FreshAccessPath): Option[(Long, Long)] =
    (chosen.projection, chosen.resolution) match
      case (Some(info), FreshnessResolution.WaitUntil(seq, _)) => Some((info.id, seq))
      case _ => None
