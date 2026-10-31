//! Schema versions: creation, the three changes, and the rules of versions and chains.
use adb_core::{SchemaEpoch, SchemaVersion, Value, ValueType};
use adb_schema::{
    apply, create, validate::validate_chain, EntitySchema, EntitySpec, NewField, SchemaChange,
    SchemaError,
};

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

/// `account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING)`.
fn account() -> EntitySchema {
    create(
        &EntitySpec {
            name: "account".into(),
            fields: vec![
                field("id", ValueType::Int64, false, None),
                field("balance", ValueType::Int64, false, None),
                field("owner", ValueType::String, true, None),
            ],
            primary_key: "id".into(),
            entity_id: None,
        },
        7,
        SchemaEpoch(1),
    )
    .unwrap()
}

/// `ADD COLUMN`.
fn add(name: &str, value_type: ValueType, nullable: bool, default: Option<Value>) -> SchemaChange {
    SchemaChange::AddField {
        field: field(name, value_type, nullable, default),
    }
}

/// Version 1 numbers fields 1.. in declaration order and points the id counter past them.
#[test]
fn create_assigns_ids_in_declaration_order() {
    let schema = account();
    assert_eq!(schema.version, SchemaVersion::FIRST);
    assert_eq!(schema.parent, None);
    let ids: Vec<u32> = schema.fields.iter().map(|f| f.id).collect();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(schema.next_field_id, 4);
    assert_eq!(schema.primary_key_field().name, "id");
    assert_eq!(
        schema.field("OWNER").unwrap().id,
        3,
        "names are case-insensitive"
    );
}

/// The primary key must exist and be a non-NULL BIGINT; ids are all explicit or all implicit;
/// names follow the identifier rule.
#[test]
fn invalid_tables_are_refused() {
    let spec = |fields: Vec<NewField>, pk: &str| EntitySpec {
        name: "t".into(),
        fields,
        primary_key: pk.into(),
        entity_id: None,
    };
    let int = |name: &str| field(name, ValueType::Int64, false, None);
    for bad in [
        spec(vec![int("id")], "missing"),
        spec(vec![field("id", ValueType::String, false, None)], "id"),
        spec(vec![field("id", ValueType::Int64, true, None)], "id"),
        spec(vec![int("id"), int("ID")], "id"),
        spec(vec![int("bad name")], "bad name"),
        spec(vec![], "id"),
        spec(
            vec![
                NewField {
                    id: Some(4),
                    ..int("id")
                },
                int("other"),
            ],
            "id",
        ),
        spec(
            vec![
                int("id"),
                field("x", ValueType::Int64, true, Some(Value::String("a".into()))),
            ],
            "id",
        ),
    ] {
        assert!(create(&bad, 1, SchemaEpoch(1)).is_err(), "{bad:?}");
    }
    let imported = create(
        &spec(
            vec![
                NewField {
                    id: Some(5),
                    ..int("id")
                },
                NewField {
                    id: Some(2),
                    ..int("a")
                },
            ],
            "id",
        ),
        1,
        SchemaEpoch(1),
    )
    .unwrap();
    assert_eq!(imported.next_field_id, 6, "imported ids are kept");
}

/// ADD, RENAME and DROP each publish the next version; RENAME keeps the id; DROP hides the
/// field from the version but keeps it in the history.
#[test]
fn changes_publish_successive_versions() {
    let v1 = account();
    let v2 = apply(
        &v1,
        &add(
            "status",
            ValueType::String,
            false,
            Some(Value::String("ACTIVE".into())),
        ),
        SchemaEpoch(2),
    )
    .unwrap();
    assert_eq!(v2.version, SchemaVersion(2));
    assert_eq!(v2.parent, Some(SchemaVersion(1)));
    let status = v2.field("status").unwrap();
    assert_eq!((status.id, status.introduced_in), (4, SchemaVersion(2)));

    let v3 = apply(
        &v2,
        &SchemaChange::RenameField {
            from: "owner".into(),
            to: "owner_name".into(),
        },
        SchemaEpoch(3),
    )
    .unwrap();
    assert_eq!(v3.field("owner_name").unwrap().id, 3);
    assert!(v3.field("owner").is_none());

    let v4 = apply(
        &v3,
        &SchemaChange::DropField {
            name: "owner_name".into(),
        },
        SchemaEpoch(4),
    )
    .unwrap();
    assert!(!v4.is_active(3));
    assert_eq!(
        v4.field_by_id(3).unwrap().dropped_in,
        Some(SchemaVersion(4))
    );
    assert!(v3.is_active(3), "published versions never change");

    // Re-adding the name gets a fresh id: ids are never reused.
    let v5 = apply(
        &v4,
        &add("owner_name", ValueType::String, true, None),
        SchemaEpoch(5),
    )
    .unwrap();
    assert_eq!(v5.field("owner_name").unwrap().id, 5);
    validate_chain(&[v1, v2, v3, v4, v5]).unwrap();
}

