package io.adb.catalog

import io.adb.model.*
import java.nio.channels.FileChannel
import java.nio.file.{Files, Path, StandardCopyOption, StandardOpenOption}
import java.util.Properties
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
import scala.jdk.CollectionConverters.*

/**
 * Durable file-backed catalog for Milestone 2 schema metadata.
 *
 * The hardened implementation bounds the input file and entity/field counts, validates restored
 * catalog shape, and publishes updates using file fsync, atomic rename and directory fsync.
 */
final class FileCatalog(path: Path) extends Catalog:
  /** In-memory state; every mutation is persisted before it is acknowledged. */
  private val delegate = new InMemoryCatalog
  load()

  /** Returns the current schema version. */
  override def version: SchemaVersion = delegate.version
  /** Looks up an entity case-insensitively by name. */
  override def entity(name: String): Option[Entity] = delegate.entity(name)
  /** Looks up an entity by stable identifier. */
  override def entity(id: EntityId): Option[Entity] = delegate.entity(id)
  /** Returns all entities ordered by identifier. */
  override def entities: Vector[Entity] = delegate.entities

  /** Creates one entity and durably publishes the updated catalog before returning. */
  override def createEntity(
      name: String,
      fields: Vector[(String, DataType, Boolean)],
      primaryKey: String
  ): Entity = synchronized {
    val entity = delegate.createEntity(name, fields, primaryKey)
    try
      save()
      entity
    catch
      case error: Throwable =>
        loadFreshAfterFailedMutation()
        throw error
  }

  /** Reloads the last durable state after a failed catalog publication. */
  private def loadFreshAfterFailedMutation(): Unit =
    if !Files.exists(path) then delegate.restore(0L, 1L, Vector.empty)
    else load()

  /** Loads and validates the bounded catalog file into the in-memory index. */
  private def load(): Unit = synchronized {
    if !Files.exists(path) then return
    val size = Files.size(path)
    if size < 0 || size > FileCatalog.MaxCatalogBytes then
      throw new IllegalStateException(s"catalog size $size exceeds ${FileCatalog.MaxCatalogBytes} bytes")

    val props = new Properties
    val in = Files.newInputStream(path)
    try props.load(in) finally in.close()

    Option(props.getProperty("catalog.checksum")).foreach { expected =>
      val actual = checksum(props)
      if !MessageDigest.isEqual(expected.getBytes(StandardCharsets.US_ASCII), actual.getBytes(StandardCharsets.US_ASCII)) then
        throw new IllegalStateException("catalog checksum mismatch")
    }

    val version = parseNonNegativeLong(props, "catalog.version", "0")
    val nextEntityId = parsePositiveLong(props, "catalog.nextEntityId", "1")
    val count = parseBoundedInt(props, "entity.count", "0", FileCatalog.MaxEntities)

    val restored = (0 until count).toVector.map { i =>
      val prefix = s"entity.$i"
      val id = EntityId(parsePositiveLong(props, s"$prefix.id"))
      val name = required(props, s"$prefix.name")
      requireName(name, "entity")
      val schemaVersion = SchemaVersion(parseNonNegativeLong(props, s"$prefix.schemaVersion"))
      val pk = FieldId(parsePositiveInt(props, s"$prefix.primaryKey"))
      val fieldCount = parseBoundedInt(props, s"$prefix.field.count", null, FileCatalog.MaxFieldsPerEntity)
      if fieldCount == 0 then throw new IllegalStateException(s"$prefix has no fields")
      val fields = (0 until fieldCount).toVector.map { f =>
        val fp = s"$prefix.field.$f"
        val fieldName = required(props, s"$fp.name")
        requireName(fieldName, "field")
        val dataType = DataType.parse(required(props, s"$fp.type"))
          .getOrElse(throw new IllegalStateException(s"unsupported type in $fp"))
        Field(
          FieldId(parsePositiveInt(props, s"$fp.id")),
          fieldName,
          dataType,
          parseBoolean(props, s"$fp.nullable")
        )
      }
      Entity(id, name, fields, pk, schemaVersion)
    }

    validateRestored(version, nextEntityId, restored)
    delegate.restore(version, nextEntityId, restored)
  }

  /** Serializes and crash-safely publishes the current catalog snapshot. */
  private def save(): Unit = synchronized {
    Option(path.getParent).foreach(parent => Files.createDirectories(parent))
    val props = new Properties
    props.setProperty("catalog.version", delegate.version.value.toString)
    props.setProperty("catalog.nextEntityId", (delegate.entities.map(_.id.value).maxOption.getOrElse(0L) + 1L).toString)
    props.setProperty("entity.count", delegate.entities.size.toString)

    delegate.entities.zipWithIndex.foreach { case (entity, i) =>
      val prefix = s"entity.$i"
      props.setProperty(s"$prefix.id", entity.id.value.toString)
      props.setProperty(s"$prefix.name", entity.name)
      props.setProperty(s"$prefix.schemaVersion", entity.schemaVersion.value.toString)
      props.setProperty(s"$prefix.primaryKey", entity.primaryKey.value.toString)
      props.setProperty(s"$prefix.field.count", entity.fields.size.toString)
      entity.fields.zipWithIndex.foreach { case (field, f) =>
        val fp = s"$prefix.field.$f"
        props.setProperty(s"$fp.id", field.id.value.toString)
        props.setProperty(s"$fp.name", field.name)
        props.setProperty(s"$fp.type", field.dataType.sqlName)
        props.setProperty(s"$fp.nullable", field.nullable.toString)
      }
    }

    props.setProperty("catalog.checksum", checksum(props))

    val tmp = path.resolveSibling(path.getFileName.toString + ".tmp")
    val channel = FileChannel.open(tmp, StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING, StandardOpenOption.WRITE)
    try
      val out = java.nio.channels.Channels.newOutputStream(channel)
      props.store(out, "Adaptive DB catalog v1 hardened")
      out.flush()
      channel.force(true)
    finally channel.close()

    Files.move(tmp, path, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
    Option(path.getParent).foreach { parent =>
      val dir = FileChannel.open(parent, StandardOpenOption.READ)
      try dir.force(true) finally dir.close()
    }
  }

  /** Computes a deterministic SHA-256 digest over all catalog properties except the digest itself. */
  private def checksum(props: Properties): String =
    val digest = MessageDigest.getInstance("SHA-256")
    props.stringPropertyNames().asScala.filterNot(_ == "catalog.checksum").toVector.sorted.foreach { key =>
      digest.update(key.getBytes(StandardCharsets.UTF_8))
      digest.update(0.toByte)
      digest.update(props.getProperty(key).getBytes(StandardCharsets.UTF_8))
      digest.update(10.toByte)
    }
    digest.digest().map(b => f"${b & 0xff}%02x").mkString

  /** Returns a required property or reports catalog corruption. */
  private def required(props: Properties, key: String): String =
    Option(props.getProperty(key)).getOrElse(throw new IllegalStateException(s"missing catalog property $key"))

  /** Parses a bounded non-negative integer property. */
  private def parseBoundedInt(props: Properties, key: String, default: String, max: Int): Int =
    val raw = Option(props.getProperty(key)).orElse(Option(default)).getOrElse(throw new IllegalStateException(s"missing catalog property $key"))
    val value = raw.toIntOption.getOrElse(throw new IllegalStateException(s"invalid integer property $key"))
    if value < 0 || value > max then throw new IllegalStateException(s"$key=$value outside 0..$max")
    value

  /** Parses a strictly positive integer property. */
  private def parsePositiveInt(props: Properties, key: String): Int =
    val value = required(props, key).toIntOption.getOrElse(throw new IllegalStateException(s"invalid integer property $key"))
    if value <= 0 then throw new IllegalStateException(s"$key must be positive")
    value

  /** Parses a non-negative long property. */
  private def parseNonNegativeLong(props: Properties, key: String, default: String = null): Long =
    val raw = Option(props.getProperty(key)).orElse(Option(default)).getOrElse(throw new IllegalStateException(s"missing catalog property $key"))
    val value = raw.toLongOption.getOrElse(throw new IllegalStateException(s"invalid long property $key"))
    if value < 0 then throw new IllegalStateException(s"$key must be non-negative")
    value

  /** Parses a strictly positive long property. */
  private def parsePositiveLong(props: Properties, key: String, default: String = null): Long =
    val value = parseNonNegativeLong(props, key, default)
    if value <= 0 then throw new IllegalStateException(s"$key must be positive")
    value

  /** Parses a canonical boolean property without accepting arbitrary false-y strings. */
  private def parseBoolean(props: Properties, key: String): Boolean = required(props, key).toLowerCase match
    case "true" => true
    case "false" => false
    case _ => throw new IllegalStateException(s"invalid boolean property $key")

  /** Validates entity and field identifiers, names, primary keys and catalog counters. */
  private def validateRestored(version: Long, nextEntityId: Long, restored: Vector[Entity]): Unit =
    if restored.map(_.id.value).distinct.size != restored.size then throw new IllegalStateException("duplicate entity id in catalog")
    if restored.map(_.name.toLowerCase).distinct.size != restored.size then throw new IllegalStateException("duplicate entity name in catalog")
    restored.foreach { entity =>
      if entity.fields.map(_.id.value).distinct.size != entity.fields.size then throw new IllegalStateException(s"duplicate field id in ${entity.name}")
      if entity.fields.map(_.name.toLowerCase).distinct.size != entity.fields.size then throw new IllegalStateException(s"duplicate field name in ${entity.name}")
      if !entity.fields.exists(_.id == entity.primaryKey) then throw new IllegalStateException(s"missing primary key field in ${entity.name}")
    }
    val maxId = restored.map(_.id.value).maxOption.getOrElse(0L)
    if nextEntityId <= maxId then throw new IllegalStateException("catalog.nextEntityId does not exceed existing entity ids")
    val maxVersion = restored.map(_.schemaVersion.value).maxOption.getOrElse(0L)
    if version < maxVersion then throw new IllegalStateException("catalog.version is behind an entity schema version")

  /** Enforces bounded non-empty catalog names. */
  private def requireName(value: String, kind: String): Unit =
    if value.isEmpty || value.length > FileCatalog.MaxNameChars then
      throw new IllegalStateException(s"$kind name length must be 1..${FileCatalog.MaxNameChars}")

/** Hard limits applied while loading untrusted/corrupt catalog metadata. */
object FileCatalog:
  /** Maximum catalog properties file size. */
  val MaxCatalogBytes: Long = 8L * 1024 * 1024
  /** Maximum entities restored from one catalog. */
  val MaxEntities: Int = 10000
  /** Maximum fields restored for one entity. */
  val MaxFieldsPerEntity: Int = 4096
  /** Maximum entity or field name length. */
  val MaxNameChars: Int = 256
