package io.adb.consistency

/** What to do about one projection that might serve a query. */
sealed trait FreshnessResolution derives CanEqual

/** The possible decisions. */
object FreshnessResolution:
  /** The projection already satisfies the read; use it. */
  case object Direct extends FreshnessResolution

  /** Wait until the projection has consumed `commitSeq`, then use it.
    *
    * @param commitSeq       commit to wait for
    * @param estimatedMillis estimated wait from the projection's rate, if known
    */
  final case class WaitUntil(commitSeq: Long, estimatedMillis: Option[Long]) extends FreshnessResolution

  /** Do not use the projection; read the canonical row path instead. */
  final case class Fallback(reason: String) extends FreshnessResolution

  /** The projection is behind but the level tolerates it.
    *
    * @param lagCommits commits the projection trails the head by
    */
  final case class AllowStale(lagCommits: Long) extends FreshnessResolution

  /** The read cannot be satisfied: no projection can serve it and no fallback exists, or the
    * session's token is beyond what the database holds.
    */
  final case class Infeasible(reason: String) extends FreshnessResolution

/** Decides whether, and how, a projection may serve a read at a given consistency level.
  *
  * The decision is a pure function of the read context and the projection's report, so it can be
  * explained (`EXPLAIN`) and tested without a database.
  */
object FreshnessResolver:
  import FreshnessResolution.*

  /** Resolves one projection.
    *
    * Rules, in order:
    *  1. A projection that is not `Ready` is never read; the query falls back if it can.
    *  2. A session token beyond the head is infeasible: waiting cannot make it true.
    *  3. Eventual reads accept the projection as it is.
    *  4. Bounded reads accept it within the bound; beyond it they fall back, or wait.
    *  5. Strong and read-your-writes reads need the projection at the required commit: direct
    *     when it is there, a wait when the estimated time fits the context's wait limit,
    *     otherwise a fallback (or, with no fallback, a wait of unknown length, or infeasible).
    *
    * @param context           the session's read context
    * @param projection        the projection's report
    * @param fallbackAvailable whether the canonical row path can answer the query instead
    */
  def resolve(
      context: SemanticReadContext,
      projection: ProjectionInfo,
      fallbackAvailable: Boolean
  ): FreshnessResolution =
    def unusable(reason: String): FreshnessResolution =
      if fallbackAvailable then Fallback(reason) else Infeasible(reason)

    val readable = projection.frontier.readableThrough
    val lag = math.max(0L, context.headCommitSeq - readable)

    if !projection.isReadable then unusable(s"projection ${projection.name} is ${projection.state}")
    else if context.consistency != ReadConsistency.Eventual && context.tokenIsAhead then
      Infeasible(s"session token ${context.token.commitSeq} is beyond the head ${context.headCommitSeq}")
    else
      context.consistency match
        case ReadConsistency.Eventual =>
          if lag == 0 then Direct else AllowStale(lag)
        case ReadConsistency.BoundedStaleness(bound) =>
          if lag == 0 then Direct
          else if withinBound(bound, lag, projection, context.headCommitSeq) then AllowStale(lag)
          else catchUp(context, projection, context.headCommitSeq, fallbackAvailable, "staleness bound exceeded")
        case ReadConsistency.StrongLatest | ReadConsistency.ReadYourWrites =>
          val required = context.requiredCommitSeq.getOrElse(0L)
          if projection.covers(required) then Direct
          else catchUp(context, projection, required, fallbackAvailable, s"projection is behind commit $required")

  /** Whether a lag of `lag` commits is inside the bound. A time bound needs a known rate. */
  private def withinBound(bound: StalenessBound, lag: Long, projection: ProjectionInfo, head: Long): Boolean =
    bound match
      case StalenessBound.Commits(n) => lag <= n
      case StalenessBound.Millis(ms) => projection.estimatedMillisUntil(head).exists(_ <= ms)

  /** The projection is behind what the read needs: wait if that is quick enough, else fall back. */
  private def catchUp(
      context: SemanticReadContext,
      projection: ProjectionInfo,
      required: Long,
      fallbackAvailable: Boolean,
      reason: String
  ): FreshnessResolution =
    projection.estimatedMillisUntil(required) match
      case Some(ms) if ms <= context.maxWaitMillis => FreshnessResolution.WaitUntil(required, Some(ms))
      case Some(_) | None if fallbackAvailable => FreshnessResolution.Fallback(reason)
      case Some(ms) => FreshnessResolution.Infeasible(s"$reason; catching up needs about $ms ms")
      case None => FreshnessResolution.WaitUntil(required, None)
