package io.adb.physical

import io.adb.model.*

/** JSON payloads of the native INSERT/UPDATE calls (`adb_insert_row_json`, `adb_update_fields_json`). */
object MutationJsonEncoder:
  /** Full row for INSERT, including the reserved entity-id field 0. */
  def row(entity: Entity, values: Map[FieldId, DbValue]): String =
    val all = values + (SystemFields.EntityIdField -> DbValue.Int64Value(entity.id.value))
    val fields = all.toVector.sortBy(_._1.value).map { case (fieldId, value) =>
      s"\"${fieldId.value}\":${PlanJsonEncoder.encodeRustValue(value)}"
    }.mkString(",")
    s"{\"fields\":{$fields}}"

  /** Field assignments for UPDATE as `{field_id: value}`. */
  def assignments(values: Map[FieldId, DbValue]): String =
    values.toVector.sortBy(_._1.value).map { case (fieldId, value) =>
      s"\"${fieldId.value}\":${PlanJsonEncoder.encodeRustValue(value)}"
    }.mkString("{", ",", "}")
