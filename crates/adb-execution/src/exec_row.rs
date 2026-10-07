//! Operator rows addressed by [`SlotId`].
use adb_core::{Row, RowId, Value};

use crate::{ScanColumn, SlotId};

/// One row flowing between operators.
///
/// `values` has one entry per slot of the query (the plan's slot width), so reading or writing a
/// slot is an index operation. Slots an operator does not produce stay [`Value::Null`]; a join
/// output therefore is simply the left row with the right side's slots copied in.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecRow {
    /// Storage key when the row comes straight from a scan or lookup; `None` for derived rows
    /// (join and aggregate output), which have no single storage identity.
    pub row_id: Option<RowId>,
    /// Value of every slot, indexed by [`SlotId::index`].
    pub values: Vec<Value>,
}

/// Rows exchanged between operators in one pull.
pub type RowBatch = Vec<ExecRow>;

impl ExecRow {
    /// A row of `width` NULL slots without a storage key.
    pub fn nulls(width: usize) -> Self {
        Self {
            row_id: None,
            values: vec![Value::Null; width],
        }
    }

    /// Maps a stored row into slots: each `columns[i].field_id` lands in `columns[i].slot`;
    /// fields that are absent in the stored row are NULL.
    pub fn from_stored(row_id: RowId, row: &Row, columns: &[ScanColumn], width: usize) -> Self {
        let mut out = Self::nulls(width);
        out.row_id = Some(row_id);
        for column in columns {
            if let Some(value) = row.get(column.field_id) {
                out.values[column.slot.index()] = value.clone();
            }
        }
        out
    }

    /// Value of `slot` (validated plans never address a slot outside the row width).
    pub fn get(&self, slot: SlotId) -> &Value {
        &self.values[slot.index()]
    }

    /// Replaces the value of `slot`.
    pub fn set(&mut self, slot: SlotId, value: Value) {
        self.values[slot.index()] = value;
    }

    /// Copies the values of `slots` from `other` (used to append a join's right side).
    pub fn copy_slots_from(&mut self, other: &ExecRow, slots: &[SlotId]) {
        for slot in slots {
            self.values[slot.index()] = other.values[slot.index()].clone();
        }
    }

    /// Approximate heap footprint, used for query memory accounting.
    pub fn estimated_bytes(&self) -> usize {
        let base = std::mem::size_of::<Self>()
            .saturating_add(self.values.len() * std::mem::size_of::<Value>());
        self.values.iter().fold(base, |total, value| {
            total.saturating_add(match value {
                Value::String(text) => text.len(),
                Value::Bytes(bytes) => bytes.len(),
                _ => 0,
            })
        })
    }
}
