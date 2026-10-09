//! Options and limits of one `ANALYZE` run.
use adb_core::FieldId;
use serde::{Deserialize, Serialize};

use crate::{model::MAX_COLUMNS, model::MAX_LIST_ENTRIES, StatsError};

/// What to analyze and the bounds of the run. Every field has a default, so `{}` is a valid
/// JSON form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnalyzeOptions {
    /// Fields to analyze; `None` analyzes every field found (at most 1024).
    pub fields: Option<Vec<FieldId>>,
    /// Reservoir sample size for histograms and most common values (1 ..= 1,000,000).
    pub sample_rows: usize,
    /// Histogram buckets per column (0 ..= 256; 0 disables histograms).
    pub histogram_buckets: usize,
    /// Most common values kept per column (0 ..= 256).
    pub most_common_values: usize,
    /// Rows scanned before the run fails with a limit error.
    pub max_rows: u64,
    /// Bytes the run may hold (sample plus distinct-value sets) before it fails.
    pub max_working_set_bytes: usize,
    /// Wall-clock budget in milliseconds; `None` means unbounded.
    pub deadline_ms: Option<u64>,
}

impl Default for AnalyzeOptions {
    /// 30,000 sampled rows, 64 buckets, 32 common values, 100M rows, 64 MiB, no deadline.
    fn default() -> Self {
        Self {
            fields: None,
            sample_rows: 30_000,
            histogram_buckets: 64,
            most_common_values: 32,
            max_rows: 100_000_000,
            max_working_set_bytes: 64 * 1024 * 1024,
            deadline_ms: None,
        }
    }
}

impl AnalyzeOptions {
    /// Decodes options from JSON (empty input means the defaults).
    pub fn from_json(bytes: &[u8]) -> Result<Self, StatsError> {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        let options: Self = serde_json::from_slice(bytes)
            .map_err(|error| StatsError::InvalidOptions(error.to_string()))?;
        options.validate()?;
        Ok(options)
    }

    /// Checks every option against its accepted range.
    pub fn validate(&self) -> Result<(), StatsError> {
        let fail = |message: String| Err(StatsError::InvalidOptions(message));
        if !(1..=1_000_000).contains(&self.sample_rows) {
            return fail("sample_rows must be 1..=1000000".into());
        }
        if self.histogram_buckets > MAX_LIST_ENTRIES || self.most_common_values > MAX_LIST_ENTRIES {
            return fail(format!(
                "histogram_buckets and most_common_values must be at most {MAX_LIST_ENTRIES}"
            ));
        }
        if self.max_rows == 0 || self.max_working_set_bytes == 0 {
            return fail("max_rows and max_working_set_bytes must be positive".into());
        }
        if let Some(fields) = &self.fields {
            if fields.len() > MAX_COLUMNS {
                return fail(format!("more than {MAX_COLUMNS} fields"));
            }
        }
        Ok(())
    }
}
