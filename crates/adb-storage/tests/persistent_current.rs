//! Persistent Current module for the adb-storage crate.
//!
use adb_core::{CommitTs, Row, RowId, Value};
use adb_storage::{CurrentRecord, PersistentCurrentStore};
use tempfile::tempdir;

/// Implements the `current_store_survives_reopen` operation used by this subsystem.
#[test]
fn current_store_survives_reopen() {
    let dir = tempdir().unwrap();
    {
        let store = PersistentCurrentStore::open(dir.path()).unwrap();
        let record = CurrentRecord {
            commit_ts: CommitTs(7),
            value: Some(Row::new().with_field(1, Value::Int64(42))),
        };
        store.put(RowId(9), &record).unwrap();
        store.flush().unwrap();
    }
    {
        let store = PersistentCurrentStore::open(dir.path()).unwrap();
        let got = store.get(RowId(9)).unwrap().unwrap();
        assert_eq!(got.commit_ts, CommitTs(7));
    }
}
