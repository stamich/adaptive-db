//! Migration of a data format 1 directory (raw primary-key bits) to format 2.
mod common;

use adb_core::{KeyRange, RowId};
use adb_engine::{
    format::{detect, DATA_FORMAT_VERSION, FORMAT_FILE},
    migrate, migrate_in_place, AnalyzeOptions, ChangeCursor, Database, DbError,
};
use adb_execution::DataSource;
use common::{delete, put, read_i64, value};
use tempfile::tempdir;

/// Entity of the test rows.
const ENTITY: u64 = 4;

/// The format-1 storage key of a primary key: its raw two's-complement bits.
fn legacy(primary_key: i64) -> RowId {
    RowId::compose(ENTITY, primary_key as u64)
}

/// Writes a format-1 database into `dir`: rows with negative and positive keys, an update
/// history, a delete, a statistics document and a consumer offset; returns the offset.
fn legacy_database(dir: &std::path::Path) -> ChangeCursor {
    let db = Database::open(dir).unwrap();
    for key in [-20, -1, 0, 3, 7] {
        put(&db, legacy(key), key * 10);
    }
    put(&db, legacy(3), 31);
    delete(&db, legacy(7));
    db.analyze(ENTITY, &AnalyzeOptions::default()).unwrap();
    let offset = db.change_feed_end();
    db.commit_consumer_offset("indexer", offset).unwrap();
    db.close().unwrap();
    drop(db);
    std::fs::remove_file(dir.join(FORMAT_FILE)).unwrap();
    offset
}

/// Rows, history, statistics and consumer offsets survive; keys now sort in value order.
#[test]
fn migration_rewrites_keys_and_keeps_everything_else() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    let offset = legacy_database(&source);

    let report = migrate(&source, &target).unwrap();
    assert!(report.records > 0);
    assert_eq!(report.statistics_documents, 1);
    assert_eq!(report.rows_per_entity.get(&ENTITY), Some(&4));
    assert_eq!(
        detect(&source, &source.join("wal")).unwrap(),
        Some(1),
        "the source is untouched"
    );
    assert_eq!(
        detect(&target, &target.join("wal")).unwrap(),
        Some(DATA_FORMAT_VERSION)
    );

    let db = Database::open(&target).unwrap();
    for (key, expected) in [
        (-20, Some(-200)),
        (-1, Some(-10)),
        (0, Some(0)),
        (3, Some(31)),
        (7, None),
    ] {
        assert_eq!(
            value(&db, RowId::from_primary_key(ENTITY, key)),
            expected,
            "key {key}"
        );
    }
    let history = db.history(RowId::from_primary_key(ENTITY, 3)).unwrap();
    assert_eq!(history.len(), 2, "the update history is kept");
    let scanned: Vec<i64> = db
        .scan_page(
            &KeyRange::entity(ENTITY),
            None,
            100,
            db.latest_committed_ts().into(),
        )
        .unwrap()
        .into_iter()
        .map(|(row_id, row)| {
            let key = row_id.signed_primary_key();
            assert_eq!(read_i64(&row), if key == 3 { 31 } else { key * 10 });
            key
        })
        .collect();
    assert_eq!(scanned, vec![-20, -1, 0, 3], "key order is value order");
    assert_eq!(
        db.consumer_offset("indexer"),
        Some(offset),
        "log positions are unchanged"
    );
    assert!(db.statistics(ENTITY).is_some());
    assert_eq!(
        db.modifications_since_analyze(ENTITY),
        0,
        "counters are recounted from the log"
    );
}

/// In-place migration keeps the original next to the migrated directory.
#[test]
fn in_place_migration_keeps_the_original() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    legacy_database(&data);
    let (report, backup) = migrate_in_place(&data).unwrap();
    assert_eq!(report.rows_per_entity.get(&ENTITY), Some(&4));
    assert_eq!(backup, dir.path().join("data.format1"));
    assert!(backup.join("wal").exists());
    let db = Database::open(&data).unwrap();
    assert_eq!(value(&db, RowId::from_primary_key(ENTITY, -1)), Some(-10));
}

/// Only unmarked format-1 directories with a log are migrated, into an empty target.
#[test]
fn migration_refuses_wrong_inputs() {
    let dir = tempdir().unwrap();
    let current = dir.path().join("current");
    Database::open(&current).unwrap();
    assert!(matches!(
        migrate(&current, &dir.path().join("t1")),
        Err(DbError::InvalidArgument(_))
    ));
    assert!(matches!(
        migrate(&dir.path().join("missing"), &dir.path().join("t2")),
        Err(DbError::InvalidArgument(_))
    ));
    // Format 2 (2.3.0) is upgraded on open, never migrated.
    let format_two = dir.path().join("two");
    Database::open(&format_two).unwrap().close().unwrap();
    adb_engine::format::stamp(&format_two, 2).unwrap();
    let refused = migrate(&format_two, &dir.path().join("t3")).unwrap_err();
    assert!(
        refused.to_string().contains("upgrades in place"),
        "{refused}"
    );
    let source = dir.path().join("source");
    legacy_database(&source);
    std::fs::create_dir_all(dir.path().join("used")).unwrap();
    std::fs::write(dir.path().join("used/file"), b"x").unwrap();
    assert!(matches!(
        migrate(&source, &dir.path().join("used")),
        Err(DbError::InvalidArgument(_))
    ));
}
