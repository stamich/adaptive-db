package io.adb.gateway

import io.adb.catalog.Catalog
import io.adb.ffm.*
import io.adb.logical.*
import io.adb.model.*
import io.adb.optimizer.RuleOptimizer
import io.adb.physical.*
import io.adb.sql.*
import scala.jdk.CollectionConverters.*
import java.util.Optional

/** SQL front door of the JVM control plane: parses, binds, optimizes and plans statements, then executes them through the native engine.
  *
  * @param catalog schema registry used for binding and DDL
  * @param native  open native database handle
  */
final class AdaptiveDatabase(catalog: Catalog, native: NativeDatabase):
  /** SQL text to AST parser. */
  private val parser = new SqlParser
  /** Resolves names and types against the catalog. */
  private val binder = new Binder(catalog)
  /** Logical-plan rewriter. */
  private val optimizer = new RuleOptimizer()

  /** Executes one SQL statement.
    *
    * @param sql statement text
    * @return rows for queries, or a status message for DDL/DML
    * @throws NativeException if the engine rejects the operation
    */
  def execute(sql: String): QueryResult =
    val ast = parser.parse(sql)
    val bound = binder.bind(ast)
    executeBound(bound)

  /** Dispatches a bound statement: DDL goes to the catalog, DML to the native row API, SELECT and EXPLAIN through planning. */
  private def executeBound(statement: BoundStatement): QueryResult = statement match
    case BoundCreateTable(name, fields, pk) =>
      val entity = catalog.createEntity(name, fields, pk)
      QueryResult(Vector.empty, Vector.empty, Some(s"created table ${entity.name} entityId=${entity.id.value}"))

    case BoundInsert(entity, values) =>
      val pkField = entity.primaryKeyField
      val pk = values.get(pkField.id) match
        case Some(DbValue.Int64Value(value)) => value
        case _ => throw new IllegalArgumentException("BIGINT primary key value required")
      val json = MutationJsonEncoder.row(entity, values)
      val ts = native.insert(entity.id.value, pk, json)
      QueryResult(Vector.empty, Vector.empty, Some(s"INSERT 1 commitTs=$ts"))

    case BoundUpdate(entity, assignments, pk) =>
      val ts = native.update(entity.id.value, pk, MutationJsonEncoder.assignments(assignments))
      QueryResult(Vector.empty, Vector.empty, Some(s"UPDATE 1 commitTs=$ts"))

    case BoundDelete(entity, pk) =>
      val ts = native.delete(entity.id.value, pk)
      QueryResult(Vector.empty, Vector.empty, Some(s"DELETE 1 commitTs=$ts"))

    case select: BoundSelect => executeSelect(select)

    case BoundExplain(inner: BoundSelect, analyze) =>
      val logical = LogicalPlanner.plan(inner)
      val optimized = optimizer.optimize(logical)
      val physical = PhysicalPlanner.plan(optimized)
      if analyze then
        val result = executeSelect(inner)
        QueryResult(
          Vector("plan"),
          Vector(
            Vector("Logical:\n" + logical),
            Vector("Optimized:\n" + optimized),
            Vector("Physical:\n" + physical),
            Vector(s"Executed rows=${result.rows.size}")
          )
        )
      else QueryResult(Vector("plan"), Vector(Vector("Logical:\n" + logical), Vector("Optimized:\n" + optimized), Vector("Physical:\n" + physical)))

    case BoundExplain(other, _) =>
      QueryResult(Vector("plan"), Vector(Vector(other.toString)))

  /** Plans a SELECT, sends the physical plan to the engine as JSON (optionally at an `AS OF` snapshot), and materializes the projected columns of every result batch.
    *
    * @throws IllegalStateException if the result exceeds [[AdaptiveDatabase.MaxMaterializedResultRows]]
    */
  private def executeSelect(select: BoundSelect): QueryResult =
    val logical = LogicalPlanner.plan(select)
    val optimized = optimizer.optimize(logical)
    val physical = PhysicalPlanner.plan(optimized)
    val json = PlanJsonEncoder.encode(physical)
    val snapshot: Optional[java.lang.Long] = select.asOfVersion match
      case Some(v) => Optional.of(java.lang.Long.valueOf(v))
      case None => Optional.empty[java.lang.Long]()
    val query = native.execute(json, snapshot)
    try
      val rows = Vector.newBuilder[Vector[Any]]
      var materializedRows = 0
      var next = query.nextBatch()
      while next.isPresent do
        val batch = next.get()
        val byField = batch.columns().asScala.map(c => c.slotId() -> c).toMap
        for row <- 0 until batch.rowCount() do
          if materializedRows >= AdaptiveDatabase.MaxMaterializedResultRows then
            throw new IllegalStateException(s"result exceeds ${AdaptiveDatabase.MaxMaterializedResultRows} rows; use LIMIT")
          rows += select.fields.map { field =>
            byField.get(field.id.value) match
              case Some(column) => column.values().get(row)
              case None => null
          }
          materializedRows += 1
        next = query.nextBatch()
      QueryResult(select.fields.map(_.name), rows.result())
    finally query.close()


/** Hard limits for the materializing Milestone 2 JVM gateway. */
object AdaptiveDatabase:
  /** Maximum rows materialized by one gateway query result. */
  val MaxMaterializedResultRows: Int = 1000000
