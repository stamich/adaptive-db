package io.adb.gateway

/** Materialized result of one statement.
  *
  * @param columns   column names, in output order
  * @param rows      row values, one inner vector per row aligned with `columns`
  * @param message   status message for statements that return no rows
  * @param commitSeq position in the commit order of the commit the statement made (DML and DDL),
  *                  which is what a session remembers to read its own writes
  */
final case class QueryResult(
    columns: Vector[String],
    rows: Vector[Vector[Any]],
    message: Option[String] = None,
    commitSeq: Option[Long] = None
)
