//! Columnar record batches produced by query cursors.
use std::collections::BTreeSet;

use adb_core::{FieldId, Row, RowId, Value};

use crate::ExecutionError;

/// Physical type of a column vector; the discriminant is the wire type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PhysicalType {
    Bool = 1,
    Int64 = 2,
    Float64 = 3,
    String = 4,
    Bytes = 5,
}

/// One typed, nullable column of a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnVector {
    Bool {
        field_id: FieldId,
        values: Vec<Option<bool>>,
    },
    Int64 {
        field_id: FieldId,
        values: Vec<Option<i64>>,
    },
    Float64 {
        field_id: FieldId,
        values: Vec<Option<f64>>,
    },
    String {
        field_id: FieldId,
        values: Vec<Option<String>>,
    },
    Bytes {
        field_id: FieldId,
        values: Vec<Option<Vec<u8>>>,
    },
}

impl ColumnVector {
    /// Field the column holds.
    pub fn field_id(&self) -> FieldId {
        match self {
            Self::Bool { field_id, .. }
            | Self::Int64 { field_id, .. }
            | Self::Float64 { field_id, .. }
            | Self::String { field_id, .. }
            | Self::Bytes { field_id, .. } => *field_id,
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

/// Rows of one batch in columnar form: row ids plus one vector per field present.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordBatch {
    pub row_ids: Vec<RowId>,
    pub columns: Vec<ColumnVector>,
}

impl RecordBatch {
    /// Number of rows.
    pub fn len(&self) -> usize {
        self.row_ids.len()
    }

    /// Whether the batch has no rows.
    pub fn is_empty(&self) -> bool {
        self.row_ids.is_empty()
    }

    /// Estimates heap bytes owned by this batch for enforcement of the per-query memory limit.
    pub fn estimated_heap_bytes(&self) -> usize {
        let mut total = self
            .row_ids
            .len()
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
    /// Pivots rows into columns; a field whose non-null values disagree in type is an error.
    pub fn from_rows(rows: &[(RowId, Row)]) -> Result<Self, ExecutionError> {
        let mut fields = BTreeSet::new();
        for (_, row) in rows {
            fields.extend(row.fields.keys().copied());
        }

        let row_ids = rows.iter().map(|(row_id, _)| *row_id).collect();
        let mut columns = Vec::with_capacity(fields.len());

        for field_id in fields {
            let physical_type = infer_type(rows, field_id)?;
            if let Some(physical_type) = physical_type {
                columns.push(build_column(rows, field_id, physical_type)?);
            }
        }

        Ok(Self { row_ids, columns })
    }
}

/// Physical type of `field_id` across `rows`, ignoring nulls and absent values.
fn infer_type(
    rows: &[(RowId, Row)],
    field_id: FieldId,
) -> Result<Option<PhysicalType>, ExecutionError> {
    let mut found = None;

    for (_, row) in rows {
        let Some(value) = row.get(field_id) else {
            continue;
        };
        let candidate = match value {
            Value::Null => continue,
            Value::Bool(_) => PhysicalType::Bool,
            Value::Int64(_) => PhysicalType::Int64,
            Value::Float64(_) => PhysicalType::Float64,
            Value::String(_) => PhysicalType::String,
            Value::Bytes(_) => PhysicalType::Bytes,
        };

        if found.is_some_and(|existing| existing != candidate) {
            return Err(ExecutionError::InconsistentType(field_id));
        }
        found = Some(candidate);
    }

    Ok(found)
}

/// Builds the column vector of `field_id` with the inferred physical type.
fn build_column(
    rows: &[(RowId, Row)],
    field_id: FieldId,
    physical_type: PhysicalType,
) -> Result<ColumnVector, ExecutionError> {
    match physical_type {
        PhysicalType::Bool => Ok(ColumnVector::Bool {
            field_id,
            values: rows
                .iter()
                .map(|(_, row)| match row.get(field_id) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::Bool(value)) => Ok(Some(*value)),
                    _ => Err(ExecutionError::InconsistentType(field_id)),
                })
                .collect::<Result<_, _>>()?,
        }),
        PhysicalType::Int64 => Ok(ColumnVector::Int64 {
            field_id,
            values: rows
                .iter()
                .map(|(_, row)| match row.get(field_id) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::Int64(value)) => Ok(Some(*value)),
                    _ => Err(ExecutionError::InconsistentType(field_id)),
                })
                .collect::<Result<_, _>>()?,
        }),
        PhysicalType::Float64 => Ok(ColumnVector::Float64 {
            field_id,
            values: rows
                .iter()
                .map(|(_, row)| match row.get(field_id) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::Float64(value)) => Ok(Some(*value)),
                    _ => Err(ExecutionError::InconsistentType(field_id)),
                })
                .collect::<Result<_, _>>()?,
        }),
        PhysicalType::String => Ok(ColumnVector::String {
            field_id,
            values: rows
                .iter()
                .map(|(_, row)| match row.get(field_id) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::String(value)) => Ok(Some(value.clone())),
                    _ => Err(ExecutionError::InconsistentType(field_id)),
                })
                .collect::<Result<_, _>>()?,
        }),
        PhysicalType::Bytes => Ok(ColumnVector::Bytes {
            field_id,
            values: rows
                .iter()
                .map(|(_, row)| match row.get(field_id) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::Bytes(value)) => Ok(Some(value.clone())),
                    _ => Err(ExecutionError::InconsistentType(field_id)),
                })
                .collect::<Result<_, _>>()?,
        }),
    }
}
