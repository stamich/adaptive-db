package io.adb.advisor

/** The detectors of the advisor. */
object Detectors:
  /** All detectors, in report order. */
  val all: Vector[Detector] = Vector(HotQueryDetector, ScanAmplificationDetector, HotJoinDetector, CardinalityErrorDetector)

  /** Hot shapes that have enough evidence: at least `minExecutions` captured runs. */
  private[advisor] def evidenced(ctx: DetectorContext): Iterator[ShapeDelta] =
    ctx.workload.shapes.valuesIterator.filter(_.executions >= ctx.model.minExecutions)

  /** Names of the entities of a shape, falling back to the id for dropped tables. */
  private[advisor] def entityNames(ctx: DetectorContext, entities: Vector[Long]): String =
    entities.map(id => ctx.design.entity(id).fold(s"#$id")(_.name)).mkString(", ")

/** Reports the shapes that take a large share of the workload's time. Informational only: it says
  * where the time goes; the other detectors say what could be done about it.
  */
object HotQueryDetector extends Detector:
  def name: String = "hot-query"

  def detect(ctx: DetectorContext): Vector[Recommendation] =
    Detectors.evidenced(ctx).filter(d => ctx.workload.timeShare(d.shapeId) >= ctx.config.hotTimeShare).flatMap { d =>
      ctx.shapes(d.shapeId).map { shape =>
        val share = (ctx.workload.timeShare(d.shapeId) * 100).round
        Informational(
          "hot-query",
          s"hot-query:${d.shapeId}",
          f"a query on ${Detectors.entityNames(ctx, shape.entities)} takes $share%d%% of the workload's time (${d.executions}%d runs, mean ${d.meanNanos / 1e6}%.2f ms)",
          Vector(d.shapeId)
        )
      }
    }.toVector.sortBy(_.key)

/** Finds shapes that read many more rows than they return and filter on a column nothing indexes.
  *
  * Only single-table shapes are judged here: with a join the waste cannot be assigned to one
  * filter column ([[HotJoinDetector]] covers those). The candidate is an index on the shape's first
  * filter column; evidence from several shapes that want the same index is added up before the
  * cost-benefit gate.
  */
object ScanAmplificationDetector extends Detector:
  def name: String = "scan-amplification"

  def detect(ctx: DetectorContext): Vector[Recommendation] =
    // (entity, field) -> (summed benefit, shapes)
    val wanted = Detectors.evidenced(ctx).foldLeft(Map.empty[(Long, Long), (Double, Vector[Long])]) { (acc, d) =>
      ctx.shapes(d.shapeId) match
        case Some(shape) if shape.entities.size == 1 && d.scanAmplification >= ctx.config.scanAmplification =>
          val entity = shape.entities.head
          val candidate = for
            design <- ctx.design.entity(entity)
            filter <- shape.filterFields.find(f => f.entity == entity && !design.hasLeadingIndexOn(f.field))
            // If any filter column already leads an index, the shape is served by it: no candidate.
            if shape.filterFields.forall(f => f.entity != entity || !design.hasLeadingIndexOn(f.field))
          yield (entity, filter.field)
          candidate.fold(acc) { key =>
            val benefit = ctx.model.indexBenefit(d, ctx.workload.spanNanos)
            val (sum, ids) = acc.getOrElse(key, (0.0, Vector.empty[Long]))
            acc.updated(key, (sum + benefit, ids :+ d.shapeId))
          }
        case _ => acc
    }
    wanted.iterator.flatMap { case ((entity, field), (benefit, ids)) =>
      ctx.design.entity(entity).flatMap { design =>
        val score = ctx.model.scoreIndex(benefit, design.rowCount)
        if ctx.model.worthIt(score) then
          Some(CreateIndex(entity, design.name, Vector(field), Vector(design.fieldName(field)), Some(score), ids.sorted))
        else None
      }
    }.toVector.sortBy(r => (-r.score.fold(0.0)(_.ratio), r.key))

/** Reports hot shapes that join tables. The engine has no action for them (a join projection
  * would be one), so the finding is informational; it says which join column of each side has no
  * leading index, which is where a person would look first.
  */
object HotJoinDetector extends Detector:
  def name: String = "hot-join"

  def detect(ctx: DetectorContext): Vector[Recommendation] =
    Detectors.evidenced(ctx).filter(d => ctx.workload.timeShare(d.shapeId) >= ctx.config.hotTimeShare).flatMap { d =>
      ctx.shapes(d.shapeId).filter(_.joinEdges.nonEmpty).map { shape =>
        val unindexed = shape.joinEdges.flatMap { e =>
          Vector((e.leftEntity, e.leftField), (e.rightEntity, e.rightField))
        }.distinct.flatMap { case (entity, field) =>
          ctx.design.entity(entity).filterNot(_.hasLeadingIndexOn(field)).map(e => s"${e.name}.${e.fieldName(field)}")
        }
        val share = (ctx.workload.timeShare(d.shapeId) * 100).round
        val where = if unindexed.isEmpty then "every join column has a leading index" else s"no leading index on ${unindexed.mkString(", ")}"
        Informational(
          "hot-join",
          s"hot-join:${d.shapeId}",
          s"a join of ${Detectors.entityNames(ctx, shape.entities)} takes $share% of the workload's time; $where (a join projection would be the next step)",
          Vector(d.shapeId)
        )
      }
    }.toVector.sortBy(_.key)

/** Finds shapes whose row estimates were far off. If the statistics of a table the shape reads are
  * missing or stale, the advice is `ANALYZE`; if they are fresh, the estimate error is a model
  * limit and is reported as information.
  */
object CardinalityErrorDetector extends Detector:
  def name: String = "cardinality-error"

  def detect(ctx: DetectorContext): Vector[Recommendation] =
    val bad = Detectors.evidenced(ctx).filter(_.maxQError >= ctx.config.qErrorThreshold).toVector.sortBy(_.shapeId)
    val perEntity = bad.foldLeft(Map.empty[Long, (String, Vector[Long])]) { (acc, d) =>
      ctx.shapes(d.shapeId).fold(acc) { shape =>
        shape.entities.foldLeft(acc) { (inner, entity) =>
          ctx.design.entity(entity).fold(inner) { design =>
            val reason = analyzeReason(design, ctx.config)
            val (r, ids) = inner.getOrElse(entity, (reason, Vector.empty[Long]))
            inner.updated(entity, (r, ids :+ d.shapeId))
          }
        }
      }
    }
    perEntity.iterator.map { case (entity, (reason, ids)) =>
      val design = ctx.design.entity(entity).get
      if reason.isEmpty then
        Informational(
          "cardinality-error",
          s"cardinality:$entity",
          s"estimates for ${design.name} are off by 10x or more although its statistics are fresh",
          ids.distinct
        )
      else RunAnalyze(entity, design.name, reason, ids.distinct)
    }.toVector.sortBy(_.key)

  /** Why `design`'s statistics need refreshing, or the empty string if they do not. */
  private def analyzeReason(design: EntityDesign, config: DetectorConfig): String = design.statistics match
    case None => "it was never analyzed"
    case Some(stats) if stats.changeRatio > config.stalePendingChangeRatio =>
      f"${stats.changeRatio * 100}%.0f%% of its rows changed since the last ANALYZE"
    case Some(_) => ""
