//! Execution wire hardening tests.
use adb_core::RowId;
use adb_execution::{encode_batch_v1, ColumnVector, RecordBatch};
/// Verifies batch encoding rejects columns whose cardinality differs from row ids.
#[test]
fn mismatched_column_length_is_rejected() {
    let batch = RecordBatch {
        row_ids: vec![RowId(1), RowId(2)],
        columns: vec![ColumnVector::Int64 {
            field_id: 1,
            values: vec![Some(7)],
        }],
    };
    assert!(encode_batch_v1(&batch).is_err());
}
