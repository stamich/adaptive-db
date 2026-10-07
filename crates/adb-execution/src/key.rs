//! Hashable keys for hash joins and grouping.
//!
//! [`adb_core::Value`] is not `Eq + Hash` (it holds `f64`), so keys are normalized into
//! [`KeyPart`]s: `-0.0` and `0.0` become one key, and every NaN becomes one canonical NaN.
use adb_core::Value;

use crate::{ExecRow, SlotId};

/// One normalized component of a composite key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyPart {
    /// SQL NULL (only used by grouping; join keys containing NULL never match).
    Null,
    /// Boolean.
    Bool(bool),
    /// Integer.
    Int64(i64),
    /// Bit pattern of a normalized double.
    Float64(u64),
    /// String.
    String(String),
    /// Byte string.
    Bytes(Vec<u8>),
}

impl KeyPart {
    /// Normalizes one value.
    pub fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(value) => Self::Bool(*value),
            Value::Int64(value) => Self::Int64(*value),
            Value::Float64(value) if value.is_nan() => Self::Float64(f64::NAN.to_bits()),
            Value::Float64(value) if *value == 0.0 => Self::Float64(0.0f64.to_bits()),
            Value::Float64(value) => Self::Float64(value.to_bits()),
            Value::String(value) => Self::String(value.clone()),
            Value::Bytes(value) => Self::Bytes(value.clone()),
        }
    }

    /// The value this key part stands for (used to emit group keys).
    pub fn to_value(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(*value),
            Self::Int64(value) => Value::Int64(*value),
            Self::Float64(bits) => Value::Float64(f64::from_bits(*bits)),
            Self::String(value) => Value::String(value.clone()),
            Self::Bytes(value) => Value::Bytes(value.clone()),
        }
    }

    /// Approximate heap footprint, for memory accounting.
    pub fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::String(value) => value.len(),
                Self::Bytes(value) => value.len(),
                _ => 0,
            }
    }
}

/// Grouping key of `row` over `slots`; NULLs form their own group (SQL GROUP BY semantics).
pub fn group_key(row: &ExecRow, slots: &[SlotId]) -> Vec<KeyPart> {
    slots
        .iter()
        .map(|slot| KeyPart::of(row.get(*slot)))
        .collect()
}

/// Join key of `row` over `slots`, or `None` if any component is NULL or NaN: such a key
/// cannot be equal to anything under SQL comparison semantics.
pub fn join_key(row: &ExecRow, slots: &[SlotId]) -> Option<Vec<KeyPart>> {
    slots
        .iter()
        .map(|slot| match row.get(*slot) {
            Value::Null => None,
            Value::Float64(value) if value.is_nan() => None,
            value => Some(KeyPart::of(value)),
        })
        .collect()
}
