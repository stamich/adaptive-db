//! Offline consistency checks across both projections.
use std::collections::BTreeMap;

use adb_core::{CommitTs, RowId};

use crate::{PersistentCurrentStore, PersistentVersionStore, StorageError};

/// Result of [`IntegrityChecker::verify`].
#[derive(Debug, Default)]
pub struct IntegrityReport {
    /// Current-store entries examined.
    pub current_rows: usize,
    /// Historical versions examined.
    pub historical_versions: usize,
    /// Human-readable violations; empty when consistent.
    pub errors: Vec<String>,
}

impl IntegrityReport {
    /// Whether no violation was found.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Verifies temporal invariants:
/// * every version key matches its payload and has a non-empty interval;
/// * the versions of a row do not overlap;
/// * no version of a row ends after the row's current state began.
pub struct IntegrityChecker;

impl IntegrityChecker {
    /// Runs every check. Reads all tuples, so every heap page and index entry is also
    /// checksum- and structure-validated on the way.
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
                    "version key {:?} differs from payload begin {:?}",
                    key, version.begin_ts
                ));
            }
            if version.begin_ts >= version.end_ts {
                report.errors.push(format!(
                    "empty interval [{:?}, {:?}) for row {:?}",
                    version.begin_ts, version.end_ts, key.row_id
                ));
            }
            by_row.entry(key.row_id).or_default().push(version);
        }

        let current_begin: BTreeMap<RowId, CommitTs> = current_entries
            .into_iter()
            .map(|(row_id, record)| (row_id, record.commit_ts))
            .collect();
        for (row_id, mut history) in by_row {
            history.sort_by_key(|version| version.begin_ts);
            for pair in history.windows(2) {
                if pair[0].end_ts > pair[1].begin_ts {
                    report.errors.push(format!(
                        "overlapping history for row {row_id:?}: [{:?},{:?}) and [{:?},{:?})",
                        pair[0].begin_ts, pair[0].end_ts, pair[1].begin_ts, pair[1].end_ts
                    ));
                }
            }
            if let (Some(last), Some(begin)) = (history.last(), current_begin.get(&row_id)) {
                if last.end_ts > *begin {
                    report.errors.push(format!(
                        "history of row {row_id:?} ends at {:?} after current state began at {begin:?}",
                        last.end_ts
                    ));
                }
            }
        }
        Ok(report)
    }
}
