//! Columnar record batches produced by query cursors.
//!
//! A batch has one column per output slot (in the plan's output order) and, when every row came
//! straight from storage, the rows' storage keys. Join and aggregate rows have no storage key,
//! so their batches carry no row ids (batch format v2, see docs/batch-format.md).
use adb_core::{RowId, Value};

use crate::{ExecRow, ExecutionError, SlotId};

/// Physical type of a column vector; the discriminant is the wire type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PhysicalType {
    /// Nullable booleans.
    Bool = 1,
    /// Nullable signed 64-bit integers.
    Int64 = 2,
    /// Nullable doubles.
    Float64 = 3,
    /// Nullable UTF-8 strings.
    String = 4,
    /// Nullable byte strings.
    Bytes = 5,
}

/// One typed, nullable column of a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnVector {
    /// Boolean column.
    Bool {
        /// Output slot the column holds.
        slot: SlotId,
        /// One value per row; `None` is NULL or absent.
        values: Vec<Option<bool>>,
    },
    /// Integer column.
    Int64 {
        /// Output slot the column holds.
        slot: SlotId,
        /// One value per row; `None` is NULL or absent.
        values: Vec<Option<i64>>,
    },
    /// Floating-point column.
    Float64 {
        /// Output slot the column holds.
        slot: SlotId,
        /// One value per row; `None` is NULL or absent.
        values: Vec<Option<f64>>,
    },
    /// String column.
    String {
        /// Output slot the column holds.
        slot: SlotId,
        /// One value per row; `None` is NULL or absent.
        values: Vec<Option<String>>,
    },
    /// Byte-string column.
    Bytes {
        /// Output slot the column holds.
        slot: SlotId,
        /// One value per row; `None` is NULL or absent.
        values: Vec<Option<Vec<u8>>>,
    },
}

impl ColumnVector {
    /// Output slot the column holds.
    pub fn slot(&self) -> SlotId {
        match self {
            Self::Bool { slot, .. }
            | Self::Int64 { slot, .. }
            | Self::Float64 { slot, .. }
            | Self::String { slot, .. }
            | Self::Bytes { slot, .. } => *slot,
        }
    }

    /// Physical type of the column.
    pub fn physical_type(&self) -> PhysicalType {
        match self {
            Self::Bool { .. } => PhysicalType::Bool,
            Self::Int64 { .. } => PhysicalType::Int64,
            Self::Float64 { .. } => PhysicalType::Float64,
            Self::String { .. } => PhysicalType::String,
            Self::Bytes { .. } => PhysicalType::Bytes,
        }
    }

    /// Whether the column has no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of values in the column.
    pub fn len(&self) -> usize {
        match self {
            Self::Bool { values, .. } => values.len(),
            Self::Int64 { values, .. } => values.len(),
            Self::Float64 { values, .. } => values.len(),
            Self::String { values, .. } => values.len(),
            Self::Bytes { values, .. } => values.len(),
        }
    }
}

/// Rows of one batch in columnar form.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordBatch {
    /// Number of rows.
    pub num_rows: usize,
    /// Storage key of each row when every row has one (scans, lookups); `None` otherwise.
    pub row_ids: Option<Vec<RowId>>,
    /// One column per output slot that has at least one non-NULL value, in output order.
    /// A column that is NULL in every row is omitted (readers treat a missing column as NULL).
    pub columns: Vec<ColumnVector>,
}

impl RecordBatch {
    /// Number of rows.
    pub fn len(&self) -> usize {
        self.num_rows
    }

    /// Whether the batch has no rows.
    pub fn is_empty(&self) -> bool {
        self.num_rows == 0
    }

    /// Value of `slot` in row `row`, if the batch has that column (else the value is NULL).
    pub fn value(&self, row: usize, slot: SlotId) -> Value {
        let Some(column) = self.columns.iter().find(|column| column.slot() == slot) else {
            return Value::Null;
        };
        match column {
            ColumnVector::Bool { values, .. } => values[row].map_or(Value::Null, Value::Bool),
            ColumnVector::Int64 { values, .. } => values[row].map_or(Value::Null, Value::Int64),
            ColumnVector::Float64 { values, .. } => values[row].map_or(Value::Null, Value::Float64),
            ColumnVector::String { values, .. } => {
                values[row].clone().map_or(Value::Null, Value::String)
            }
            ColumnVector::Bytes { values, .. } => {
                values[row].clone().map_or(Value::Null, Value::Bytes)
            }
        }
    }

