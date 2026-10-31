//! The schema registry: applying catalog events in order, pinning readers to an epoch, the
//! adapter cache, and validation of persisted state.
use adb_core::{Row, SchemaEpoch, SchemaVersion, Value, ValueType};
use adb_schema::{
    apply, create, CatalogEvent, EntitySchema, EntitySpec, NewField, SchemaChange, SchemaError,
    SchemaRegistry,
};

/// A field declaration.
fn field(name: &str, default: Option<Value>) -> NewField {
    NewField {
        name: name.into(),
        value_type: ValueType::Int64,
        nullable: default.is_none(),
        default,
        references: None,
        id: None,
    }
}

/// Version 1 of `name (id BIGINT PRIMARY KEY, v BIGINT)` with id `entity` at `epoch`.
fn table(name: &str, entity: u64, epoch: u64) -> EntitySchema {
    create(
        &EntitySpec {
            name: name.into(),
            fields: vec![
                NewField {
                    nullable: false,
                    ..field("id", None)
                },
                field("v", None),
            ],
            primary_key: "id".into(),
            entity_id: None,
        },
        entity,
        SchemaEpoch(epoch),
    )
    .unwrap()
}

/// Creation, alteration, an epoch-only change and a drop move the epoch and the histories.
#[test]
fn events_build_histories() {
    let mut registry = SchemaRegistry::new();
    let v1 = table("acc", 1, 1);
    registry
        .apply(SchemaEpoch(1), &CatalogEvent::Schema(v1.clone()))
        .unwrap();
    let v2 = apply(
        &v1,
        &SchemaChange::AddField {
            field: field("status", Some(Value::Int64(1))),
        },
        SchemaEpoch(3),
    )
    .unwrap();
    registry
        .apply(SchemaEpoch(2), &CatalogEvent::Epoch)
        .unwrap();
    registry
        .apply(SchemaEpoch(3), &CatalogEvent::Schema(v2))
        .unwrap();
    assert_eq!(registry.epoch(), SchemaEpoch(3));
    assert_eq!(registry.latest(1).unwrap().version, SchemaVersion(2));
    assert_eq!(registry.by_name("ACC").unwrap().entity_id, 1);
    assert_eq!(registry.next_entity_id(), 2);

    // A reader pinned before the ALTER keeps seeing version 1.
    assert_eq!(
        registry.at_epoch(1, SchemaEpoch(2)).unwrap().version,
        SchemaVersion(1)
    );
    assert!(registry.at_epoch(1, SchemaEpoch(0)).is_none());

    registry
        .apply(SchemaEpoch(4), &CatalogEvent::DropEntity { entity_id: 1 })
        .unwrap();
    assert!(registry.latest(1).is_none());
    assert!(registry.by_name("acc").is_none(), "the name is free again");
    assert_eq!(
        registry.history(1).unwrap().versions.len(),
        2,
        "history is kept"
    );
    assert!(registry.at_epoch(1, SchemaEpoch(3)).is_some());
    assert!(registry.at_epoch(1, SchemaEpoch(4)).is_none());
    // A new table with the old name gets a new id.
    registry
        .apply(SchemaEpoch(5), &CatalogEvent::Schema(table("acc", 2, 5)))
        .unwrap();
    assert_eq!(registry.by_name("acc").unwrap().entity_id, 2);
}

/// Out-of-order epochs, taken names, gaps and changes to dropped entities are refused, and a
/// refused event changes nothing.
#[test]
fn invalid_events_are_refused() {
    let mut registry = SchemaRegistry::new();
    registry
        .apply(SchemaEpoch(1), &CatalogEvent::Schema(table("a", 1, 1)))
        .unwrap();
    let refused = [
        (SchemaEpoch(1), CatalogEvent::Epoch),
        (SchemaEpoch(2), CatalogEvent::Schema(table("A", 2, 2))),
        (SchemaEpoch(2), CatalogEvent::Schema(table("b", 3, 9))),
        (SchemaEpoch(2), CatalogEvent::DropEntity { entity_id: 7 }),
        (SchemaEpoch(2), CatalogEvent::Schema(table("b", 0, 2))),
    ];
    for (epoch, event) in refused {
        assert!(registry.apply(epoch, &event).is_err(), "{event:?}");
    }
    let mut gap = table("a", 1, 2);
    gap.version = SchemaVersion(3);
    gap.parent = Some(SchemaVersion(2));
    assert!(registry
        .apply(SchemaEpoch(2), &CatalogEvent::Schema(gap))
        .is_err());
    assert_eq!(registry.epoch(), SchemaEpoch(1));
}

/// Rows adapt to the latest version, or to the version a reader pinned; entities without a
/// schema are left alone; adapters are compiled once.
#[test]
fn rows_adapt_through_the_registry() {
    let mut registry = SchemaRegistry::new();
    let v1 = table("acc", 1, 1);
    registry
        .apply(SchemaEpoch(1), &CatalogEvent::Schema(v1.clone()))
        .unwrap();
    let v2 = apply(
        &v1,
        &SchemaChange::AddField {
            field: field("status", Some(Value::Int64(1))),
        },
        SchemaEpoch(2),
    )
    .unwrap();
    registry
        .apply(SchemaEpoch(2), &CatalogEvent::Schema(v2))
        .unwrap();
    let old = Row::new().with_field(1, Value::Int64(5));
    let latest = registry.adapt(1, old.clone(), None);
    assert_eq!(latest.get(3), Some(&Value::Int64(1)));
    assert_eq!(latest.schema_version, SchemaVersion(2));
    let pinned = registry.adapt(1, old.clone(), Some(SchemaEpoch(1)));
    assert_eq!(pinned.get(3), None);
    assert_eq!(
        registry.adapt(9, old.clone(), None),
        old,
        "no schema, no adaptation"
    );
    let a = registry.adapter(1, SchemaVersion(0), SchemaVersion(2));
    let b = registry.adapter(1, SchemaVersion(1), SchemaVersion(2));
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "unversioned and version 1 share one adapter"
    );
}

/// Persisted state is validated: a damaged history or misfiled entity is corruption, and a
/// valid state round-trips.
#[test]
fn persisted_state_is_validated() {
    let mut registry = SchemaRegistry::new();
    registry
        .apply(SchemaEpoch(1), &CatalogEvent::Schema(table("a", 1, 1)))
        .unwrap();
    let bytes = bincode::serialize(registry.state()).unwrap();
    let restored = SchemaRegistry::from_state(bincode::deserialize(&bytes).unwrap()).unwrap();
    assert_eq!(restored.state(), registry.state());

    let mut misfiled = registry.state().clone();
    let history = misfiled.entities.remove(&1).unwrap();
    misfiled.entities.insert(5, history);
    misfiled.next_entity_id = 6;
    assert!(matches!(
        SchemaRegistry::from_state(misfiled),
        Err(SchemaError::Corrupt(_))
    ));
    let mut future = registry.state().clone();
    future.epoch = SchemaEpoch(0);
    assert!(SchemaRegistry::from_state(future).is_err());
}
