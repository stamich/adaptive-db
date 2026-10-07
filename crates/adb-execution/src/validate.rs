//! Structural validation of physical plans.
//!
//! Validation runs once, before any operator is built, and guarantees everything operators
//! later rely on without re-checking: bounded depth and list sizes, slot ids below
//! [`MAX_SLOTS`], no slot produced twice, and every slot an operator reads is produced by its
//! input. Operators can then index [`crate::ExecRow`] values without bounds surprises.
use std::collections::HashSet;

use crate::{
    limits::{MAX_LIMIT, MAX_LIST_LEN, MAX_PLAN_DEPTH, MAX_SLOTS},
    AggregateFunction, Expr, PhysicalPlan, ScanColumn, SlotId, SortKey,
};

/// What validation learned about a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanShape {
    /// Length of every [`crate::ExecRow`] of the query: highest slot used plus one.
    pub width: usize,
    /// Slots of the root's output, in output-column order.
    pub output: Vec<SlotId>,
}

impl PhysicalPlan {
    /// Validates the whole plan and returns its shape.
    pub fn validate(&self) -> Result<PlanShape, String> {
        let mut validator = Validator { width: 0 };
        let output = validator.node(self, 0)?;
        Ok(PlanShape {
            width: validator.width,
            output,
        })
    }
}

/// Walks the plan bottom-up, tracking the highest slot seen.
struct Validator {
    /// Highest slot id seen plus one.
    width: usize,
}

