//! Module `persistent_version` for crate `adb-storage`.
use adb_core::{CommitTs, Lsn, Row, RowId, Value};
use adb_storage::{HistoricalVersion, PersistentVersionStore};
use tempfile::tempdir;

/// Implements the `row` operation used by this subsystem.
fn row(value: i64) -> Row {
    Row::new().with_field(1, Value::Int64(value))
}

/// Implements the `persistent_version_store_survives_restart` operation used by this subsystem.
#[test]
fn persistent_version_store_survives_restart() {
    let dir = tempdir().unwrap();

    {
        let store = PersistentVersionStore::open(dir.path()).unwrap();

        store
            .put_at_lsn(
                RowId(7),
                &HistoricalVersion {
                    begin_ts: CommitTs(10),
                    end_ts: CommitTs(20),
                    value: Some(row(100)),
                },
                Lsn(10),
            )
            .unwrap();

        store
            .put_at_lsn(
                RowId(7),
                &HistoricalVersion {
                    begin_ts: CommitTs(20),
                    end_ts: CommitTs(30),
                    value: Some(row(200)),
                },
                Lsn(20),
            )
            .unwrap();

        store.flush().unwrap();
    }

    {
        let store = PersistentVersionStore::open(dir.path()).unwrap();

        let v1 = store.get_at(RowId(7), CommitTs(15)).unwrap().unwrap();

        let v2 = store.get_at(RowId(7), CommitTs(25)).unwrap().unwrap();

        assert_eq!(v1.value.unwrap().get(1), Some(&Value::Int64(100)));

        assert_eq!(v2.value.unwrap().get(1), Some(&Value::Int64(200)));

        assert_eq!(store.history(RowId(7)).unwrap().len(), 2);
    }
}
