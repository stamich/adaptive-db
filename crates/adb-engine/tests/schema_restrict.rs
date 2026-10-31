//! RESTRICT semantics of schema DDL: dropping a column an index uses, dropping a table another
//! table references, and creating an index on a column that does not exist (any more).
use adb_core::{Direction, SchemaEpoch, ValueType};
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

/// `account(id BIGINT PRIMARY KEY, balance BIGINT, owner STRING, note STRING)`, entity 1.
fn account() -> EntitySpec {
    EntitySpec {
        name: "account".into(),
        fields: vec![
            field("id", ValueType::Int64, false),
            field("balance", ValueType::Int64, true),
            field("owner", ValueType::String, true),
            field("note", ValueType::String, true),
        ],
        primary_key: "id".into(),
        entity_id: None,
    }
}

/// An index of entity 1 on `keys` covering `include`.
fn index(name: &str, keys: &[u32], include: &[u32]) -> IndexSpec {
    IndexSpec {
        name: name.into(),
        entity_id: 1,
        columns: keys
            .iter()
            .map(|field_id| IndexColumn {
                field_id: *field_id,
                direction: Direction::Asc,
            })
            .collect(),
        include: include.to_vec(),
    }
}

/// `DROP COLUMN` of a key or covered column is refused and names the index; after the index is
/// dropped the column can go. Refusals commit nothing.
#[test]
fn drop_column_is_restricted_by_indexes() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(account()).unwrap();
    db.create_index(index("idx_balance", &[2], &[3])).unwrap();
    let epoch = db.schema_epoch();
    for (column, kind) in [("balance", "key column"), ("owner", "covered column")] {
        let refused = db
            .alter_entity(
                1,
                &SchemaChange::DropField {
                    name: column.into(),
                },
            )
            .expect_err("an index uses the column");
        assert!(
            matches!(refused, DbError::InvalidArgument(_)),
            "{refused:?}"
        );
        let message = refused.to_string();
        assert!(
            message.contains(&format!("cannot drop column account.{column}"))
                && message.contains(&format!("index idx_balance ({kind})")),
            "{message}"
        );
    }
    assert_eq!(db.schema_epoch(), epoch, "refusals commit nothing");
    db.alter_entity(
        1,
        &SchemaChange::DropField {
            name: "note".into(),
        },
    )
    .expect("an unindexed column can be dropped");
    db.drop_index("idx_balance").unwrap();
    db.alter_entity(
        1,
        &SchemaChange::DropField {
            name: "balance".into(),
        },
    )
    .expect("no index uses it any more");
    let refused = db
        .alter_entity(1, &SchemaChange::DropField { name: "id".into() })
        .expect_err("the primary key stays");
    assert!(refused.to_string().contains("primary key"), "{refused}");
}

/// `DROP TABLE` is refused while another live table holds a foreign-key hint to it.
#[test]
fn drop_table_is_restricted_by_foreign_keys() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(account()).unwrap();
    db.create_entity(EntitySpec {
        name: "transfer".into(),
        fields: vec![
            field("id", ValueType::Int64, false),
            NewField {
                references: Some(ForeignKey {
                    entity_id: 1,
                    field_id: 1,
                }),
                ..field("account_id", ValueType::Int64, true)
            },
        ],
        primary_key: "id".into(),
        entity_id: None,
    })
    .unwrap();
    let refused = db.drop_entity(1).expect_err("transfer references account");
    assert!(
        refused
            .to_string()
            .contains("column transfer.account_id (foreign-key hint) depends on it"),
        "{refused}"
    );
    db.alter_entity(
        2,
        &SchemaChange::DropField {
            name: "account_id".into(),
        },
    )
    .unwrap();
    db.drop_entity(1)
        .expect("a dropped column no longer references");
}

/// An index of an entity with a schema may only name its active columns (or the system field);
/// an entity without a schema is not checked, and a dropped entity cannot be indexed.
#[test]
fn indexes_need_active_columns() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    db.create_entity(account()).unwrap();
    db.alter_entity(
        1,
        &SchemaChange::DropField {
            name: "note".into(),
        },
    )
    .unwrap();
    let epoch = db.schema_epoch();
    for (name, keys, include) in [
        ("idx_dropped", &[4u32][..], &[][..]),
        ("idx_unknown", &[2][..], &[9][..]),
    ] {
        let refused = db
            .create_index(index(name, keys, include))
            .expect_err("not an active column");
        assert!(
            refused.to_string().contains("is not a column of account"),
            "{refused}"
        );
    }
    assert_eq!(db.schema_epoch(), epoch);
    assert!(db.indexes().is_empty(), "nothing was built");
    db.create_index(index("idx_system", &[0], &[2])).unwrap();

    let legacy = IndexSpec {
        entity_id: 7,
        ..index("idx_legacy", &[5], &[])
    };
    db.create_index(legacy)
        .expect("entities without a schema declare no fields");

    db.drop_index("idx_system").unwrap();
    db.drop_entity(1).unwrap();
    assert!(matches!(
        db.create_index(index("idx_gone", &[2], &[])),
        Err(DbError::EntityNotFound(_))
    ));
    assert!(db.schema_epoch() > SchemaEpoch(0));
}
