//! The statistics model and its JSON form (shared with the JVM planner).
use adb_core::{FieldId, Value};
use serde::{Deserialize, Serialize};

use crate::StatsError;

/// Version of the statistics document; bumped on incompatible changes.
pub const STATISTICS_FORMAT_VERSION: u32 = 1;
/// Largest statistics document accepted or produced.
pub const MAX_STATISTICS_JSON_BYTES: usize = 4 * 1024 * 1024;
/// Most columns one statistics document may describe.
pub const MAX_COLUMNS: usize = 1024;
/// Most histogram buckets or most common values per column.
pub const MAX_LIST_ENTRIES: usize = 256;

/// Statistics of one entity (table) as of one snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableStatistics {
    /// [`STATISTICS_FORMAT_VERSION`] of the document.
    pub format_version: u32,
    /// Entity described.
    pub entity_id: u64,
    /// Live rows at the snapshot.
    pub row_count: u64,
    /// Mean encoded size of a row in bytes (values plus a per-field overhead).
    pub avg_row_bytes: f64,
    /// Commit timestamp of the snapshot that was analyzed.
    pub analyzed_at_ts: u64,
    /// The entity's modification counter when the snapshot was taken; the planner compares it
    /// with the current counter to judge staleness.
    pub modifications_at_analyze: u64,
    /// Rows in the reservoir sample that histograms and most common values were built from.
    pub sampled_rows: u64,
    /// Whether the sample covered every row (histograms and frequencies are then exact).
    pub exact: bool,
    /// One entry per analyzed field, ordered by field id.
    pub columns: Vec<ColumnStatistics>,
}

/// Statistics of one column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnStatistics {
    /// Field described.
    pub field_id: FieldId,
    /// Rows where the field is NULL or absent.
    pub null_count: u64,
    /// Number of distinct non-NULL values (estimated unless `distinct_exact`).
    pub distinct_count: u64,
    /// Whether `distinct_count` is exact.
    pub distinct_exact: bool,
    /// Smallest non-NULL value (a prefix of at most [`crate::values::MAX_STORED_VALUE_BYTES`]
    /// bytes for long strings and byte strings); absent when values of several types occur.
    pub min: Option<Value>,
    /// Largest non-NULL value (same truncation rule as `min`).
    pub max: Option<Value>,
    /// Mean encoded width of a non-NULL value in bytes.
    pub avg_width_bytes: f64,
    /// Equi-depth histogram over the non-NULL sample values, in ascending order.
    pub histogram: Vec<HistogramBucket>,
    /// Most common values, most frequent first.
    pub most_common: Vec<MostCommonValue>,
}

/// One equi-depth histogram bucket: the values in `[lower, upper]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistogramBucket {
    /// Smallest sample value in the bucket.
    pub lower: Value,
    /// Largest sample value in the bucket.
    pub upper: Value,
    /// Estimated rows of the whole table whose value falls in the bucket.
    pub rows: u64,
    /// Distinct sample values in the bucket.
    pub distinct: u64,
}

/// A value that occurs much more often than the average value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MostCommonValue {
    /// The value.
    pub value: Value,
    /// Estimated rows of the whole table holding it.
    pub rows: u64,
}

impl TableStatistics {
    /// Statistics of a column, if it was analyzed.
    pub fn column(&self, field_id: FieldId) -> Option<&ColumnStatistics> {
        self.columns
            .binary_search_by_key(&field_id, |column| column.field_id)
            .ok()
            .map(|index| &self.columns[index])
    }

    /// Encodes the document as JSON after checking its bounds.
    pub fn to_json(&self) -> Result<Vec<u8>, StatsError> {
        self.validate()?;
        let bytes =
            serde_json::to_vec(self).map_err(|error| StatsError::Format(error.to_string()))?;
        if bytes.len() > MAX_STATISTICS_JSON_BYTES {
            return Err(StatsError::Format(format!(
                "document of {} bytes exceeds {MAX_STATISTICS_JSON_BYTES}",
                bytes.len()
            )));
        }
        Ok(bytes)
    }

    /// Decodes and checks a JSON document.
    pub fn from_json(bytes: &[u8]) -> Result<Self, StatsError> {
        if bytes.len() > MAX_STATISTICS_JSON_BYTES {
            return Err(StatsError::Format(format!(
                "document of {} bytes exceeds {MAX_STATISTICS_JSON_BYTES}",
                bytes.len()
            )));
        }
        let statistics: Self =
            serde_json::from_slice(bytes).map_err(|error| StatsError::Format(error.to_string()))?;
        statistics.validate()?;
        Ok(statistics)
    }

    /// Checks the format version, list bounds, column order and count consistency.
    pub fn validate(&self) -> Result<(), StatsError> {
        let fail = |message: String| Err(StatsError::Format(message));
        if self.format_version != STATISTICS_FORMAT_VERSION {
            return fail(format!(
                "unsupported statistics format {} (expected {STATISTICS_FORMAT_VERSION})",
                self.format_version
            ));
        }
        if self.columns.len() > MAX_COLUMNS {
            return fail(format!("more than {MAX_COLUMNS} columns"));
        }
        if !self.avg_row_bytes.is_finite() || self.avg_row_bytes < 0.0 {
            return fail("avg_row_bytes must be a finite, non-negative number".into());
        }
        if !self
            .columns
            .windows(2)
            .all(|pair| pair[0].field_id < pair[1].field_id)
        {
            return fail("columns must be ordered by field id without duplicates".into());
        }
        for column in &self.columns {
            if column.null_count > self.row_count {
                return fail(format!("field {}: more NULLs than rows", column.field_id));
            }
            if column.histogram.len() > MAX_LIST_ENTRIES
                || column.most_common.len() > MAX_LIST_ENTRIES
            {
                return fail(format!(
                    "field {}: more than {MAX_LIST_ENTRIES} histogram buckets or common values",
                    column.field_id
                ));
            }
            if !column.avg_width_bytes.is_finite() || column.avg_width_bytes < 0.0 {
                return fail(format!("field {}: invalid average width", column.field_id));
            }
            let values = column
                .min
                .iter()
                .chain(&column.max)
                .chain(column.histogram.iter().flat_map(|b| [&b.lower, &b.upper]))
                .chain(column.most_common.iter().map(|c| &c.value));
            if values
                .into_iter()
                .any(|value| matches!(value, Value::Float64(v) if !v.is_finite()))
            {
                return fail(format!(
                    "field {}: non-finite floats have no JSON form",
                    column.field_id
                ));
            }
        }
        Ok(())
    }
}
