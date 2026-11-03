package io.adb.advisor

/** What the advisor can say. A recommendation is advice for a person; the advisor never runs it.
  *
  * Only [[CreateIndex]] and [[RunAnalyze]] map to statements the engine has. Everything else is
  * [[Informational]]: a finding that points at a problem the engine has no action for yet.
  */
sealed trait Recommendation derives CanEqual:
  /** Stable identity of the finding. Two evaluations that find the same thing produce the same
    * key, which is what lets hysteresis count how many times in a row it was found.
    */
  def key: String

  /** One-line description for `\advice` and the audit log. */
  def summary: String

  /** The SQL statement that carries the advice out, if there is one. */
  def sql: Option[String]

  /** Cost-benefit estimate, if the finding has one. */
  def score: Option[Score]

/** Create an index.
  *
  * @param entityId   entity to index
  * @param entityName its name
  * @param fields     key columns, by field id, in index order
  * @param fieldNames the same columns by name
  * @param score      cost-benefit estimate
  * @param shapeIds   shapes whose evidence led to the recommendation
  */
final case class CreateIndex(
    entityId: Long,
    entityName: String,
    fields: Vector[Long],
    fieldNames: Vector[String],
    score: Option[Score],
    shapeIds: Vector[Long]
) extends Recommendation:
  /** Generated index name, `adv_<table>_<columns>`. */
  def indexName: String = s"adv_${entityName}_${fieldNames.mkString("_")}".toLowerCase
  def key: String = s"index:$entityId:${fields.mkString(",")}"
  def sql: Option[String] = Some(s"CREATE INDEX $indexName ON $entityName (${fieldNames.mkString(", ")})")
  def summary: String =
    val ratio = score.fold("")(s => f" (benefit/cost ${s.ratio}%.1f)")
    s"create an index on $entityName(${fieldNames.mkString(", ")})$ratio"

/** Refresh the statistics of an entity.
  *
  * @param entityId   entity to analyze
  * @param entityName its name
  * @param reason     why (stale statistics, or an estimate that was far off)
  * @param shapeIds   shapes whose evidence led to the recommendation
  */
final case class RunAnalyze(entityId: Long, entityName: String, reason: String, shapeIds: Vector[Long]) extends Recommendation:
  def key: String = s"analyze:$entityId"
  def sql: Option[String] = Some(s"ANALYZE $entityName")
  def score: Option[Score] = None
  def summary: String = s"analyze $entityName: $reason"

/** A finding without an action the engine can carry out (a hot join that wants a join projection,
  * a hot table that wants partitioning).
  *
  * @param kind     short category, e.g. `hot-join`
  * @param key      stable identity, see [[Recommendation.key]]
  * @param text     description of the finding
  * @param shapeIds shapes it concerns
  */
final case class Informational(kind: String, key: String, text: String, shapeIds: Vector[Long]) extends Recommendation:
  def sql: Option[String] = None
  def score: Option[Score] = None
  def summary: String = s"$kind: $text"
