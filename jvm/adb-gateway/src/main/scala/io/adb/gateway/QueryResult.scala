package io.adb.gateway

/** Documents `QueryResult` and its role in the Milestone 2.0.1 JVM control plane. */
final case class QueryResult(columns: Vector[String], rows: Vector[Vector[Any]], message: Option[String] = None)
