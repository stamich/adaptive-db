package io.adb.gateway

import io.adb.consistency.{ProjectionInfo, ProjectionJsonCodec}
import io.adb.ffm.{NativeDatabase, NativeWaitOutcome}

/** What a session needs from the engine to enforce a consistency level: the head of the commit
  * order, the projection reports, and a bounded wait for a projection.
  *
  * A trait so session logic can be tested without the native library.
  */
trait ConsistencyBackend:
  /** Newest published commit sequence number (0 before the first commit). */
  def latestCommitSeq(): Long

  /** Every projection with its state and lag. */
  def projections(): Vector[ProjectionInfo]

  /** Waits until projection `id` is ready and has consumed `commitSeq`.
    *
    * @param timeoutMillis longest wait, in milliseconds
    * @return whether the projection reached the commit in time
    */
  def awaitProjection(id: Long, commitSeq: Long, timeoutMillis: Long): Boolean

/** Construction from the native database. */
object ConsistencyBackend:
  /** Longest single wait the engine accepts (`adb_projection_wait_until`). */
  val MaxWaitMillis: Long = 60000L

  /** A backend reading `native`. */
  def apply(native: NativeDatabase): ConsistencyBackend = new ConsistencyBackend:
    /** @inheritdoc */
    def latestCommitSeq(): Long = native.latestCommitSeq()

    /** @inheritdoc */
    def projections(): Vector[ProjectionInfo] = ProjectionJsonCodec.decodeList(native.projectionsJson())

    /** @inheritdoc */
    def awaitProjection(id: Long, commitSeq: Long, timeoutMillis: Long): Boolean =
      native.waitForProjection(id, commitSeq, math.max(0L, math.min(timeoutMillis, MaxWaitMillis)).toInt) == NativeWaitOutcome.REACHED
