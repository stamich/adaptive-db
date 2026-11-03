package io.adb.telemetry

/** One join condition `left = right` between stored columns, with entity and field ids.
  *
  * @param leftEntity  entity of the left column
  * @param leftField   field id of the left column
  * @param rightEntity entity of the right column
  * @param rightField  field id of the right column
  */
final case class JoinEdge(leftEntity: Long, leftField: Long, rightEntity: Long, rightField: Long) derives CanEqual

/** The semantic shape of a SELECT: which entities, filters, joins and ordering it uses, without
  * its text and without any literal value.
  *
  * Two statements that differ only in layout, case or literals have the same shape, so the
  * workload model counts them together. A shape is computed **once, when the statement is
  * planned**, and kept next to the cached plan; executing a cached statement never rebuilds it.
  *
  * Fields are `(entityId, fieldId)` pairs flattened into [[FieldRef]]s so equal shapes compare
  * equal whatever the order in which the query mentions things (sets are kept sorted).
  *
  * @param entities     ids of the entities read, sorted
  * @param filterFields stored columns that appear in the WHERE predicate, sorted
  * @param joinEdges    equality joins between stored columns, normalised (smaller side first), sorted
  * @param orderFields  ORDER BY columns in order, each with its direction
  * @param hasAggregate whether the query groups or aggregates
  * @param hasLimit     whether the query has a LIMIT
  */
final case class QueryShape(
    entities: Vector[Long],
    filterFields: Vector[FieldRef],
    joinEdges: Vector[JoinEdge],
    orderFields: Vector[OrderRef],
    hasAggregate: Boolean,
    hasLimit: Boolean
) derives CanEqual:
  /** Stable 64-bit id of the shape (FNV-1a over its content); used as the key of the workload
    * model. Collisions are possible in principle (2^-64 per pair) and only merge two shapes.
    */
  lazy val id: Long =
    var h = QueryShape.FnvOffset
    def mix(v: Long): Unit =
      var i = 0
      while i < 8 do
        h = (h ^ ((v >>> (8 * i)) & 0xffL)) * QueryShape.FnvPrime
        i += 1
    entities.foreach(mix); mix(-1)
    filterFields.foreach { f => mix(f.entity); mix(f.field) }; mix(-2)
    joinEdges.foreach { e => mix(e.leftEntity); mix(e.leftField); mix(e.rightEntity); mix(e.rightField) }; mix(-3)
    orderFields.foreach { o => mix(o.field.entity); mix(o.field.field); mix(if o.descending then 1 else 0) }; mix(-4)
    mix(if hasAggregate then 1 else 0); mix(if hasLimit then 1 else 0)
    h

/** A stored column. */
final case class FieldRef(entity: Long, field: Long) derives CanEqual

/** An ORDER BY key. */
final case class OrderRef(field: FieldRef, descending: Boolean) derives CanEqual

/** Constants of [[QueryShape]]. */
object QueryShape:
  /** FNV-1a 64-bit offset basis. */
  private val FnvOffset: Long = 0xcbf29ce484222325L
  /** FNV-1a 64-bit prime. */
  private val FnvPrime: Long = 0x100000001b3L
