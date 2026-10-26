package io.adb.catalog

import io.adb.model.*

/** One key column of an index: a field and its sort direction. */
final case class IndexKeyColumn(field: FieldId, descending: Boolean) derives CanEqual

/** A secondary index as the engine reports it (`adb_list_indexes_json`).
  *
  * The engine owns index definitions (it maintains every index inside the commit), so the JVM
  * keeps no copy of its own; this is a read-only view.
  *
  * @param id           engine-assigned id
  * @param name         name (unique, case-insensitive)
  * @param entity       indexed entity
  * @param columns      key columns, most significant first
  * @param include      covered (`INCLUDE`) fields
  * @param createdAtTs  latest commit included by the build
  * @param usableFromTs oldest snapshot the index serves (creation or vacuum horizon)
  * @param watermarkTs  latest commit the index reflects
  */
final case class IndexInfo(
    id: IndexId,
    name: String,
    entity: EntityId,
    columns: Vector[IndexKeyColumn],
    include: Vector[FieldId],
    createdAtTs: Long,
    usableFromTs: Long,
    watermarkTs: Long
) derives CanEqual:
  /** Whether `field` is a key or covered column (a scan can read it without a row fetch). */
  def covers(field: FieldId): Boolean = columns.exists(_.field == field) || include.contains(field)

  /** Whether the index can answer a query at `snapshot`: `usableFromTs <= snapshot <= watermarkTs`.
    * Indexes are maintained inside the commit, so the latest snapshot is always served.
    */
  def serves(snapshot: Long): Boolean = usableFromTs <= snapshot && snapshot <= watermarkTs

/** Source of index definitions for binding and planning. Implementations must be cheap to call
  * repeatedly while one query is planned.
  */
trait IndexCatalog:
  /** Every index, in id order. */
  def indexes: Vector[IndexInfo]

  /** The indexes of `entity`, in id order. */
  def indexes(entity: EntityId): Vector[IndexInfo] = indexes.filter(_.entity == entity)

  /** The index named `name` (case-insensitive). */
  def index(name: String): Option[IndexInfo] = indexes.find(_.name.equalsIgnoreCase(name))

/** Index catalogs without the engine, and the decoder of the engine's listing. */
object IndexCatalog:
  /** No indexes at all. */
  val Empty: IndexCatalog = of()

  /** A fixed set of indexes (tests, benchmarks, what-if planning). */
  def of(infos: IndexInfo*): IndexCatalog =
    val sorted = infos.toVector.sortBy(_.id.value)
    new IndexCatalog:
      def indexes: Vector[IndexInfo] = sorted

  /** Decodes the engine's listing `{"indexes":[status, ...]}` (see `include/adb.h`).
    *
    * @throws IllegalArgumentException if the document is malformed
    */
  def decode(json: String): Vector[IndexInfo] =
    JsonReader.parse(json) match
      case root: Map[?, ?] =>
        root.asInstanceOf[Map[String, Any]].get("indexes") match
          case Some(items: Vector[?]) => items.map(item => decodeStatus(obj(item)))
          case other => throw new IllegalArgumentException(s"indexes must be an array, got $other")
      case other => throw new IllegalArgumentException(s"index listing must be an object, got $other")

  /** Decodes one index status object (also the result of `adb_create_index_json`). */
  def decodeStatus(status: Map[String, Any]): IndexInfo =
    val columns = array(status, "columns").map { raw =>
      val column = obj(raw)
      val direction = column.get("direction") match
        case Some("asc") | None => false
        case Some("desc") => true
        case other => throw new IllegalArgumentException(s"invalid index direction $other")
      IndexKeyColumn(FieldId(int(column, "field_id")), direction)
    }
    require(columns.nonEmpty, "an index has at least one key column")
    IndexInfo(
      id = IndexId(long(status, "id")),
      name = status.get("name") match
        case Some(name: String) if name.nonEmpty => name
        case other => throw new IllegalArgumentException(s"invalid index name $other"),
      entity = EntityId(long(status, "entity_id")),
      columns = columns,
      include = array(status, "include").map {
        case v: Long if v >= 0 && v <= Int.MaxValue => FieldId(v.toInt)
        case other => throw new IllegalArgumentException(s"invalid INCLUDE field $other")
      },
      createdAtTs = long(status, "created_at_ts"),
      usableFromTs = long(status, "usable_from_ts"),
      watermarkTs = long(status, "watermark_ts")
    )

  /** A JSON object. */
  private def obj(raw: Any): Map[String, Any] = raw match
    case map: Map[?, ?] => map.asInstanceOf[Map[String, Any]]
    case other => throw new IllegalArgumentException(s"expected an object, got $other")

  /** A required array field. */
  private def array(map: Map[String, Any], key: String): Vector[Any] = map.get(key) match
    case Some(values: Vector[?]) => values
    case other => throw new IllegalArgumentException(s"$key must be an array, got $other")

  /** A required non-negative integer field (`u64` values above `Long.MaxValue` keep their bits). */
  private def long(map: Map[String, Any], key: String): Long = map.get(key) match
    case Some(v: Long) if v >= 0 => v
    case Some(v: BigInt) if v >= 0 && v.bitLength <= 64 => v.toLong
    case other => throw new IllegalArgumentException(s"$key must be a non-negative integer, got $other")

  /** A required field id. */
  private def int(map: Map[String, Any], key: String): Int = long(map, key) match
    case v if v <= Int.MaxValue => v.toInt
    case v => throw new IllegalArgumentException(s"$key $v is out of range")
