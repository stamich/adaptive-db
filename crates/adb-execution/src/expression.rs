//! Scalar expressions evaluated per row (filters and projections).
use std::cmp::Ordering;

use adb_core::{FieldId, Row, Value};
use serde::{Deserialize, Serialize};

use crate::ExecutionError;

/// Comparison and boolean operators.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Expression tree; the serde form is part of the plan wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expr {
    Column {
        field_id: FieldId,
    },
    Literal {
        value: Value,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Not {
        expr: Box<Expr>,
    },
}

impl Expr {
    /// Rejects trees deeper than the hardened limit (protects the recursive evaluator).
    pub fn validate(&self) -> Result<(), String> {
        self.validate_depth(0)
    }

    /// Depth-bounded validation helper.
    fn validate_depth(&self, depth: usize) -> Result<(), String> {
        /// Deepest expression tree accepted.
        const MAX_EXPR_DEPTH: usize = 128;
        if depth > MAX_EXPR_DEPTH {
            return Err(format!("expression depth exceeds {MAX_EXPR_DEPTH}"));
        }

        match self {
            Self::Column { .. } | Self::Literal { .. } => Ok(()),
            Self::Binary { left, right, .. } => {
                left.validate_depth(depth + 1)?;
                right.validate_depth(depth + 1)
            }
            Self::Not { expr } => expr.validate_depth(depth + 1),
        }
    }

    /// Evaluates as a predicate; NULL counts as false.
    pub fn evaluate_bool(&self, row: &Row) -> Result<bool, ExecutionError> {
        match self.evaluate(row)? {
            Value::Bool(value) => Ok(value),
            Value::Null => Ok(false),
            other => Err(ExecutionError::Expression(format!(
                "predicate produced non-boolean value {other:?}"
            ))),
        }
    }

    /// Evaluates against one row; absent fields are NULL.
    pub fn evaluate(&self, row: &Row) -> Result<Value, ExecutionError> {
        match self {
            Self::Column { field_id } => Ok(row.get(*field_id).cloned().unwrap_or(Value::Null)),

            Self::Literal { value } => Ok(value.clone()),

            Self::Not { expr } => match expr.evaluate(row)? {
                Value::Bool(value) => Ok(Value::Bool(!value)),
                Value::Null => Ok(Value::Null),
                other => Err(ExecutionError::Expression(format!(
                    "NOT expects bool, got {other:?}"
                ))),
            },

            Self::Binary { left, op, right } => {
                let left = left.evaluate(row)?;
                let right = right.evaluate(row)?;
                eval_binary(left, *op, right)
            }
        }
    }
}

/// Applies a binary operator with SQL-like NULL propagation.
fn eval_binary(left: Value, op: BinaryOp, right: Value) -> Result<Value, ExecutionError> {
    use BinaryOp::*;

    match op {
        And | Or => {
            let l = as_bool(&left)?;
            let r = as_bool(&right)?;
            return Ok(Value::Bool(match op {
                And => l && r,
                Or => l || r,
                _ => unreachable!(),
            }));
        }
        _ => {}
    }

    if matches!(left, Value::Null) || matches!(right, Value::Null) {
        return Ok(Value::Null);
    }

    let ordering = compare(&left, &right)?;
    let result = match op {
        Eq => ordering == Ordering::Equal,
        Ne => ordering != Ordering::Equal,
        Lt => ordering == Ordering::Less,
        Le => ordering != Ordering::Greater,
        Gt => ordering == Ordering::Greater,
        Ge => ordering != Ordering::Less,
        And | Or => unreachable!(),
    };

    Ok(Value::Bool(result))
}

/// Interprets a value as a boolean (NULL is false).
fn as_bool(value: &Value) -> Result<bool, ExecutionError> {
    match value {
        Value::Bool(value) => Ok(*value),
        Value::Null => Ok(false),
        other => Err(ExecutionError::Expression(format!(
            "boolean operator expects bool, got {other:?}"
        ))),
    }
}

/// Orders two non-null values of compatible types.
fn compare(left: &Value, right: &Value) -> Result<Ordering, ExecutionError> {
    match (left, right) {
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        (Value::Int64(a), Value::Int64(b)) => Ok(a.cmp(b)),
        (Value::Float64(a), Value::Float64(b)) => a
            .partial_cmp(b)
            .ok_or_else(|| ExecutionError::Expression("cannot compare NaN".to_string())),
        (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
        (Value::Bytes(a), Value::Bytes(b)) => Ok(a.cmp(b)),
        _ => Err(ExecutionError::Expression(format!(
            "incompatible comparison: {left:?} vs {right:?}"
        ))),
    }
}
