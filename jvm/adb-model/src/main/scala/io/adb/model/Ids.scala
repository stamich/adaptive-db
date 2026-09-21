package io.adb.model

/** Documents `EntityId` and its role in the Milestone 2.0.1 JVM control plane. */
opaque type EntityId = Long
/** Documents `EntityId` and its role in the Milestone 2.0.1 JVM control plane. */
object EntityId:
  /** Documents `apply` and its role in the Milestone 2.0.1 JVM control plane. */
  def apply(value: Long): EntityId = value
  extension (id: EntityId) def value: Long = id

/** Documents `FieldId` and its role in the Milestone 2.0.1 JVM control plane. */
opaque type FieldId = Int
/** Documents `FieldId` and its role in the Milestone 2.0.1 JVM control plane. */
object FieldId:
  /** Documents `apply` and its role in the Milestone 2.0.1 JVM control plane. */
  def apply(value: Int): FieldId = value
  extension (id: FieldId) def value: Int = id

/** Documents `SchemaVersion` and its role in the Milestone 2.0.1 JVM control plane. */
opaque type SchemaVersion = Long
/** Documents `SchemaVersion` and its role in the Milestone 2.0.1 JVM control plane. */
object SchemaVersion:
  /** Documents `apply` and its role in the Milestone 2.0.1 JVM control plane. */
  def apply(value: Long): SchemaVersion = value
  extension (version: SchemaVersion) def value: Long = version

/** Documents `SystemFields` and its role in the Milestone 2.0.1 JVM control plane. */
object SystemFields:
  val EntityIdField: FieldId = FieldId(0)
