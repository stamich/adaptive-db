package io.adb.gateway

import io.adb.consistency.*
import io.adb.model.JsonReader
import io.adb.optimizer.access.IndexDecision

/** Text and table renderings of freshness state, for EXPLAIN and the shell's meta-commands. */
object ConsistencyReport:
  /** Column names of [[projectionRows]]. */
  val ProjectionColumns: Vector[String] =
    Vector("id", "name", "kind", "mode", "state", "frontier", "head", "lag_commits", "commits_per_second")

  /** One row per projection. */
  def projectionRows(infos: Vector[ProjectionInfo]): Vector[Vector[Any]] =
    infos.map { p =>
      Vector[Any](p.id, p.name, p.kind.toString, p.mode.toString, p.state.toString, p.frontier.readableThrough,
        p.headCommitSeq, p.lagCommits, f"${p.commitsPerSecond}%.1f")
    }

  /** Column names of [[sessionRows]]. */
  val SessionColumns: Vector[String] = Vector("setting", "value")

  /** The session's level, token, head and wait limit. */
  def sessionRows(context: SemanticReadContext): Vector[Vector[Any]] =
    Vector(
      Vector("consistency", context.consistency.sql),
      Vector("token", context.token.commitSeq),
      Vector("head", context.headCommitSeq),
      Vector("max_wait_ms", context.maxWaitMillis)
    )

  /** Column names of [[changeRows]]. */
  val ChangeColumns: Vector[String] = Vector("commit_seq", "commit_ts", "schema_epoch", "kind", "changes")

  /** One row per commit of a change-cursor batch document (see `include/adb.h`).
    *
    * @throws IllegalArgumentException if the document is malformed
    */
  def changeRows(batchJson: String): Vector[Vector[Any]] =
    JsonReader.parse(batchJson) match
      case root: Map[?, ?] =>
        root.asInstanceOf[Map[String, Any]].get("commits") match
          case Some(commits: Vector[?]) =>
            commits.map {
              case commit: Map[?, ?] =>
                val c = commit.asInstanceOf[Map[String, Any]]
                def text(key: String): String = c.get(key) match
                  case Some(v: String) => v
                  case other => throw new IllegalArgumentException(s"commit member $key must be a string, got $other")
                val count = c.get("changes") match
                  case Some(changes: Vector[?]) => changes.size
                  case other => throw new IllegalArgumentException(s"commit member changes must be an array, got $other")
                Vector[Any](text("commit_seq"), text("commit_ts"), text("schema_epoch"), text("kind"), count)
              case other => throw new IllegalArgumentException(s"commit must be an object, got $other")
            }
          case other => throw new IllegalArgumentException(s"batch must have a commits array, got $other")
      case other => throw new IllegalArgumentException(s"batch must be an object, got $other")

  /** The EXPLAIN section "Read context": the session's level and, per index, the decision.
    *
    * @param context   what the query was planned with
    * @param decisions one decision per index of the database
    */
  def explain(context: SemanticReadContext, decisions: Vector[IndexDecision]): String =
    val header = s"- consistency=${context.consistency.sql} token=${context.token.commitSeq} head=${context.headCommitSeq} max wait ${context.maxWaitMillis} ms"
    val lines = decisions.map { d =>
      val state = d.projection.fold("synchronous (no projection report)")(p =>
        s"projection ${p.name} ${p.state}, at ${p.frontier.readableThrough}, lag ${math.max(0L, context.headCommitSeq - p.frontier.readableThrough)}")
      s"- index ${d.index.name}: ${describe(d.resolution)} ($state)"
    }
    ("Read context:" +: header +: lines).mkString("\n")

  /** A decision in words. */
  def describe(resolution: FreshnessResolution): String = resolution match
    case FreshnessResolution.Direct => "usable"
    case FreshnessResolution.WaitUntil(seq, Some(ms)) => s"usable after waiting for commit $seq (about $ms ms)"
    case FreshnessResolution.WaitUntil(seq, None) => s"usable after waiting for commit $seq"
    case FreshnessResolution.AllowStale(lag) => s"usable, $lag commits behind is allowed"
    case FreshnessResolution.Fallback(reason) => s"not used: $reason"
    case FreshnessResolution.Infeasible(reason) => s"not used: $reason"
