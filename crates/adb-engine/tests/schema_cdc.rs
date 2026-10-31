//! The change feed and the catalog: DDL events in commit order, row changes naming the schema
//! version they were written under, entity filters applied to both, index DDL left out.
use adb_core::{Direction, Row, RowId, SchemaEpoch, SchemaVersion, Value, ValueType};
use adb_engine::{
    CatalogChange, ChangeCursor, ChangeFilter, ChangeKind, Database, EntitySpec, IndexColumn,
    IndexSpec, NewField, SchemaChange,
};
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

/// `CREATE TABLE name (id BIGINT PRIMARY KEY, balance BIGINT)` with id `entity_id`.
fn table(name: &str, entity_id: u64) -> EntitySpec {
    EntitySpec {
        name: name.into(),
        fields: vec![
            field("id", ValueType::Int64, false),
            field("balance", ValueType::Int64, true),
        ],
        primary_key: "id".into(),
        entity_id: Some(entity_id),
    }
}

/// Commits row `pk` of entity `entity` with balance `balance`.
fn put(db: &Database, entity: u64, pk: i64, balance: i64) {
    let mut tx = db.begin();
    tx.put(
        RowId::from_primary_key(entity, pk),
        Row::new()
            .with_field(1, Value::Int64(pk))
            .with_field(2, Value::Int64(balance)),
    );
    db.commit(tx).unwrap();
}

/// CREATE, ALTER and DROP TABLE appear as catalog changes between the row events, and every
/// row change carries the version its row was written under.
#[test]
fn ddl_and_row_versions_are_in_the_feed() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(table("account", 1)).unwrap();
    put(&db, 1, 1, 10);
    db.alter_entity(
        1,
        &SchemaChange::AddField {
            field: field("note", ValueType::String, true),
        },
    )
    .unwrap();
    put(&db, 1, 2, 20);
    {
        let mut tx = db.begin();
        tx.delete(RowId::from_primary_key(1, 1));
        db.commit(tx).unwrap();
    }
    db.create_index(IndexSpec {
        name: "idx".into(),
        entity_id: 1,
        columns: vec![IndexColumn {
            field_id: 2,
            direction: Direction::Asc,
        }],
        include: vec![],
    })
    .unwrap();
    db.drop_index("idx").unwrap();
    db.drop_entity(1).unwrap();

    let batch = db
        .read_changes(ChangeCursor::BEGINNING, 100, &ChangeFilter::all())
        .unwrap();
    let summary: Vec<String> = batch
        .events
        .iter()
        .map(|event| {
            let catalog = event.catalog.iter().map(|change| match change {
                CatalogChange::Schema { epoch, schema } => {
                    format!("schema v{} @{}", schema.version.0, epoch.0)
                }
                CatalogChange::DropEntity { epoch, entity_id } => {
                    format!("drop {entity_id} @{}", epoch.0)
                }
            });
            let rows = event.changes.iter().map(|change| {
                format!(
                    "{:?} {} v{}",
                    change.kind,
                    change.primary_key(),
                    change.schema_version.0
                )
            });
            catalog.chain(rows).collect::<Vec<_>>().join(", ")
        })
        .collect();
    assert_eq!(
        summary,
        [
            "schema v1 @1",
            "Insert 1 v1",
            "schema v2 @2",
            "Insert 2 v2",
            "Delete 1 v1",
            "drop 1 @5",
        ],
        "index DDL (epochs 3 and 4) is not part of the feed"
    );
    let CatalogChange::Schema { schema, .. } = &batch.events[2].catalog[0] else {
        panic!("expected a schema change");
    };
    assert_eq!(
        schema.field("note").unwrap().introduced_in,
        SchemaVersion(2)
    );
    assert_eq!(batch.events[4].changes[0].kind, ChangeKind::Delete);
    assert_eq!(db.schema_epoch(), SchemaEpoch(5));
}

/// The entity filter applies to catalog changes as to rows.
#[test]
fn entity_filters_apply_to_catalog_changes() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(table("a", 1)).unwrap();
    db.create_entity(table("b", 2)).unwrap();
    put(&db, 2, 1, 1);
    let only_b = db
        .read_changes(ChangeCursor::BEGINNING, 100, &ChangeFilter::entities([2]))
        .unwrap();
    assert_eq!(only_b.events.len(), 2, "b's schema and b's row");
    assert!(only_b
        .events
        .iter()
        .flat_map(|event| &event.catalog)
        .all(|change| change.entity_id() == 2));
    let unversioned = {
        put(&db, 9, 1, 1);
        db.read_changes(only_b.next, 100, &ChangeFilter::entities([9]))
            .unwrap()
    };
    assert_eq!(
        unversioned.events[0].changes[0].schema_version,
        SchemaVersion(0),
        "rows of entities without a schema are unversioned"
    );
}
