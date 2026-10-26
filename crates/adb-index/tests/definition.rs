//! Index definitions and the entry encoding.
use adb_core::{CommitTs, Direction, IndexId, Row, RowId, Value};
use adb_index::{
    entry::{decode_entry, encode_included, encode_index_key, tree_key, tree_value},
    IndexColumn, IndexDefinition, IndexError, IndexSpec, IndexState, MAX_INDEX_KEY_BYTES, OPEN,
};

/// `CREATE INDEX name ON entity 3 (fields...) INCLUDE (include...)` with ascending columns.
fn spec(name: &str, fields: &[u32], include: &[u32]) -> IndexSpec {
    IndexSpec {
        name: name.into(),
        entity_id: 3,
        columns: fields
            .iter()
            .map(|field| IndexColumn {
                field_id: *field,
                direction: Direction::Asc,
            })
            .collect(),
        include: include.to_vec(),
    }
}

/// Names, column counts and duplicates are validated.
#[test]
fn definitions_are_validated() {
    assert!(spec("idx_balance", &[2], &[3]).validate().is_ok());
    for bad in [
        spec("", &[2], &[]),
        spec("1abc", &[2], &[]),
        spec("bad-name", &[2], &[]),
        spec(&"x".repeat(129), &[2], &[]),
        spec("none", &[], &[]),
        spec("many", &(0..17).collect::<Vec<_>>(), &[]),
        spec("dup", &[2, 2], &[]),
        spec("dup_include", &[2], &[2]),
        spec("wide", &[2], &(10..43).collect::<Vec<_>>()),
    ] {
        assert!(
            matches!(bad.validate(), Err(IndexError::InvalidDefinition(_))),
            "{bad:?}"
        );
    }
}

/// Key and covered values come from the row; absent fields are NULL.
#[test]
fn rows_map_to_keys_and_covered_values() {
    let definition = IndexDefinition::new(
        IndexId(1),
        spec("i", &[2, 4], &[3]),
        IndexState::Active,
        CommitTs(5),
    );
    let row = Row::new()
        .with_field(2, Value::Int64(10))
        .with_field(3, Value::String("o".into()));
    assert_eq!(
        definition.key_values(&row),
        vec![Value::Int64(10), Value::Null]
    );
    assert_eq!(
        definition.included_values(&row),
        vec![Value::String("o".into())]
    );
    assert!(definition.covers(3) && definition.covers(4) && !definition.covers(1));
}

/// Entries round-trip; visibility is `begin <= ts < end`; oversized keys are refused.
#[test]
fn entries_round_trip_and_know_their_visibility() {
    let definition = IndexDefinition::new(
        IndexId(1),
        spec("i", &[2], &[3]),
        IndexState::Active,
        CommitTs(1),
    );
    let key = encode_index_key(&definition, &[Value::Int64(-4)]).unwrap();
    let included = encode_included(&definition, &[Value::String("x".into())]).unwrap();
    let row_id = RowId::from_primary_key(3, 9);
    let entry = decode_entry(
        &definition,
        &tree_key(&key, row_id, CommitTs(7)),
        &tree_value(CommitTs(9), &included),
    )
    .unwrap();
    assert_eq!(entry.key, vec![Value::Int64(-4)]);
    assert_eq!(entry.row_id, row_id);
    assert_eq!(entry.included, vec![Value::String("x".into())]);
    assert!(
        !entry.visible_at(CommitTs(6))
            && entry.visible_at(CommitTs(7))
            && entry.visible_at(CommitTs(8))
    );
    assert!(!entry.visible_at(CommitTs(9)));
    let open = decode_entry(
        &definition,
        &tree_key(&key, row_id, CommitTs(7)),
        &tree_value(OPEN, &included),
    )
    .unwrap();
    assert!(open.visible_at(CommitTs(u64::MAX - 1)));
    let huge = Value::String("y".repeat(MAX_INDEX_KEY_BYTES));
    assert!(matches!(
        encode_index_key(&definition, &[huge]),
        Err(IndexError::KeyTooLarge { .. })
    ));
    assert!(decode_entry(&definition, b"short", b"")
        .unwrap_err()
        .is_corruption());
}

/// Tree keys order by key value, then by primary key, then by begin timestamp.
#[test]
fn tree_keys_follow_index_order() {
    let definition = IndexDefinition::new(
        IndexId(1),
        spec("i", &[2], &[]),
        IndexState::Active,
        CommitTs(1),
    );
    let key = |v: i64| encode_index_key(&definition, &[Value::Int64(v)]).unwrap();
    let a = tree_key(&key(-1), RowId::from_primary_key(3, 50), CommitTs(9));
    let b = tree_key(&key(5), RowId::from_primary_key(3, -50), CommitTs(1));
    let c = tree_key(&key(5), RowId::from_primary_key(3, 2), CommitTs(1));
    let d = tree_key(&key(5), RowId::from_primary_key(3, 2), CommitTs(4));
    assert!(a < b && b < c && c < d);
}
