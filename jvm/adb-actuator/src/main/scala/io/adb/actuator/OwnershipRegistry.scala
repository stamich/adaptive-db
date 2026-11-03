package io.adb.actuator

import io.adb.advisor.PhysicalDesign
import io.adb.telemetry.{FieldRef, QueryShape}

/** An index the advisor created and may therefore drop.
  *
  * @param name           index name (always `adv_*` when the advisor made it)
  * @param entityId       entity indexed
  * @param entityName     its name
  * @param fields         key columns by field id, in index order
  * @param target         the target key actions on this index use
  * @param createdBy      the action that built it, `None` for an index taken over after a restart
  * @param createdAtNanos clock reading when it was built or adopted
  * @param estimatedBytes estimated size, counted against the byte budget
  * @param adopted        whether it was found by its name rather than built by this process
  * @param baseline       the workload figures from before it was built, for verification; `None` when adopted
  * @param appliedAtNanos clock reading when the build finished, the start of the verification period
  */
final case class OwnedIndex(
    name: String,
    entityId: Long,
    entityName: String,
    fields: Vector[Long],
    target: ActionTarget,
    createdBy: Option[ActionId],
    createdAtNanos: Long,
    estimatedBytes: Long,
    adopted: Boolean = false,
    baseline: Option[Baseline] = None,
    appliedAtNanos: Long = 0L
) derives CanEqual

/** Helpers of [[OwnedIndex]]. */
object OwnedIndex:
  /** Whether `shape` filters on, or joins through, the leading column of `index`. */
  def wants(index: OwnedIndex, shape: QueryShape): Boolean =
    index.fields.headOption.exists { lead =>
      val ref = FieldRef(index.entityId, lead)
      shape.filterFields.contains(ref) ||
        shape.joinEdges.exists(e => (e.leftEntity == index.entityId && e.leftField == lead) || (e.rightEntity == index.entityId && e.rightField == lead))
    }

/** The record of what the advisor owns: the only indexes it may ever drop.
  *
  * A name prefix alone is not trusted; ownership is an entry here. The rules it enforces:
  * - [[mayDrop]] is true only for an index recorded with [[record]] (or taken over by [[adopt]]), so a
  *   user's index can never be the subject of a rollback;
  * - an owned index that disappears from the design was dropped by a person: [[reconcile]] removes
  *   it and remembers the target, so the advisor does not create it again.
  *
  * Thread-safe; the controller thread writes and the shell may read.
  */
final class OwnershipRegistry:
  /** Owned indexes by lower-case name. */
  private var byName: Map[String, OwnedIndex] = Map.empty
  /** Target keys of indexes a person dropped. */
  private var dropped: Set[String] = Set.empty

  /** Name prefix of every index the advisor creates (see [[io.adb.advisor.CreateIndex.indexName]]). */
  val Prefix: String = OwnershipRegistry.Prefix

  /** Records a freshly built index as owned. */
  def record(index: OwnedIndex): Unit = synchronized {
    byName = byName.updated(index.name.toLowerCase, index)
    dropped -= index.target.key
  }

  /** Every owned index, oldest first. */
  def owned: Vector[OwnedIndex] = synchronized(byName.valuesIterator.toVector.sortBy(_.createdAtNanos))

  /** The owned index called `name` (case-insensitive). */
  def get(name: String): Option[OwnedIndex] = synchronized(byName.get(name.toLowerCase))

  /** The owned index on `target`, if any. */
  def forTarget(target: ActionTarget): Option[OwnedIndex] = synchronized(byName.valuesIterator.find(_.target == target))

  /** Whether the advisor owns an index called `name`. This is the only test a drop may rely on. */
  def mayDrop(name: String): Boolean = synchronized(byName.contains(name.toLowerCase))

  /** Number of owned indexes. */
  def count: Int = synchronized(byName.size)

  /** Estimated bytes of all owned indexes. */
  def bytes: Long = synchronized(byName.valuesIterator.map(_.estimatedBytes).sum)

  /** Stops owning `name` after the advisor dropped it; returns what it owned. */
  def release(name: String): Option[OwnedIndex] = synchronized {
    val found = byName.get(name.toLowerCase)
    byName -= name.toLowerCase
    found
  }

  /** Whether a person dropped an advisor index on `targetKey`. */
  def externallyDropped(targetKey: String): Boolean = synchronized(dropped.contains(targetKey))

  /** Compares the registry with the physical design: every owned index that is no longer there was
    * dropped by a person. Those are removed from the registry and their targets remembered.
    *
    * @return the indexes found missing
    */
  def reconcile(design: PhysicalDesign): Vector[OwnedIndex] = synchronized {
    val present = design.entities.valuesIterator.flatMap(_.indexes.iterator.map(_.name.toLowerCase)).toSet
    val missing = byName.valuesIterator.filterNot(o => present.contains(o.name.toLowerCase)).toVector
    missing.foreach { o =>
      byName -= o.name.toLowerCase
      dropped += o.target.key
    }
    missing
  }

  /** Takes over the indexes named `adv_*` that the design holds and the registry does not know
    * (the advisor created them before a restart; the registry is in memory and starts empty).
    *
    * An adopted index has no baseline, so it is never verified, only subject to the idle rule. A
    * name is only a hint: the entry is marked `OwnedIndex.adopted` so `\actions` shows that
    * the advisor took it over by name. Indexes a person dropped earlier stay blocked.
    *
    * @param design    the physical design now
    * @param nowNanos  the controller's clock
    * @param budget    gives the size estimate for the byte budget
    * @return the indexes adopted
    */
  def adopt(design: PhysicalDesign, nowNanos: Long, budget: Budget): Vector[OwnedIndex] = synchronized {
    val found = for
      entity <- design.entities.valuesIterator.toVector.sortBy(_.entityId)
      index <- entity.indexes
      if index.name.regionMatches(true, 0, Prefix, 0, Prefix.length) && !byName.contains(index.name.toLowerCase)
    yield
      val fields = index.columns.map(_.field.value.toLong)
      OwnedIndex(
        index.name, entity.entityId, entity.name, fields,
        ActionTarget(entity.entityId, s"index:${entity.entityId}:${fields.mkString(",")}"),
        None, nowNanos, budget.estimateBytes(entity.rowCount), adopted = true)
    found.foreach(o => byName = byName.updated(o.name.toLowerCase, o))
    found
  }

  /** Forgets everything (tests, `reset`). */
  def clear(): Unit = synchronized {
    byName = Map.empty
    dropped = Set.empty
  }

/** Constants shared by the ownership rules (see [[OwnershipRegistry]]). */
object OwnershipRegistry:
  /** `adv_`: the reserved name prefix of advisor indexes. */
  val Prefix: String = "adv_"
