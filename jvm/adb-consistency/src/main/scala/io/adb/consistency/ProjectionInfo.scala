package io.adb.consistency

/** What a projection is. */
enum ProjectionKind derives CanEqual:
  /** The store of current rows. */
  case CurrentRow
  /** A secondary index. */
  case SecondaryIndex
  /** A consumer of the change log registered from outside (also used by tests). */
  case TestAsync

/** How a projection is kept up to date. */
enum ProjectionMode derives CanEqual:
  /** Updated inside the commit. */
  case Synchronous
  /** Updated by a consumer of the change log, at its own pace. */
  case Asynchronous

/** Lifecycle state of a projection. */
enum ProjectionState derives CanEqual:
  /** Being built; not readable. */
  case Building
  /** Readable up to its frontier. */
  case Ready
  /** The change log no longer holds what it needs. */
  case NeedsRebuild
  /** Stopped after an error. */
  case Failed

/** How far a projection has consumed the commit order.
  *
  * @param baseCommitSeq  last commit covered by the projection's own structure
  * @param deltaCommitSeq last commit covered with a delta overlay (equal to the base in 2.5.0)
  */
final case class ProjectionFrontier(baseCommitSeq: Long, deltaCommitSeq: Long) derives CanEqual:
  /** The last commit a read from the projection reflects. */
  def readableThrough: Long = math.max(baseCommitSeq, deltaCommitSeq)

/** A projection with its state and lag, as the engine reports it.
  *
  * @param id              stable id
  * @param name            unique name (`__current_row`, `index:<name>`, a consumer's name)
  * @param kind            what it is
  * @param mode            how it is maintained
  * @param entityId        entity it covers, if it covers one
  * @param epoch           version of the projection's definition
  * @param state           lifecycle state
  * @param frontier        consumed position
  * @param headCommitSeq   head of the commit order when the report was taken
  * @param lagCommits      commits between the frontier and the head
  * @param lagChanges      changes in that gap, when the consumer reports them
  * @param lagBytes        encoded change-log bytes in that gap, when reported
  * @param commitsPerSecond smoothed consumption speed; 0 when never observed
  */
final case class ProjectionInfo(
    id: Long,
    name: String,
    kind: ProjectionKind,
    mode: ProjectionMode,
    entityId: Option[Long],
    epoch: Long,
    state: ProjectionState,
    frontier: ProjectionFrontier,
    headCommitSeq: Long,
    lagCommits: Long,
    lagChanges: Long,
    lagBytes: Long,
    commitsPerSecond: Double
) derives CanEqual:
  /** Whether reads may be served from the projection at all. */
  def isReadable: Boolean = state == ProjectionState.Ready

  /** Whether the projection reflects every commit up to `seq`. */
  def covers(seq: Long): Boolean = isReadable && frontier.readableThrough >= seq

  /** Estimated milliseconds until the projection has consumed `seq`, from its observed rate;
    * `None` when the rate is unknown (or the projection is not progressing).
    */
  def estimatedMillisUntil(seq: Long): Option[Long] =
    val behind = seq - frontier.readableThrough
    if behind <= 0 then Some(0L)
    else if commitsPerSecond > 0 && commitsPerSecond.isFinite then Some(math.ceil(behind / commitsPerSecond * 1000.0).toLong)
    else None