    /// Estimates heap bytes owned by this batch for enforcement of the per-query memory limit.
    pub fn estimated_heap_bytes(&self) -> usize {
        let mut total = self
            .row_ids
            .as_ref()
            .map_or(0, Vec::len)
            .saturating_mul(std::mem::size_of::<RowId>());
        for column in &self.columns {
            total = total.saturating_add(match column {
                ColumnVector::Bool { values, .. } => values.len().saturating_mul(2),
                ColumnVector::Int64 { values, .. } => values.len().saturating_mul(16),
                ColumnVector::Float64 { values, .. } => values.len().saturating_mul(16),
                ColumnVector::String { values, .. } => values
                    .iter()
                    .map(|value| {
                        value
                            .as_ref()
                            .map_or(1, |text| text.len().saturating_add(1))
                    })
                    .sum(),
                ColumnVector::Bytes { values, .. } => values
                    .iter()
                    .map(|value| {
                        value
                            .as_ref()
                            .map_or(1, |bytes| bytes.len().saturating_add(1))
                    })
                    .sum(),
            });
        }
        total
    }
    /// Pivots operator rows into the columns of `output`; a slot whose non-null values disagree
    /// in type is an error.
    pub fn from_rows(rows: &[ExecRow], output: &[SlotId]) -> Result<Self, ExecutionError> {
        let row_ids = rows
            .iter()
            .map(|row| row.row_id)
            .collect::<Option<Vec<RowId>>>()
            .filter(|ids| !ids.is_empty());

        let mut columns = Vec::with_capacity(output.len());
        for slot in output {
            if let Some(physical_type) = infer_type(rows, *slot)? {
                columns.push(build_column(rows, *slot, physical_type)?);
            }
        }

        Ok(Self {
            num_rows: rows.len(),
            row_ids,
            columns,
        })
    }
}

/// Physical type of `slot` across `rows`, ignoring NULLs; `None` if every value is NULL.
fn infer_type(rows: &[ExecRow], slot: SlotId) -> Result<Option<PhysicalType>, ExecutionError> {
    let mut found = None;

    for row in rows {
        let candidate = match row.get(slot) {
            Value::Null => continue,
            Value::Bool(_) => PhysicalType::Bool,
            Value::Int64(_) => PhysicalType::Int64,
            Value::Float64(_) => PhysicalType::Float64,
            Value::String(_) => PhysicalType::String,
            Value::Bytes(_) => PhysicalType::Bytes,
        };

        if found.is_some_and(|existing| existing != candidate) {
            return Err(ExecutionError::InconsistentType(slot.0));
        }
        found = Some(candidate);
    }

    Ok(found)
}

/// Collects one typed column; `extract` returns `Ok(None)` for NULL.
fn collect<T>(
    rows: &[ExecRow],
    slot: SlotId,
    extract: impl Fn(&Value) -> Option<T>,
) -> Result<Vec<Option<T>>, ExecutionError> {
    rows.iter()
        .map(|row| match row.get(slot) {
            Value::Null => Ok(None),
            value => extract(value)
                .map(Some)
                .ok_or(ExecutionError::InconsistentType(slot.0)),
        })
        .collect()
}

/// Builds the column vector of `slot` with the inferred physical type.
fn build_column(
    rows: &[ExecRow],
    slot: SlotId,
    physical_type: PhysicalType,
) -> Result<ColumnVector, ExecutionError> {
    Ok(match physical_type {
        PhysicalType::Bool => ColumnVector::Bool {
            slot,
            values: collect(rows, slot, |value| match value {
                Value::Bool(value) => Some(*value),
                _ => None,
            })?,
        },
        PhysicalType::Int64 => ColumnVector::Int64 {
            slot,
            values: collect(rows, slot, |value| match value {
                Value::Int64(value) => Some(*value),
                _ => None,
            })?,
        },
        PhysicalType::Float64 => ColumnVector::Float64 {
            slot,
            values: collect(rows, slot, |value| match value {
                Value::Float64(value) => Some(*value),
                _ => None,
            })?,
        },
        PhysicalType::String => ColumnVector::String {
            slot,
            values: collect(rows, slot, |value| match value {
                Value::String(value) => Some(value.clone()),
                _ => None,
            })?,
        },
        PhysicalType::Bytes => ColumnVector::Bytes {
            slot,
            values: collect(rows, slot, |value| match value {
                Value::Bytes(value) => Some(value.clone()),
                _ => None,
            })?,
        },
    })
}
