package io.adb.catalog

import io.adb.model.*

/** Documents `Catalog` and its role in the Milestone 2.0.1 JVM control plane. */
trait Catalog:
  /** Documents `version` and its role in the Milestone 2.0.1 JVM control plane. */
  def version: SchemaVersion
  /** Documents `entity` and its role in the Milestone 2.0.1 JVM control plane. */
  def entity(name: String): Option[Entity]
  /** Documents `entity` and its role in the Milestone 2.0.1 JVM control plane. */
  def entity(id: EntityId): Option[Entity]
  /** Documents `entities` and its role in the Milestone 2.0.1 JVM control plane. */
  def entities: Vector[Entity]
  /** Documents `createEntity` and its role in the Milestone 2.0.1 JVM control plane. */
  def createEntity(name: String, fields: Vector[(String, DataType, Boolean)], primaryKey: String): Entity
