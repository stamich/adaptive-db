//! The engine registers its synchronous projections with the projection runtime and keeps their
//! frontiers on the commit order; retention keeps what slow consumers need.
mod common;

use adb_change_log::ChangeRetentionPolicy;
use adb_core::{CommitSeq, Direction, RowId};
use adb_engine::{
    projection_runtime::{
        index_projection_id, CURRENT_ROW_PROJECTION, CURRENT_ROW_PROJECTION_NAME,
    },
    Database, IndexColumn, IndexSpec,
};
use adb_projection_runtime::{ProjectionKind, ProjectionMode, ProjectionState};
use common::put;
use tempfile::tempdir;

/// `CREATE INDEX name ON entity 3 (field 2)`.
fn spec(name: &str) -> IndexSpec {
    IndexSpec {
        name: name.into(),
        entity_id: 3,
        columns: vec![IndexColumn {
            field_id: 2,
            direction: Direction::Asc,
        }],
        include: vec![],
    }
}

/// A new database has the current-row projection, ready at the head, which follows each commit.
#[test]
fn current_row_is_a_synchronous_projection() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let registry = db.projection_registry();
    let initial = registry.get(CURRENT_ROW_PROJECTION).unwrap();
    assert_eq!(initial.descriptor.name, CURRENT_ROW_PROJECTION_NAME);
    assert_eq!(initial.descriptor.kind, ProjectionKind::CurrentRow);
    assert_eq!(initial.descriptor.mode, ProjectionMode::Synchronous);
    assert_eq!(initial.state, ProjectionState::Ready);
    for i in 1..=4 {
        put(&db, RowId(i), i as i64);
    }
    let progress = registry.get(CURRENT_ROW_PROJECTION).unwrap();
    assert_eq!(progress.frontier.readable_through(), CommitSeq(4));
    assert_eq!(progress.head_commit_seq, CommitSeq(4));
    assert_eq!(progress.lag_commits, 0);
}

/// Indexes appear and disappear with `CREATE INDEX` / `DROP INDEX`, and follow the commits.
#[test]
fn indexes_are_registered_and_dropped() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    put(&db, RowId(1), 1);
    let index = db.create_index(spec("idx_a")).unwrap();
    let registry = db.projection_registry();
    let id = index_projection_id(index.id);
    let progress = registry.get(id).unwrap();
    assert_eq!(progress.descriptor.name, "index:idx_a");
    assert_eq!(progress.descriptor.kind, ProjectionKind::SecondaryIndex);
    assert_eq!(progress.descriptor.entity_id, Some(3));
    put(&db, RowId(2), 2);
    let head = db.latest_commit_seq();
    assert_eq!(registry.get(id).unwrap().frontier.readable_through(), head);
    db.drop_index("idx_a").unwrap();
    assert!(registry.get(id).is_none());
    assert!(registry.get(CURRENT_ROW_PROJECTION).is_some());
}

/// The registry survives a restart: the projections are known again and at the head.
#[test]
fn the_registry_survives_restart() {
    let dir = tempdir().unwrap();
    let id;
    {
        let db = Database::open(dir.path()).unwrap();
        put(&db, RowId(1), 1);
        id = index_projection_id(db.create_index(spec("idx_a")).unwrap().id);
        put(&db, RowId(2), 2);
        db.close().unwrap();
    }
    let db = Database::open(dir.path()).unwrap();
    let registry = db.projection_registry();
    let head = db.latest_commit_seq();
    assert_eq!(registry.get(id).unwrap().frontier.readable_through(), head);
    assert_eq!(
        registry
            .get(CURRENT_ROW_PROJECTION)
            .unwrap()
            .frontier
            .readable_through(),
        head
    );
    assert_eq!(registry.list().len(), 2);
    put(&db, RowId(3), 3);
    assert_eq!(registry.get(id).unwrap().lag_commits, 0);
}

/// An index dropped before a crash (no checkpoint after) does not come back as a projection.
#[test]
fn stale_index_projections_are_removed_at_open() {
    let dir = tempdir().unwrap();
    let id;
    {
        let db = Database::open(dir.path()).unwrap();
        id = index_projection_id(db.create_index(spec("idx_a")).unwrap().id);
        db.close().unwrap();
        db.drop_index("idx_a").unwrap();
    }
    let db = Database::open(dir.path()).unwrap();
    assert!(db.projection_registry().get(id).is_none());
}

/// With only synchronous projections nothing holds the change log back: pruning keeps just the
/// newest segment; an asynchronous consumer would hold its share (covered with the consumer in
/// the next task).
#[test]
fn pruning_without_consumers_keeps_the_active_segment() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    for i in 1..=5 {
        put(&db, RowId(i), i as i64);
    }
    let report = db
        .prune_change_log(&ChangeRetentionPolicy::default())
        .unwrap();
    assert!(!report.overtook_frontier);
    assert_eq!(db.change_log().head(), Some(CommitSeq(5)));
    assert_eq!(
        db.change_log()
            .read_after(CommitSeq::ZERO, 10)
            .unwrap()
            .len(),
        5
    );
}
