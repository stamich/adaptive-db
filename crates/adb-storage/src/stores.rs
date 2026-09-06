//! Stores module for the adb-storage crate.
//!
use adb_core::{CommitTs, Row, RowId};

use crate::{CurrentRecord, CurrentStore, HistoricalVersion, VersionStore};

/// Groups the current-state and historical-version stores used by the MVCC engine.
#[derive(Debug, Default)]
pub struct Stores {
    pub current: CurrentStore,
    pub versions: VersionStore,
}

/// Implements behavior for `Stores`.
impl Stores {
    /// Reads the `at` value from the binary representation.
    pub fn read_at(&self, row_id: RowId, ts: CommitTs) -> Option<Row> {
        if let Some(current) = self.current.get(row_id) {
            if current.commit_ts <= ts {
                return current.value.clone();
            }
        }

        self.versions
            .get_at(row_id, ts)
            .and_then(|version| version.value.clone())
    }

    /// Applies the `put` operation to the relevant storage state.
    pub fn apply_put(&mut self, row_id: RowId, value: Row, commit_ts: CommitTs) {
        if let Some(old) = self.current.remove(row_id) {
            self.versions.push(
                row_id,
                HistoricalVersion {
                    begin_ts: old.commit_ts,
                    end_ts: commit_ts,
                    value: old.value,
                },
            );
        }

        self.current.insert(
            row_id,
            CurrentRecord {
                commit_ts,
                value: Some(value),
            },
        );
    }

    /// Applies the `delete` operation to the relevant storage state.
    pub fn apply_delete(&mut self, row_id: RowId, commit_ts: CommitTs) {
        if let Some(old) = self.current.remove(row_id) {
            self.versions.push(
                row_id,
                HistoricalVersion {
                    begin_ts: old.commit_ts,
                    end_ts: commit_ts,
                    value: old.value,
                },
            );
        }

        self.current.insert(
            row_id,
            CurrentRecord {
                commit_ts,
                value: None,
            },
        );
    }
}
