//! Catalog records (data format 3): written, read back and kept in order like row records,
//! and the encoding of the earlier record kinds is unchanged.
use adb_core::{CommitTs, Row, RowId, SchemaEpoch, TxId, ValueType};
use adb_schema::{create, EntitySpec, NewField};
use adb_wal::{read_all, CatalogEvent, SegmentedWalWriter, WalRecord};
use tempfile::tempdir;

/// Version 1 of `t (id BIGINT PRIMARY KEY)`.
fn schema() -> adb_schema::EntitySchema {
    create(
        &EntitySpec {
            name: "t".into(),
            fields: vec![NewField {
                name: "id".into(),
                value_type: ValueType::Int64,
                nullable: false,
                default: None,
                references: None,
                id: None,
            }],
            primary_key: "id".into(),
            entity_id: None,
        },
        4,
        SchemaEpoch(1),
    )
    .unwrap()
}

/// Every catalog event round-trips through the log between ordinary records.
#[test]
fn catalog_records_round_trip() {
    let dir = tempdir().unwrap();
    let tx_id = TxId(1);
    let records = vec![
        WalRecord::Begin {
            tx_id,
            snapshot_ts: CommitTs(0),
        },
        WalRecord::Catalog {
            tx_id,
            epoch: SchemaEpoch(1),
            event: CatalogEvent::Schema(schema()),
        },
        WalRecord::Catalog {
            tx_id,
            epoch: SchemaEpoch(2),
            event: CatalogEvent::Epoch,
        },
        WalRecord::Catalog {
            tx_id,
            epoch: SchemaEpoch(3),
            event: CatalogEvent::DropEntity { entity_id: 4 },
        },
        WalRecord::Commit {
            tx_id,
            commit_ts: CommitTs(1),
        },
    ];
    let mut writer = SegmentedWalWriter::open(dir.path(), 1 << 20).unwrap();
    for record in &records {
        writer.append(record).unwrap();
    }
    writer.sync().unwrap();
    let read: Vec<WalRecord> = read_all(dir.path())
        .unwrap()
        .into_iter()
        .map(|entry| entry.record)
        .collect();
    assert_eq!(read, records);
}

/// A row record of 2.3.0 decodes unchanged (the catalog variant was appended last, and an
/// unversioned row keeps its encoding).
#[test]
fn earlier_record_kinds_keep_their_encoding() {
    let put = WalRecord::Put {
        tx_id: TxId(3),
        row_id: RowId(9),
        value: Row::new().with_field(1, adb_core::Value::Int64(5)),
    };
    let bytes = bincode::serialize(&put).unwrap();
    // Variant index 2 (Begin, Version, Put, ...), then the transaction, key and field map.
    assert_eq!(&bytes[..4], &2u32.to_le_bytes());
    assert_eq!(bincode::deserialize::<WalRecord>(&bytes).unwrap(), put);
}
