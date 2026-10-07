//! Execution wire hardening tests.

use adb_core::RowId;
use adb_execution::{
    encode_batch, ColumnVector, RecordBatch, SlotId, BATCH_FORMAT_VERSION, FLAG_ROW_IDS,
};

/// Batch encoding rejects columns whose cardinality differs from the row count.
#[test]
fn mismatched_column_length_is_rejected() {
    let batch = RecordBatch {
        num_rows: 2,
        row_ids: Some(vec![RowId(1), RowId(2)]),
        columns: vec![ColumnVector::Int64 {
            slot: SlotId(1),
            values: vec![Some(7)],
        }],
    };
    assert!(encode_batch(&batch).is_err());
}

/// Batch encoding rejects a row-id list whose length differs from the row count.
#[test]
fn mismatched_row_id_count_is_rejected() {
    let batch = RecordBatch {
        num_rows: 2,
        row_ids: Some(vec![RowId(1)]),
        columns: Vec::new(),
    };
    assert!(encode_batch(&batch).is_err());
}

/// Batch v2 sets the row-id flag only when row ids are present and omits them otherwise.
#[test]
fn row_ids_are_optional_in_batch_v2() {
    let column = ColumnVector::Int64 {
        slot: SlotId(3),
        values: vec![Some(1), None],
    };
    let with_ids = encode_batch(&RecordBatch {
        num_rows: 2,
        row_ids: Some(vec![RowId(1), RowId(2)]),
        columns: vec![column.clone()],
    })
    .unwrap();
    let without_ids = encode_batch(&RecordBatch {
        num_rows: 2,
        row_ids: None,
        columns: vec![column],
    })
    .unwrap();

    let version = u16::from_le_bytes([with_ids[4], with_ids[5]]);
    let flags = |bytes: &[u8]| u16::from_le_bytes([bytes[6], bytes[7]]);
    assert_eq!(version, BATCH_FORMAT_VERSION);
    assert_eq!(flags(&with_ids), FLAG_ROW_IDS);
    assert_eq!(flags(&without_ids), 0);
    assert_eq!(with_ids.len() - without_ids.len(), 2 * 16);
    // The first column header carries the output slot id.
    assert_eq!(&without_ids[16..20], &3u32.to_le_bytes());
}
