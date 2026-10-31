package io.adb.gateway

import io.adb.catalog.SchemaBackend
import io.adb.ffm.NativeDatabase

/** The engine catalog functions of an open native database (C ABI 7), for [[io.adb.catalog.EngineCatalog]].
  *
  * @param native open native database
  */
final class NativeSchemaBackend(native: NativeDatabase) extends SchemaBackend:
  def currentSchemaEpoch(): Long = native.currentSchemaEpoch()
  def listSchemasJson(): String = native.listSchemasJson()
  def createEntityJson(specJson: String): String = native.createEntityJson(specJson)
  def alterEntityJson(requestJson: String): String = native.alterEntityJson(requestJson)
  def dropEntity(entityId: Long): Unit = native.dropEntity(entityId)
  def schemaHistoryJson(entityId: Long): String = native.schemaHistoryJson(entityId)