impl Validator {
    /// Validates one node and returns the slots it outputs.
    fn node(&mut self, plan: &PhysicalPlan, depth: usize) -> Result<Vec<SlotId>, String> {
        if depth > MAX_PLAN_DEPTH {
            return Err(format!("physical plan depth exceeds {MAX_PLAN_DEPTH}"));
        }
        match plan {
            PhysicalPlan::PointLookup { columns, .. }
            | PhysicalPlan::Scan { columns }
            | PhysicalPlan::EntityScan { columns, .. } => self.scan_columns(columns),

            PhysicalPlan::Filter { input, predicate } => {
                let output = self.node(input, depth + 1)?;
                self.expr(predicate, &output, "filter predicate")?;
                Ok(output)
            }

            PhysicalPlan::Project { input, slots } => {
                let output = self.node(input, depth + 1)?;
                self.list("projected slots", slots.len())?;
                distinct("projected slots", slots)?;
                require_all(slots, &output, "project")?;
                Ok(slots.clone())
            }

            PhysicalPlan::Limit { input, limit } => {
                check_limit("LIMIT", *limit)?;
                self.node(input, depth + 1)
            }

            PhysicalPlan::HashJoin {
                left,
                right,
                keys,
                residual,
                ..
            } => {
                let (left_out, right_out) = self.join_inputs(left, right, depth)?;
                if keys.is_empty() {
                    return Err("hash join needs at least one key".into());
                }
                self.list("join keys", keys.len())?;
                let left_keys: Vec<SlotId> = keys.iter().map(|key| key.left).collect();
                let right_keys: Vec<SlotId> = keys.iter().map(|key| key.right).collect();
                require_all(&left_keys, &left_out, "hash join left key")?;
                require_all(&right_keys, &right_out, "hash join right key")?;
                let output = [left_out, right_out].concat();
                if let Some(residual) = residual {
                    self.expr(residual, &output, "hash join residual")?;
                }
                Ok(output)
            }

            PhysicalPlan::NestedLoopJoin {
                left,
                right,
                predicate,
                ..
            } => {
                let (left_out, right_out) = self.join_inputs(left, right, depth)?;
                let output = [left_out, right_out].concat();
                if let Some(predicate) = predicate {
                    self.expr(predicate, &output, "nested loop join predicate")?;
                }
                Ok(output)
            }

            PhysicalPlan::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let input_out = self.node(input, depth + 1)?;
                if group_by.is_empty() && aggregates.is_empty() {
                    return Err("aggregate needs grouping slots or aggregates".into());
                }
                self.list("group by slots", group_by.len())?;
                self.list("aggregates", aggregates.len())?;
                distinct("group by slots", group_by)?;
                require_all(group_by, &input_out, "group by")?;
                for aggregate in aggregates {
                    match aggregate.input {
                        Some(slot) => require_all(&[slot], &input_out, "aggregate input")?,
                        None if aggregate.function == AggregateFunction::Count => {}
                        None => {
                            return Err(format!("{:?} needs an input slot", aggregate.function))
                        }
                    }
                }
                let outputs: Vec<SlotId> = aggregates.iter().map(|a| a.output).collect();
                for slot in &outputs {
                    if input_out.contains(slot) {
                        return Err(format!(
                            "aggregate output slot {slot} is already produced by the input"
                        ));
                    }
                }
                self.new_slots(&outputs, "aggregate outputs")?;
                let output = [group_by.as_slice(), outputs.as_slice()].concat();
                distinct("aggregate output", &output)?;
                Ok(output)
            }

            PhysicalPlan::Sort { input, keys } => {
                let output = self.node(input, depth + 1)?;
                self.sort_keys(keys, &output)?;
                Ok(output)
            }

            PhysicalPlan::TopK { input, keys, limit } => {
                check_limit("TopK", *limit)?;
                let output = self.node(input, depth + 1)?;
                self.sort_keys(keys, &output)?;
                Ok(output)
            }
        }
    }

    /// Checks sort keys: non-empty, bounded, reading slots of the input.
    fn sort_keys(&self, keys: &[SortKey], available: &[SlotId]) -> Result<(), String> {
        if keys.is_empty() {
            return Err("sort needs at least one key".into());
        }
        self.list("sort keys", keys.len())?;
        let slots: Vec<SlotId> = keys.iter().map(|key| key.slot).collect();
        require_all(&slots, available, "sort key")
    }

    /// Validates both join inputs and checks that they produce disjoint slots.
    fn join_inputs(
        &mut self,
        left: &PhysicalPlan,
        right: &PhysicalPlan,
        depth: usize,
    ) -> Result<(Vec<SlotId>, Vec<SlotId>), String> {
        let left_out = self.node(left, depth + 1)?;
        let right_out = self.node(right, depth + 1)?;
        distinct(
            "join output",
            &[left_out.as_slice(), right_out.as_slice()].concat(),
        )?;
        Ok((left_out, right_out))
    }

    /// Checks a leaf's field-to-slot mapping and returns its slots.
    fn scan_columns(&mut self, columns: &[ScanColumn]) -> Result<Vec<SlotId>, String> {
        self.list("scan columns", columns.len())?;
        let slots: Vec<SlotId> = columns.iter().map(|column| column.slot).collect();
        self.new_slots(&slots, "scan columns")?;
        Ok(slots)
    }

    /// Checks slots an operator *produces*: in range and unique.
    fn new_slots(&mut self, slots: &[SlotId], what: &str) -> Result<(), String> {
        distinct(what, slots)?;
        for slot in slots {
            self.slot(*slot)?;
        }
        Ok(())
    }

    /// Checks that one slot id is in range and records it in the row width.
    fn slot(&mut self, slot: SlotId) -> Result<(), String> {
        if slot.0 >= MAX_SLOTS {
            return Err(format!(
                "slot {slot} exceeds the limit of {MAX_SLOTS} slots"
            ));
        }
        self.width = self.width.max(slot.index() + 1);
        Ok(())
    }

    /// Checks an expression's depth and that it only reads slots in `available`.
    fn expr(&mut self, expr: &Expr, available: &[SlotId], what: &str) -> Result<(), String> {
        expr.validate()?;
        require_all(&expr.referenced_slots(), available, what)
    }

    /// Checks a list length against [`MAX_LIST_LEN`].
    fn list(&self, what: &str, len: usize) -> Result<(), String> {
        if len > MAX_LIST_LEN {
            return Err(format!("{what}: {len} entries exceed {MAX_LIST_LEN}"));
        }
        Ok(())
    }
}

/// Checks a LIMIT-like row count against [`MAX_LIMIT`].
pub(crate) fn check_limit(what: &str, limit: usize) -> Result<(), String> {
    if limit > MAX_LIMIT {
        return Err(format!("{what} {limit} exceeds {MAX_LIMIT}"));
    }
    Ok(())
}

/// Fails if `slots` contains a duplicate.
pub(crate) fn distinct(what: &str, slots: &[SlotId]) -> Result<(), String> {
    let mut seen = HashSet::with_capacity(slots.len());
    for slot in slots {
        if !seen.insert(*slot) {
            return Err(format!("{what}: slot {slot} appears twice"));
        }
    }
    Ok(())
}

/// Fails unless every slot of `needed` is in `available`.
pub(crate) fn require_all(
    needed: &[SlotId],
    available: &[SlotId],
    what: &str,
) -> Result<(), String> {
    let available: HashSet<SlotId> = available.iter().copied().collect();
    for slot in needed {
        if !available.contains(slot) {
            return Err(format!(
                "{what} reads slot {slot}, which its input does not produce"
            ));
        }
    }
    Ok(())
}
