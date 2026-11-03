package io.adb.telemetry

import java.util.concurrent.ConcurrentHashMap

/** Bounded map from shape id to [[QueryShape]], filled when a shape is first captured.
  *
  * The workload model resolves the `shapeId` of an event here. When the registry is full a new
  * shape is simply not registered (its events are still counted by id), so memory stays bounded
  * even under a flood of distinct shapes.
  *
  * @param maxShapes upper bound on registered shapes
  */
final class QueryShapeRegistry(maxShapes: Int):
  require(maxShapes > 0, "maxShapes must be positive")
  /** Registered shapes by id. */
  private val shapes = new ConcurrentHashMap[Long, QueryShape]()

  /** Registers `shape` if there is room; true if it is (now) registered. */
  def register(shape: QueryShape): Boolean =
    if shapes.containsKey(shape.id) then true
    else if shapes.size >= maxShapes then false
    else
      shapes.putIfAbsent(shape.id, shape)
      true

  /** Whether `shapeId` is registered (the sampler's "known shape" test). */
  def contains(shapeId: Long): Boolean = shapes.containsKey(shapeId)

  /** The shape registered under `shapeId`. */
  def get(shapeId: Long): Option[QueryShape] = Option(shapes.get(shapeId))

  /** Number of registered shapes. */
  def size: Int = shapes.size

  /** Forgets every shape. */
  def clear(): Unit = shapes.clear()