/// The changes that would break existing rows or the primary key are refused.
#[test]
fn unsafe_changes_are_refused() {
    let v1 = account();
    let refused = [
        add("flag", ValueType::Bool, false, None),
        add("balance", ValueType::Int64, true, None),
        add("Owner", ValueType::String, true, None),
        add("n", ValueType::Int64, true, Some(Value::Float64(1.0))),
        add("n", ValueType::Int64, true, Some(Value::Null)),
        SchemaChange::DropField { name: "id".into() },
        SchemaChange::DropField {
            name: "nope".into(),
        },
        SchemaChange::RenameField {
            from: "owner".into(),
            to: "balance".into(),
        },
        SchemaChange::RenameField {
            from: "owner".into(),
            to: "owner".into(),
        },
        SchemaChange::AddField {
            field: NewField {
                id: Some(9),
                ..field("x", ValueType::Int64, true, None)
            },
        },
    ];
    for change in refused {
        assert!(apply(&v1, &change, SchemaEpoch(2)).is_err(), "{change:?}");
    }
    assert!(matches!(
        apply(
            &v1,
            &SchemaChange::DropField {
                name: "nope".into()
            },
            SchemaEpoch(2)
        ),
        Err(SchemaError::UnknownField { .. })
    ));
    // The primary key may be renamed: its id, and every row key, stay the same.
    let renamed = apply(
        &v1,
        &SchemaChange::RenameField {
            from: "id".into(),
            to: "account_id".into(),
        },
        SchemaEpoch(2),
    )
    .unwrap();
    assert_eq!(renamed.primary_key_field().name, "account_id");
    // Changing only the case of a name is a rename.
    apply(
        &v1,
        &SchemaChange::RenameField {
            from: "owner".into(),
            to: "Owner".into(),
        },
        SchemaEpoch(2),
    )
    .unwrap();
}

/// A tampered history (reused id, changed type, gap, older epoch) fails chain validation.
#[test]
fn damaged_histories_are_corruption() {
    let v1 = account();
    let v2 = apply(
        &v1,
        &add("note", ValueType::String, true, None),
        SchemaEpoch(2),
    )
    .unwrap();
    validate_chain(&[v1.clone(), v2.clone()]).unwrap();

    let mut retyped = v2.clone();
    retyped.fields[1].value_type = ValueType::String;
    let mut reused = v2.clone();
    reused.next_field_id = 3;
    let mut gap = v2.clone();
    gap.version = SchemaVersion(3);
    gap.parent = Some(SchemaVersion(2));
    let mut early = v2.clone();
    early.epoch = SchemaEpoch(1);
    for bad in [retyped, reused, gap, early] {
        assert!(matches!(
            validate_chain(&[v1.clone(), bad]),
            Err(SchemaError::Corrupt(_))
        ));
    }
    assert!(
        validate_chain(&[v2]).is_err(),
        "a history starts at version 1"
    );
}

/// Deterministic random chains of changes stay valid, never reuse an id, and keep the names
/// of each version unique.
#[test]
fn random_chains_stay_valid() {
    let mut seed = 0x2400_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..50 {
        let mut history = vec![account()];
        let mut ever = std::collections::BTreeSet::from([1, 2, 3]);
        for step in 0..40u64 {
            let current = history.last().unwrap();
            let active: Vec<String> = current.active_fields().map(|f| f.name.clone()).collect();
            let pick = &active[(next() % active.len() as u64) as usize];
            let change = match next() % 3 {
                0 => add(&format!("c{step}"), ValueType::Int64, true, None),
                1 => SchemaChange::RenameField {
                    from: pick.clone(),
                    to: format!("r{step}"),
                },
                _ => SchemaChange::DropField { name: pick.clone() },
            };
            let epoch = SchemaEpoch(current.epoch.0 + 1);
            if let Ok(version) = apply(current, &change, epoch) {
                for f in &version.fields {
                    if !current.fields.iter().any(|old| old.id == f.id) {
                        assert!(ever.insert(f.id), "id {} reused", f.id);
                    }
                }
                history.push(version);
            }
        }
        validate_chain(&history).unwrap();
        assert!(
            history.last().unwrap().is_active(1),
            "the primary key survives"
        );
    }
}

/// The JSON form of the changes is the C ABI contract.
#[test]
fn changes_have_a_stable_json_form() {
    let change: SchemaChange = serde_json::from_str(
        r#"{"op":"add_field","field":{"name":"status","value_type":"String","nullable":false,"default":{"String":"ACTIVE"}}}"#,
    )
    .unwrap();
    assert_eq!(
        change,
        add(
            "status",
            ValueType::String,
            false,
            Some(Value::String("ACTIVE".into()))
        )
    );
    let drop: SchemaChange = serde_json::from_str(r#"{"op":"drop_field","name":"x"}"#).unwrap();
    assert_eq!(drop, SchemaChange::DropField { name: "x".into() });
    assert!(serde_json::from_str::<SchemaChange>(
        r#"{"op":"drop_field","name":"x","cascade":true}"#
    )
    .is_err());
}
