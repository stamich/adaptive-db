//! Row adaptation: virtual defaults, hidden fields, explicit NULLs, both directions, and a model
//! test against a direct reading of the rules.
use adb_core::{Row, SchemaEpoch, SchemaVersion, Value, ValueType};
use adb_schema::{apply, create, EntitySchema, EntitySpec, NewField, RowAdapter, SchemaChange};

/// A field declaration.
fn field(name: &str, nullable: bool, default: Option<Value>) -> NewField {
    NewField {
        name: name.into(),
        value_type: ValueType::Int64,
        nullable,
        default,
        references: None,
        id: None,
    }
}

/// v1 (id, balance, legacy), v2 + status DEFAULT 1, v3 + tier (nullable), v4 rename balance,
/// v5 drop legacy; returns every version.
fn history() -> Vec<EntitySchema> {
    let v1 = create(
        &EntitySpec {
            name: "acc".into(),
            fields: vec![
                field("id", false, None),
                field("balance", true, None),
                field("legacy", true, None),
            ],
            primary_key: "id".into(),
            entity_id: None,
        },
        1,
        SchemaEpoch(1),
    )
    .unwrap();
    let mut versions = vec![v1];
    for change in [
        SchemaChange::AddField {
            field: field("status", false, Some(Value::Int64(1))),
        },
        SchemaChange::AddField {
            field: field("tier", true, None),
        },
        SchemaChange::RenameField {
            from: "balance".into(),
            to: "amount".into(),
        },
        SchemaChange::DropField {
            name: "legacy".into(),
        },
    ] {
        let last = versions.last().unwrap();
        let next = apply(last, &change, SchemaEpoch(last.epoch.0 + 1)).unwrap();
        versions.push(next);
    }
    versions
}

/// A row with the given fields and stamp.
fn row(version: u32, fields: &[(u32, Value)]) -> Row {
    fields.iter().fold(
        Row::new().with_schema_version(SchemaVersion(version)),
        |row, (f, v)| row.with_field(*f, v.clone()),
    )
}

/// An old row read through the latest version gets the virtual default and loses the dropped
/// field; the rename needs nothing.
#[test]
fn old_rows_read_through_the_latest_version() {
    let versions = history();
    let latest = versions.last().unwrap();
    let adapter = RowAdapter::compile(latest, SchemaVersion(1), latest.version);
    let adapted = adapter.adapt(row(
        1,
        &[
            (1, Value::Int64(7)),
            (2, Value::Int64(100)),
            (3, Value::Int64(9)),
        ],
    ));
    assert_eq!(
        adapted,
        row(
            5,
            &[
                (1, Value::Int64(7)),
                (2, Value::Int64(100)),
                (4, Value::Int64(1))
            ]
        )
    );
    // Unversioned rows (written before 2.4.0) count as version 1.
    let unversioned = RowAdapter::compile(latest, SchemaVersion(0), latest.version);
    assert_eq!(unversioned, adapter);
}

/// A default applies only to rows that predate the field: a row written after it that lacks
/// the field reads NULL, and an explicit NULL is never replaced.
#[test]
fn defaults_follow_versions_not_presence() {
    let versions = history();
    let latest = versions.last().unwrap();
    let from_v3 = RowAdapter::compile(latest, SchemaVersion(3), latest.version);
    let adapted = from_v3.adapt(row(3, &[(1, Value::Int64(1))]));
    assert_eq!(
        adapted.get(4),
        None,
        "written at v3 without status: NULL, not the default"
    );
    let from_v1 = RowAdapter::compile(latest, SchemaVersion(1), latest.version);
    let explicit = from_v1.adapt(row(1, &[(1, Value::Int64(1)), (4, Value::Null)]));
    assert_eq!(explicit.get(4), Some(&Value::Null));
}

/// Rows already at the target are returned unchanged; the system field 0 always survives.
#[test]
fn identity_and_system_field() {
    let versions = history();
    let latest = versions.last().unwrap();
    let identity = RowAdapter::compile(latest, latest.version, latest.version);
    assert!(identity.is_identity());
    let r = row(5, &[(0, Value::Int64(1)), (1, Value::Int64(2))]);
    assert_eq!(identity.adapt(r.clone()), r);
    let from_v1 = RowAdapter::compile(latest, SchemaVersion(1), latest.version);
    assert_eq!(
        from_v1.adapt(row(1, &[(0, Value::Int64(1))])).get(0),
        Some(&Value::Int64(1))
    );
}

/// Reading backward (a query pinned to v2 sees a row written at v5): later fields are hidden,
/// a field dropped before the row was written reads NULL.
#[test]
fn newer_rows_read_through_an_older_version() {
    let versions = history();
    let latest = versions.last().unwrap();
    let adapter = RowAdapter::compile(latest, SchemaVersion(5), SchemaVersion(2));
    let adapted = adapter.adapt(row(
        5,
        &[
            (1, Value::Int64(1)),
            (4, Value::Int64(2)),
            (5, Value::Int64(3)),
        ],
    ));
    assert_eq!(
        adapted,
        row(2, &[(1, Value::Int64(1)), (4, Value::Int64(2))])
    );
}

/// The rules read directly, for comparison with the compiled adapter.
fn reference(history: &EntitySchema, input: &Row, to: SchemaVersion) -> Row {
    let from = input.schema_version.effective();
    let mut out = Row::new().with_schema_version(to);
    for f in history.fields.iter().filter(|f| f.is_active_in(to)) {
        match input.get(f.id) {
            Some(v) => {
                out.fields.insert(f.id, v.clone());
            }
            None if from < f.introduced_in && f.default.is_some() => {
                out.fields.insert(f.id, f.default.clone().unwrap());
            }
            None => {}
        }
    }
    if let Some(v) = input.get(0) {
        out.fields.insert(0, v.clone());
    }
    out
}

/// For every pair of versions and random rows, the adapter equals the reference reading.
#[test]
fn adapter_matches_the_rules() {
    let versions = history();
    let latest = versions.last().unwrap();
    let mut seed = 99u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for from in 1..=5u32 {
        for to in 1..=5u32 {
            let adapter = RowAdapter::compile(latest, SchemaVersion(from), SchemaVersion(to));
            for _ in 0..200 {
                let mut input = Row::new().with_schema_version(SchemaVersion(from));
                for f in 0..=6u32 {
                    match next() % 3 {
                        0 => {}
                        1 => input.fields.insert(f, Value::Null).map_or((), |_| ()),
                        _ => input
                            .fields
                            .insert(f, Value::Int64((next() % 100) as i64))
                            .map_or((), |_| ()),
                    }
                }
                // A row only holds fields active in the version it was written under.
                let written = &versions[from as usize - 1];
                input.fields.retain(|f, _| *f == 0 || written.is_active(*f));
                let expected = reference(latest, &input, SchemaVersion(to));
                assert_eq!(
                    adapter.adapt(input.clone()),
                    expected,
                    "{from} -> {to}: {input:?}"
                );
            }
        }
    }
}
