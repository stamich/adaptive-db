package io.adb.consistency

import io.adb.model.JsonReader

/** Decodes the engine's projection documents (`adb_projection_list_json`,
  * `adb_projection_get_json`; see `include/adb.h`).
  */
object ProjectionJsonCodec:
  /** Decodes a listing: a JSON array of progress documents.
    *
    * @throws IllegalArgumentException if the document is malformed
    */
  def decodeList(json: String): Vector[ProjectionInfo] =
    JsonReader.parse(json) match
      case items: Vector[?] => items.map(item => decodeObject(obj(item, "projection")))
      case other => throw new IllegalArgumentException(s"projection listing must be an array, got $other")

  /** Decodes one progress document.
    *
    * @throws IllegalArgumentException if the document is malformed
    */
  def decode(json: String): ProjectionInfo = decodeObject(obj(JsonReader.parse(json), "projection"))

  /** Decodes a parsed progress object. */
  private def decodeObject(root: Map[String, Any]): ProjectionInfo =
    val descriptor = obj(root.getOrElse("descriptor", missing("descriptor")), "descriptor")
    val frontier = obj(root.getOrElse("frontier", missing("frontier")), "frontier")
    ProjectionInfo(
      id = long(descriptor, "id"),
      name = descriptor.get("name") match
        case Some(name: String) if name.nonEmpty => name
        case other => throw new IllegalArgumentException(s"invalid projection name $other"),
      kind = enumValue(descriptor, "kind", ProjectionKind.valueOf),
      mode = enumValue(descriptor, "mode", ProjectionMode.valueOf),
      entityId = descriptor.get("entity_id") match
        case None | Some(null) => None
        case Some(_) => Some(long(descriptor, "entity_id")),
      epoch = long(descriptor, "epoch"),
      state = enumValue(root, "state", ProjectionState.valueOf),
      frontier = ProjectionFrontier(long(frontier, "base_commit_seq"), long(frontier, "delta_commit_seq")),
      headCommitSeq = long(root, "head_commit_seq"),
      lagCommits = long(root, "lag_commits"),
      lagChanges = long(root, "lag_changes"),
      lagBytes = long(root, "lag_bytes"),
      commitsPerSecond = double(root, "commits_per_second")
    )

  /** A JSON object. */
  private def obj(raw: Any, what: String): Map[String, Any] = raw match
    case map: Map[?, ?] => map.asInstanceOf[Map[String, Any]]
    case other => throw new IllegalArgumentException(s"$what must be an object, got $other")

  /** The error for a missing member. */
  private def missing(key: String): Nothing = throw new IllegalArgumentException(s"missing $key")

  /** A required non-negative integer member. */
  private def long(map: Map[String, Any], key: String): Long = map.get(key) match
    case Some(v: Long) if v >= 0 => v
    case other => throw new IllegalArgumentException(s"$key must be a non-negative integer, got $other")

  /** A required finite, non-negative number member (an integer literal is accepted). */
  private def double(map: Map[String, Any], key: String): Double = map.get(key) match
    case Some(v: Double) if v.isFinite && v >= 0 => v
    case Some(v: Long) if v >= 0 => v.toDouble
    case other => throw new IllegalArgumentException(s"$key must be a finite non-negative number, got $other")

  /** A required member naming one of the constants of an enum. */
  private def enumValue[T](map: Map[String, Any], key: String, parse: String => T): T = map.get(key) match
    case Some(name: String) =>
      try parse(name)
      catch case _: IllegalArgumentException => throw new IllegalArgumentException(s"unknown $key $name")
    case other => throw new IllegalArgumentException(s"$key must be a string, got $other")
