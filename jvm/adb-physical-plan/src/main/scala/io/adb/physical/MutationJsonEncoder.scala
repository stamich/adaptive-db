package io.adb.physical

import io.adb.model.*

/** Documents `MutationJsonEncoder` and its role in the Milestone 2.0.1 JVM control plane. */
object MutationJsonEncoder:
  /** Documents `row` and its role in the Milestone 2.0.1 JVM control plane. */
  def row(entity: Entity, values: Map[FieldId, DbValue]): String =
    val all = values + (SystemFields.EntityIdField -> DbValue.Int64Value(entity.id.value))
    val fields = all.toVector.sortBy(_._1.value).map { case (fieldId, value) =>
      s"\"${fieldId.value}\":${PlanJsonEncoder.encodeRustValue(value)}"
    }.mkString(",")
    s"{\"fields\":{$fields}}"

  /** Documents `assignments` and its role in the Milestone 2.0.1 JVM control plane. */
  def assignments(values: Map[FieldId, DbValue]): String =
    values.toVector.sortBy(_._1.value).map { case (fieldId, value) =>
      s"\"${fieldId.value}\":${PlanJsonEncoder.encodeRustValue(value)}"
    }.mkString("{", ",", "}")
