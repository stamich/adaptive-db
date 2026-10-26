//! Data format marker: new directories are stamped, older ones are refused until migrated.
mod common;

use adb_core::RowId;
use adb_engine::{
    format::{detect, DATA_FORMAT_VERSION, FORMAT_FILE, LEGACY_DATA_FORMAT_VERSION},
    Database, DbError,
};
use common::{put, value};
use tempfile::tempdir;

/// A new directory is stamped with the current format and reopens normally.
#[test]
fn new_directories_are_stamped() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId::from_primary_key(1, -3), 7);
    db.close().unwrap();
    drop(db);
    assert_eq!(
        detect(dir.path(), &dir.path().join("wal")).unwrap(),
        Some(DATA_FORMAT_VERSION)
    );
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(value(&db, RowId::from_primary_key(1, -3)), Some(7));
}

/// A directory with log records but no marker was written by an engine before 2.3.0 and is
/// refused with a message that names the migration tool.
#[test]
fn unmarked_directories_with_data_are_refused() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        put(&db, RowId::compose(1, 1), 1);
        db.close().unwrap();
    }
    std::fs::remove_file(dir.path().join(FORMAT_FILE)).unwrap();
    let error = Database::open(dir.path())
        .err()
        .expect("format 1 is refused");
    match &error {
        DbError::IncompatibleFormat { found, expected } => {
            assert_eq!(*found, LEGACY_DATA_FORMAT_VERSION);
            assert_eq!(*expected, DATA_FORMAT_VERSION);
        }
        other => panic!("unexpected error {other:?}"),
    }
    assert!(error.to_string().contains("adb-migrate"));
    assert!(!dir.path().join(FORMAT_FILE).exists(), "nothing is stamped");
}

/// An unmarked directory without log records holds no data and is simply stamped.
#[test]
fn unmarked_empty_directories_are_stamped() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("wal")).unwrap();
    Database::open(dir.path()).unwrap();
    assert!(dir.path().join(FORMAT_FILE).exists());
}

/// A damaged marker is reported as corruption, not silently replaced.
#[test]
fn damaged_markers_are_corruption() {
    let dir = tempdir().unwrap();
    Database::open(dir.path()).unwrap();
    std::fs::write(dir.path().join(FORMAT_FILE), b"garbage").unwrap();
    let error = Database::open(dir.path()).err().expect("damaged marker");
    assert!(error.is_corruption(), "{error:?}");
}
