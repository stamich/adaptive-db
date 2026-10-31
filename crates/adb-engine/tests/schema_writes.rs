//! Schema-aware writes: the engine stamps, completes and validates every row of an entity with
//! a schema before logging it; rows of entities without a schema are stored as written.
use adb_core::{Row, RowId, SchemaVersion, Value, ValueType};
use adb_engine::{Database, DbError, EntitySpec, NewField, SchemaChange};
use tempfile::tempdir;

/// A field declaration.
fn field(name: &str, value_type: ValueType, nullable: bool, default: Option<Value>) -> NewField {
    NewField {
        name: name.into(),
        value_type,
        nullable,
        default,
        references: None,
        id: None,
    }
}

/// A database with `account (id BIGINT PK, balance BIGINT NOT NULL, owner STRING,
/// status STRING NOT NULL DEFAULT 'ACTIVE')` as entity 1.
fn database(dir: &std::path::Path) -> Database {
    let db = Database::open(dir).unwrap();
    db.create_entity(EntitySpec {
        name: "account".into(),
        fields: vec![
            field("id", ValueType::Int64, false, None),
            field("balance", ValueType::Int64, false, None),
            field("owner", ValueType::String, true, None),
            field(
                "status",
                ValueType::String,
                false,
                Some(Value::String("ACTIVE".into())),
            ),
        ],
        primary_key: "id".into(),
        entity_id: None,
    })
    .unwrap();
    db
}

/// Commits `row` under primary key `pk` of entity `entity`.
fn put(db: &Database, entity: u64, pk: i64, row: Row) -> Result<(), DbError> {
    let mut tx = db.begin();
    tx.put(RowId::from_primary_key(entity, pk), row);
    db.commit(tx).map(|_| ())
}

/// A valid account row with key `pk`.
fn account(pk: i64) -> Row {
    Row::new()
        .with_field(1, Value::Int64(pk))
        .with_field(2, Value::Int64(100))
}

/// A new row is stamped with the latest version and stores the defaults it omits.
#[test]
fn inserts_are_stamped_and_completed() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    put(&db, 1, 7, account(7).with_field(0, Value::Int64(1))).unwrap();
    let stored = db.history(RowId::from_primary_key(1, 7)).unwrap()[0]
        .value
        .clone()
        .unwrap();
    assert_eq!(stored.schema_version, SchemaVersion(1));
    assert_eq!(stored.get(4), Some(&Value::String("ACTIVE".into())));
    assert_eq!(
        stored.get(3),
        None,
        "a nullable field without a default stays absent"
    );
    assert_eq!(
        stored.get(0),
        Some(&Value::Int64(1)),
        "the system field is kept"
    );
    // An explicit NULL into a nullable defaulted column would stay NULL; here status is NOT NULL.
    put(&db, 1, 8, account(8).with_field(3, Value::Null)).unwrap();
}

/// Rows that do not fit the schema are refused before anything is logged.
#[test]
fn invalid_rows_are_refused_before_logging() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    let end = db.change_feed_end();
    let refused = [
        account(1).with_field(2, Value::String("100".into())),
        account(1).with_field(9, Value::Int64(1)),
        Row::new().with_field(1, Value::Int64(1)),
        account(1).with_field(2, Value::Null),
        account(1).with_field(4, Value::Null),
        account(2),
        account(1).with_schema_version(SchemaVersion(5)),
    ];
    for row in refused {
        let error = put(&db, 1, 1, row.clone()).expect_err("refused");
        assert!(matches!(error, DbError::Schema(_)), "{row:?}: {error}");
    }
    assert_eq!(db.change_feed_end(), end, "nothing was logged");
    assert!(!db.is_poisoned());
    assert!(db.get(RowId::from_primary_key(1, 1)).unwrap().is_none());
}

/// A dropped column can no longer be written.
#[test]
fn dropped_columns_are_refused() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    db.alter_entity(
        1,
        &SchemaChange::DropField {
            name: "owner".into(),
        },
    )
    .unwrap();
    let error = put(
        &db,
        1,
        1,
        account(1).with_field(3, Value::String("x".into())),
    )
    .expect_err("dropped");
    assert!(error.to_string().contains("dropped"), "{error}");
    put(&db, 1, 1, account(1)).unwrap();
}

/// Entities without a registered schema keep the 2.3.0 behaviour: rows are stored as written.
#[test]
fn entities_without_a_schema_are_not_validated() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    put(&db, 9, 1, Row::new().with_field(42, Value::Bool(true))).unwrap();
    let stored = db.get(RowId::from_primary_key(9, 1)).unwrap().unwrap();
    assert!(stored.schema_version.is_unversioned());
}

/// After ADD COLUMN ... DEFAULT and DROP COLUMN an old row reads through the latest version
/// without being rewritten; an update rewrites it at the latest version, with the default
/// materialized and the dropped column's value gone.
#[test]
fn updates_migrate_rows_lazily() {
    let dir = tempdir().unwrap();
    let db = database(dir.path());
    put(
        &db,
        1,
        5,
        account(5).with_field(3, Value::String("ada".into())),
    )
    .unwrap();
    let key = RowId::from_primary_key(1, 5);
    db.alter_entity(
        1,
        &SchemaChange::AddField {
            field: field("tier", ValueType::Int64, false, Some(Value::Int64(1))),
        },
    )
    .unwrap();
    db.alter_entity(
        1,
        &SchemaChange::DropField {
            name: "owner".into(),
        },
    )
    .unwrap();

    let read = db.get(key).unwrap().unwrap();
    assert_eq!(read.schema_version, SchemaVersion(3));
    assert_eq!(read.get(5), Some(&Value::Int64(1)), "virtual default");
    assert_eq!(read.get(3), None, "dropped column hidden");
    let stored = db.history(key).unwrap().pop().unwrap().value.unwrap();
    assert_eq!(
        stored.schema_version,
        SchemaVersion(1),
        "nothing was rewritten"
    );
    assert_eq!(stored.get(3), Some(&Value::String("ada".into())));

    let mut tx = db.begin();
    let mut row = db.get_in_tx(&mut tx, key).unwrap().unwrap();
    row.fields.insert(2, Value::Int64(250));
    tx.put(key, row);
    db.commit(tx).unwrap();
    let rewritten = db.history(key).unwrap().pop().unwrap().value.unwrap();
    assert_eq!(rewritten.schema_version, SchemaVersion(3));
    assert_eq!(
        rewritten.get(5),
        Some(&Value::Int64(1)),
        "default materialized"
    );
    assert_eq!(
        rewritten.get(3),
        None,
        "space of the dropped column reclaimed"
    );
    assert_eq!(rewritten.get(2), Some(&Value::Int64(250)));
}
