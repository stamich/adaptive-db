//! DDL in the engine: CREATE / ALTER / DROP TABLE as catalog transactions, epochs, foreign
//! keys, and the epoch moves of index DDL.
use adb_core::{SchemaEpoch, SchemaVersion, Value, ValueType};
use adb_engine::{Database, DbError, EntitySpec, IndexColumn, IndexSpec, NewField, SchemaChange};
use adb_schema::ForeignKey;
use tempfile::tempdir;

/// A field declaration.
fn field(name: &str, value_type: ValueType, nullable: bool) -> NewField {
    NewField {
        name: name.into(),
        value_type,
        nullable,
        default: None,
        references: None,
        id: None,
    }
}

/// `CREATE TABLE name (id BIGINT PRIMARY KEY, balance BIGINT, owner STRING)`.
fn spec(name: &str) -> EntitySpec {
    EntitySpec {
        name: name.into(),
        fields: vec![
            field("id", ValueType::Int64, false),
            field("balance", ValueType::Int64, true),
            field("owner", ValueType::String, true),
        ],
        primary_key: "id".into(),
        entity_id: None,
    }
}

/// CREATE assigns entity and field ids and moves the epoch; ALTER publishes the next version;
/// both survive a crash (the log holds them) and a clean reopen (the checkpoint does).
#[test]
fn create_and_alter_publish_versions() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.schema_epoch(), SchemaEpoch(0));
    let account = db.create_entity(spec("account")).unwrap();
    assert_eq!(account.entity_id, 1);
    assert_eq!(account.version, SchemaVersion(1));
    assert_eq!(db.schema_epoch(), SchemaEpoch(1));
    let shop = db.create_entity(spec("shop")).unwrap();
    assert_eq!(shop.entity_id, 2);

    let v2 = db
        .alter_entity(
            1,
            &SchemaChange::AddField {
                field: NewField {
                    default: Some(Value::String("ACTIVE".into())),
                    ..field("status", ValueType::String, false)
                },
            },
        )
        .unwrap();
    assert_eq!((v2.version, v2.epoch), (SchemaVersion(2), SchemaEpoch(3)));
    let v3 = db
        .alter_entity(
            1,
            &SchemaChange::RenameField {
                from: "owner".into(),
                to: "holder".into(),
            },
        )
        .unwrap();
    assert_eq!(v3.field("holder").unwrap().id, 3);
    drop(db); // crash: the catalog transactions are replayed from the log
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.schema(1).unwrap().version, SchemaVersion(3));
    assert_eq!(db.schema_epoch(), SchemaEpoch(4));
    db.close().unwrap();
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.schema_history(1).len(), 3);
}

/// Refused DDL changes nothing: the epoch stays, the instance is not poisoned.
#[test]
fn refused_ddl_leaves_no_trace() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(spec("account")).unwrap();
    let epoch = db.schema_epoch();
    assert!(db.create_entity(spec("ACCOUNT")).is_err());
    assert!(matches!(
        db.alter_entity(
            1,
            &SchemaChange::AddField {
                field: field("x", ValueType::Int64, false)
            }
        ),
        Err(DbError::Schema(_))
    ));
    assert!(matches!(
        db.alter_entity(9, &SchemaChange::DropField { name: "x".into() }),
        Err(DbError::EntityNotFound(_))
    ));
    assert!(db
        .alter_entity(1, &SchemaChange::DropField { name: "id".into() })
        .is_err());
    let mut taken = spec("other");
    taken.entity_id = Some(1);
    assert!(db.create_entity(taken).is_err());
    assert_eq!(db.schema_epoch(), epoch);
    assert!(!db.is_poisoned());
}

/// Foreign-key hints must name the primary key of a live entity with the same type.
#[test]
fn foreign_keys_are_checked() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(spec("customer")).unwrap();
    let with_reference = |reference: ForeignKey, value_type: ValueType| EntitySpec {
        name: "orders".into(),
        fields: vec![
            field("id", ValueType::Int64, false),
            NewField {
                references: Some(reference),
                ..field("customer_id", value_type, false)
            },
        ],
        primary_key: "id".into(),
        entity_id: None,
    };
    for (reference, value_type) in [
        (
            ForeignKey {
                entity_id: 9,
                field_id: 1,
            },
            ValueType::Int64,
        ),
        (
            ForeignKey {
                entity_id: 1,
                field_id: 2,
            },
            ValueType::Int64,
        ),
        (
            ForeignKey {
                entity_id: 1,
                field_id: 1,
            },
            ValueType::String,
        ),
    ] {
        assert!(db
            .create_entity(with_reference(reference, value_type))
            .is_err());
    }
    let orders = db
        .create_entity(with_reference(
            ForeignKey {
                entity_id: 1,
                field_id: 1,
            },
            ValueType::Int64,
        ))
        .unwrap();
    assert_eq!(
        orders
            .field("customer_id")
            .unwrap()
            .references
            .unwrap()
            .entity_id,
        1
    );
}

/// DROP TABLE frees the name but never the id, keeps the history, and is refused while an
/// index depends on the entity; index DDL moves the epoch too.
#[test]
fn drop_entity_and_index_ddl() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(spec("account")).unwrap();
    let before = db.schema_epoch();
    db.create_index(IndexSpec {
        name: "idx_balance".into(),
        entity_id: 1,
        columns: vec![IndexColumn {
            field_id: 2,
            direction: adb_core::Direction::Asc,
        }],
        include: vec![],
    })
    .unwrap();
    assert_eq!(db.schema_epoch(), SchemaEpoch(before.0 + 1));
    let refused = db.drop_entity(1).expect_err("an index depends on it");
    assert!(refused.to_string().contains("idx_balance"), "{refused}");
    db.drop_index("idx_balance").unwrap();
    assert_eq!(db.schema_epoch(), SchemaEpoch(before.0 + 2));
    let dropped = db.drop_entity(1).unwrap();
    assert_eq!(dropped.name, "account");
    assert!(db.schema(1).is_none());
    assert_eq!(db.schema_history(1).len(), 1);
    let again = db.create_entity(spec("account")).unwrap();
    assert_eq!(again.entity_id, 2, "ids are never reused");
    drop(db);
    let db = Database::open(dir.path()).unwrap();
    assert_eq!(db.schemas().len(), 1);
    assert_eq!(db.schema_epoch(), SchemaEpoch(before.0 + 4));
}

/// An imported catalog keeps its entity and field ids.
#[test]
fn imported_ids_are_kept() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut imported = spec("legacy");
    imported.entity_id = Some(7);
    for (at, f) in imported.fields.iter_mut().enumerate() {
        f.id = Some(at as u32 + 1);
    }
    let schema = db.create_entity(imported).unwrap();
    assert_eq!(schema.entity_id, 7);
    assert_eq!(db.create_entity(spec("next")).unwrap().entity_id, 8);
}
