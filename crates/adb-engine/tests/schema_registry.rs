//! The schema registry as a projection of the log: replayed by recovery, published by
//! checkpoints, rebuilt by `rebuild_projections`, and refused when damaged.
use adb_core::{CommitTs, SchemaEpoch, SchemaVersion, TxId, ValueType};
use adb_engine::{
    schema_registry::registry_path, CatalogEvent, Database, DatabaseOptions, EntitySpec, NewField,
};
use adb_schema::{apply, create, SchemaChange};
use adb_wal::{SegmentedWalWriter, WalRecord};
use tempfile::tempdir;

/// `acc (id BIGINT PRIMARY KEY)` as entity 1 at epoch 1, then version 2 adding `note`.
fn versions() -> Vec<adb_engine::EntitySchema> {
    let int = |name: &str, nullable: bool| NewField {
        name: name.into(),
        value_type: ValueType::Int64,
        nullable,
        default: None,
        references: None,
        id: None,
    };
    let v1 = create(
        &EntitySpec {
            name: "acc".into(),
            fields: vec![int("id", false)],
            primary_key: "id".into(),
            entity_id: None,
        },
        1,
        SchemaEpoch(1),
    )
    .unwrap();
    let v2 = apply(
        &v1,
        &SchemaChange::AddField {
            field: int("note", true),
        },
        SchemaEpoch(2),
    )
    .unwrap();
    vec![v1, v2]
}

/// Appends one catalog transaction per version to the closed database's log.
fn log_catalog(dir: &std::path::Path) {
    let mut writer =
        SegmentedWalWriter::open(dir.join("wal"), adb_wal::DEFAULT_SEGMENT_SIZE).unwrap();
    for (n, schema) in versions().into_iter().enumerate() {
        let tx_id = TxId(1000 + n as u64);
        for record in [
            WalRecord::Begin {
                tx_id,
                snapshot_ts: CommitTs(0),
            },
            WalRecord::Catalog {
                tx_id,
                epoch: schema.epoch,
                event: CatalogEvent::Schema(schema),
            },
            WalRecord::Commit {
                tx_id,
                commit_ts: CommitTs(100 + n as u64),
            },
        ] {
            writer.append(&record).unwrap();
        }
    }
    writer.sync().unwrap();
}

/// Catalog records logged after the last checkpoint are replayed by recovery; the next
/// checkpoint publishes the registry, which then survives a reopen on its own.
#[test]
fn recovery_replays_catalog_records() {
    let dir = tempdir().unwrap();
    Database::open(dir.path()).unwrap().close().unwrap();
    log_catalog(dir.path());
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.schema_epoch(), SchemaEpoch(2));
    let latest = db.schema(1).unwrap();
    assert_eq!(latest.version, SchemaVersion(2));
    assert_eq!(db.schema_by_name("ACC").unwrap().entity_id, 1);
    assert_eq!(db.schema_history(1).len(), 2);
    assert_eq!(db.schemas().len(), 1);
    db.close().unwrap();
    drop(db);
    assert!(
        registry_path(dir.path()).exists(),
        "recovery checkpointed the registry"
    );
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.schema_epoch(), SchemaEpoch(2));
    assert!(db.schema(2).is_none());
}

/// Rebuilding the projections discards the registry file and restores the same registry from
/// the log.
#[test]
fn rebuild_restores_the_registry() {
    let dir = tempdir().unwrap();
    Database::open(dir.path()).unwrap().close().unwrap();
    log_catalog(dir.path());
    let before = {
        let db = Database::open(dir.path()).unwrap();
        let history = db.schema_history(1);
        db.close().unwrap();
        history
    };
    let db = Database::rebuild_projections(dir.path(), DatabaseOptions::default()).unwrap();
    assert_eq!(db.schema_history(1), before);
    assert_eq!(db.schema_epoch(), SchemaEpoch(2));
}

/// A damaged registry file is corruption, and rebuilding the projections repairs it.
#[test]
fn damaged_registry_is_corruption() {
    let dir = tempdir().unwrap();
    Database::open(dir.path()).unwrap().close().unwrap();
    log_catalog(dir.path());
    Database::open(dir.path()).unwrap().close().unwrap();
    std::fs::write(registry_path(dir.path()), b"not a registry").unwrap();
    let error = Database::open(dir.path()).err().expect("damaged registry");
    assert!(error.is_corruption(), "{error:?}");
    let db = Database::rebuild_projections(dir.path(), DatabaseOptions::default()).unwrap();
    assert_eq!(db.schema(1).unwrap().version, SchemaVersion(2));
}
