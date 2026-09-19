//! Module `integrity` for crate `adb-storage`.
use std::collections::BTreeMap;

use adb_core::RowId;

use crate::{PersistentCurrentStore, PersistentVersionStore, StorageError};

/// Represents `IntegrityReport` state used by this subsystem.
#[derive(Debug, Default)]
pub struct IntegrityReport {
    pub current_rows: usize,
    pub historical_versions: usize,
    pub errors: Vec<String>,
}

/// Implements behavior for `IntegrityReport`.
impl IntegrityReport {
    /// Implements the `is_ok` operation used by this subsystem.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Represents `IntegrityChecker` state used by this subsystem.
pub struct IntegrityChecker;

/// Implements behavior for `IntegrityChecker`.
impl IntegrityChecker {
    /// Implements the `verify` operation used by this subsystem.
    pub fn verify(
        current: &PersistentCurrentStore,
        versions: &PersistentVersionStore,
    ) -> Result<IntegrityReport, StorageError> {
        let current_entries = current.entries()?;
        let version_entries = versions.scan_all()?;

        let mut report = IntegrityReport {
            current_rows: current_entries.len(),
            historical_versions: version_entries.len(),
            errors: Vec::new(),
        };

        let mut by_row: BTreeMap<RowId, Vec<_>> = BTreeMap::new();

        for (key, version) in version_entries {
            if key.begin_ts != version.begin_ts {
                report.errors.push(format!(
                    "version key beginTs {:?} differs from payload {:?} for row {:?}",
                    key.begin_ts, version.begin_ts, key.row_id
                ));
            }

            if version.begin_ts >= version.end_ts {
                report.errors.push(format!(
                    "invalid interval [{:?}, {:?}) for row {:?}",
                    version.begin_ts, version.end_ts, key.row_id
                ));
            }

            by_row.entry(key.row_id).or_default().push(version);
        }

        for (row_id, mut row_versions) in by_row {
            row_versions.sort_by_key(|version| version.begin_ts);

            for pair in row_versions.windows(2) {
                if pair[0].end_ts > pair[1].begin_ts {
                    report.errors.push(format!(
                        "overlapping history for row {:?}: [{:?},{:?}) and [{:?},{:?})",
                        row_id, pair[0].begin_ts, pair[0].end_ts, pair[1].begin_ts, pair[1].end_ts,
                    ));
                }
            }
        }

        Ok(report)
    }
}
