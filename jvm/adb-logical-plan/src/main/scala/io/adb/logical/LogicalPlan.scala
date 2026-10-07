package io.adb.logical

import io.adb.model.*

/** Relational operator tree produced from a bound SELECT; input to the optimizer.
  *
  * Every operator knows the attributes it outputs; expressions above an operator may only read
  * those attributes.
  */
sealed trait LogicalPlan derives CanEqual:
  /** Attributes this operator outputs, in order. */
  def output: Vector[Attribute]
  /** Input operators, left to right. */
  def children: Vector[LogicalPlan]

/** Logical operators. */
object LogicalPlan:
  /** Reads every row of one relation instance. */
  final case class TableScan(relation: BoundRelation) extends LogicalPlan:
    /** The relation's referenced columns. */
    def output = relation.columns
    /** None. */
    def children = Vector.empty
  /** Reads one row of a relation instance by primary key (produced by the point-lookup rule). */
  final case class PointLookup(relation: BoundRelation, primaryKey: Long) extends LogicalPlan:
    /** The relation's referenced columns. */
    def output = relation.columns
    /** None. */
    def children = Vector.empty
  /** Keeps the rows of `input` for which `predicate` is true. */
  final case class Filter(input: LogicalPlan, predicate: TypedExpr) extends LogicalPlan:
    /** Same as the input. */
    def output = input.output
    /** The input. */
    def children = Vector(input)
  /** Joins two inputs; outputs the left attributes followed by the right ones. */
  final case class Join(left: LogicalPlan, right: LogicalPlan, joinType: JoinType, condition: Option[TypedExpr]) extends LogicalPlan:
    /** Left attributes, then right attributes (the binder already marks the attributes of a
      * LEFT JOIN's right side nullable).
      */
    def output = left.output ++ right.output
    /** Left and right input. */
    def children = Vector(left, right)
  /** Groups `input` by `groupBy` and computes `aggregates`. */
  final case class Aggregate(input: LogicalPlan, groupBy: Vector[Attribute], aggregates: Vector[BoundAggregate]) extends LogicalPlan:
    /** Grouping attributes, then aggregate results. */
    def output = groupBy ++ aggregates.map(_.output)
    /** The input. */
    def children = Vector(input)
  /** Orders `input` by `keys`. */
  final case class Sort(input: LogicalPlan, keys: Vector[BoundOrder]) extends LogicalPlan:
    /** Same as the input. */
    def output = input.output
    /** The input. */
    def children = Vector(input)
  /** Selects and orders the output attributes. */
  final case class Project(input: LogicalPlan, attributes: Vector[Attribute]) extends LogicalPlan:
    /** The projected attributes. */
    def output = attributes
    /** The input. */
    def children = Vector(input)
  /** Stops after `limit` rows. */
  final case class Limit(input: LogicalPlan, limit: Int) extends LogicalPlan:
    /** Same as the input. */
    def output = input.output
    /** The input. */
    def children = Vector(input)

  /** `plan` with its inputs replaced by `children` (same order as [[LogicalPlan.children]]). */
  def withChildren(plan: LogicalPlan, children: Vector[LogicalPlan]): LogicalPlan = plan match
    case TableScan(_) | PointLookup(_, _) => plan
    case Filter(_, predicate) => Filter(children(0), predicate)
    case Join(_, _, joinType, condition) => Join(children(0), children(1), joinType, condition)
    case Aggregate(_, groupBy, aggregates) => Aggregate(children(0), groupBy, aggregates)
    case Sort(_, keys) => Sort(children(0), keys)
    case Project(_, attributes) => Project(children(0), attributes)
    case Limit(_, limit) => Limit(children(0), limit)

  /** Applies `rewrite` to every input of `plan`. */
  def mapChildren(plan: LogicalPlan)(rewrite: LogicalPlan => LogicalPlan): LogicalPlan =
    if plan.children.isEmpty then plan else withChildren(plan, plan.children.map(rewrite))

  /** Wraps `plan` in a filter of the conjunction of `predicates` (no filter if there are none). */
  def filtered(plan: LogicalPlan, predicates: Vector[TypedExpr]): LogicalPlan =
    TypedExpr.conjunction(predicates).fold(plan)(Filter(plan, _))

  /** Indented, one-operator-per-line rendering for EXPLAIN. */
  def render(plan: LogicalPlan, indent: Int = 0): String =
    val pad = "  " * indent
    val line = plan match
      case TableScan(relation) => s"TableScan ${scanLabel(relation)}"
      case PointLookup(relation, key) => s"PointLookup ${scanLabel(relation)} key=$key"
      case Filter(_, predicate) => s"Filter ${predicate.display}"
      case Join(_, _, joinType, condition) => s"Join ${joinType.toString.toUpperCase}${condition.fold("")(c => s" ON ${c.display}")}"
      case Aggregate(_, groupBy, aggregates) =>
        s"Aggregate group=[${groupBy.map(_.display).mkString(", ")}] aggregates=[${aggregates.map(a => s"${a.display} -> ${a.output.display}").mkString(", ")}]"
      case Sort(_, keys) => s"Sort [${keys.map(_.display).mkString(", ")}]"
      case Project(_, attributes) => s"Project [${attributes.map(_.display).mkString(", ")}]"
      case Limit(_, limit) => s"Limit $limit"
    ((pad + line) +: plan.children.map(render(_, indent + 1))).mkString("\n")

  /** `entity AS alias [columns]` label of a scan. */
  private def scanLabel(relation: BoundRelation): String =
    val alias = if relation.alias.equalsIgnoreCase(relation.entity.name) then "" else s" AS ${relation.alias}"
    s"${relation.entity.name}$alias [${relation.columns.map(_.display).mkString(", ")}]"
