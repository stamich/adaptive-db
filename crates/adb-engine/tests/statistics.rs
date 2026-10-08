//! Optimizer statistics in the engine: `ANALYZE`, persistence and modification counters
//! across checkpoints, crashes and projection rebuilds.

mod common;

use adb_core::{RowId, Value};
use adb_engine::{AnalyzeOptions, Database, DatabaseOptions, DbError, StatsError};
use tempfile::tempdir;

use common::{delete, put};

/// Entity used by the tests.
const ENTITY: u64 = 5;

/// Key `pk` of [`ENTITY`].
fn key(pk: u64) -> RowId {
    RowId::compose(ENTITY, pk)
}

/// Commits `count` rows with primary keys `first..first + count`, value `pk % 10`.
fn load(db: &Database, first: u64, count: u64) {
    for pk in first..first + count {
        put(db, key(pk), (pk % 10) as i64);
    }
}

/// `ANALYZE` describes the latest snapshot and its document survives a reopen.
#[test]
fn analyze_persists_document() {
    let dir = tempdir().unwrap();
    let analyzed = {
        let db = Database::open(dir.path()).unwrap();
        load(&db, 0, 50);
        put(&db, RowId::compose(ENTITY + 1, 0), 1); // another entity is not counted
        let statistics = db.analyze(ENTITY, &AnalyzeOptions::default()).unwrap();
        assert_eq!(statistics.row_count, 50);
        assert_eq!(statistics.analyzed_at_ts, db.latest_committed_ts().0);
        let column = statistics.column(1).unwrap();
        assert_eq!(column.distinct_count, 10);
        assert_eq!(column.min, Some(Value::Int64(0)));
        assert_eq!(column.max, Some(Value::Int64(9)));
        assert_eq!(db.statistics(ENTITY), Some(statistics.clone()));
        assert_eq!(db.statistics(ENTITY + 1), None);
        db.close().unwrap();
        statistics
    };
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.statistics(ENTITY), Some(analyzed));
}

/// Every committed mutation counts, `ANALYZE` resets the delta, and the counters survive a
/// crash (log replay) and a clean shutdown (checkpoint) alike.
#[test]
fn modification_counters_track_commits() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        load(&db, 0, 20);
        delete(&db, key(0));
        assert_eq!(db.modifications_since_analyze(ENTITY), 21);
        db.analyze(ENTITY, &AnalyzeOptions::default()).unwrap();
        assert_eq!(db.modifications_since_analyze(ENTITY), 0);
        load(&db, 100, 7);
        assert_eq!(db.modifications_since_analyze(ENTITY), 7);
        db.checkpoint().unwrap();
        load(&db, 200, 3);
        // Dropped without a checkpoint: the last 3 commits are recovered from the log.
    }
    {
        let db = Database::open(dir.path()).unwrap();
        assert_eq!(db.modifications_since_analyze(ENTITY), 10);
        db.close().unwrap();
    }
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.modifications_since_analyze(ENTITY), 10);
}

/// Rebuilding the projections recounts the mutations from the complete log.
#[test]
fn rebuild_recounts_modifications() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        load(&db, 0, 12);
        db.analyze(ENTITY, &AnalyzeOptions::default()).unwrap();
        load(&db, 50, 4);
        db.close().unwrap();
    }
    let db = Database::rebuild_projections(dir.path(), DatabaseOptions::default()).unwrap();
    assert_eq!(db.modifications_since_analyze(ENTITY), 4);
    assert_eq!(db.statistics(ENTITY).unwrap().row_count, 12);
}

/// A damaged statistics document is ignored (derived data); the database still opens.
#[test]
fn damaged_document_counts_as_missing() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        load(&db, 0, 5);
        db.analyze(ENTITY, &AnalyzeOptions::default()).unwrap();
        db.close().unwrap();
    }
    let path = dir
        .path()
        .join("stats")
        .join(format!("entity-{ENTITY}.stats"));
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();

    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.statistics(ENTITY), None);
    assert_eq!(db.modifications_since_analyze(ENTITY), 5);
    db.analyze(ENTITY, &AnalyzeOptions::default()).unwrap();
    assert_eq!(db.modifications_since_analyze(ENTITY), 0);
}

/// A damaged modification counters file is checkpointed state: it is reported as corruption
/// and repaired by rebuilding the projections.
#[test]
fn damaged_counters_are_corruption() {
    let dir = tempdir().unwrap();
    {
        let db = Database::open(dir.path()).unwrap();
        load(&db, 0, 3);
        db.close().unwrap();
    }
    let path = dir.path().join("stats").join("modifications.meta");
    std::fs::write(&path, b"garbage").unwrap();
    let error = Database::open(dir.path()).err().unwrap();
    assert!(error.is_corruption(), "{error}");

    let db = Database::rebuild_projections(dir.path(), DatabaseOptions::default()).unwrap();
    assert_eq!(db.modifications_since_analyze(ENTITY), 3);
}

/// Option and resource failures are statistics errors and leave no document behind.
#[test]
fn analyze_failures_are_reported() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    load(&db, 0, 10);
    let limited = AnalyzeOptions {
        max_rows: 5,
        ..AnalyzeOptions::default()
    };
    assert!(matches!(
        db.analyze(ENTITY, &limited),
        Err(DbError::Statistics(StatsError::Limit(_)))
    ));
    let invalid = AnalyzeOptions {
        sample_rows: 0,
        ..AnalyzeOptions::default()
    };
    assert!(matches!(
        db.analyze(ENTITY, &invalid),
        Err(DbError::Statistics(StatsError::InvalidOptions(_)))
    ));
    assert_eq!(db.statistics(ENTITY), None);

    let empty = db.analyze(ENTITY + 9, &AnalyzeOptions::default()).unwrap();
    assert_eq!(empty.row_count, 0);
}
