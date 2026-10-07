//! Row ordering shared by Sort and TopK.
//!
//! Comparisons between values can fail (mixed types, NaN). Rust's sort requires a total order
//! and may panic on an inconsistent comparator, so the rows are first checked with
//! [`check_sortable`]; afterwards [`compare_rows`] is total and infallible.
use std::cmp::Ordering;

use adb_core::Value;

use crate::{expression::compare, ExecRow, ExecutionError, SortKey};

/// Fails unless, for every key, all non-NULL values share one comparable type and none is NaN.
pub fn check_sortable(rows: &[ExecRow], keys: &[SortKey]) -> Result<(), ExecutionError> {
    for key in keys {
        let mut first: Option<&Value> = None;
        for row in rows {
            let value = row.get(key.slot);
            match value {
                Value::Null => continue,
                Value::Float64(number) if number.is_nan() => {
                    return Err(ExecutionError::Expression(format!(
                        "cannot sort NaN in slot {}",
                        key.slot
                    )))
                }
                _ => {}
            }
            match first {
                None => first = Some(value),
                Some(first) => {
                    compare(first, value)?;
                }
            }
        }
    }
    Ok(())
}

/// Orders two rows by `keys` (NULLs last ascending, first descending). Total for rows that
/// passed [`check_sortable`].
pub fn compare_rows(a: &ExecRow, b: &ExecRow, keys: &[SortKey]) -> Ordering {
    for key in keys {
        let ordering = match (a.get(key.slot), b.get(key.slot)) {
            (Value::Null, Value::Null) => Ordering::Equal,
            // NULL is treated as larger than every value ...
            (Value::Null, _) => Ordering::Greater,
            (_, Value::Null) => Ordering::Less,
            (left, right) => compare(left, right).unwrap_or(Ordering::Equal),
        };
        // ... so reversing the order for DESC also puts NULLs first.
        let ordering = if key.descending {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// Checks and stably sorts `rows` by `keys`.
pub fn sort_rows(rows: &mut [ExecRow], keys: &[SortKey]) -> Result<(), ExecutionError> {
    check_sortable(rows, keys)?;
    rows.sort_by(|a, b| compare_rows(a, b, keys));
    Ok(())
}
