package io.adb.actuator

import io.adb.advisor.PhysicalDesign

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
    adopted: Boolean = false
) derives CanEqual

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

  /** Forgets everything (tests, `reset`). */
  def clear(): Unit = synchronized {
    byName = Map.empty
    dropped = Set.empty
  }

object OwnershipRegistry:
  /** `adv_`: the reserved name prefix of advisor indexes. */
  val Prefix: String = "adv_"
