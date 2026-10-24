//! Position-preserving log rewrites.
use adb_core::{CommitTs, Row, RowId, TxId, Value};
use adb_wal::{read_all, rewrite_log, SegmentedWalWriter, WalError, WalRecord};
use tempfile::tempdir;

/// A put of `row_id` in transaction `tx`.
fn put(tx: u64, row_id: RowId) -> WalRecord {
    WalRecord::Put {
        tx_id: TxId(tx),
        row_id,
        value: Row::new().with_field(1, Value::Int64(tx as i64)),
    }
}

/// Every record is mapped and keeps its LSN, across several segments.
#[test]
fn rewrite_keeps_every_position() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let mut writer = SegmentedWalWriter::open(&source, 256).unwrap();
    for tx in 0..20 {
        writer.append(&put(tx, RowId::compose(1, tx))).unwrap();
        writer
            .append(&WalRecord::Commit {
                tx_id: TxId(tx),
                commit_ts: CommitTs(tx + 1),
            })
            .unwrap();
    }
    writer.sync().unwrap();
    let target = dir.path().join("target");
    let report = rewrite_log(&source, &target, |record| match record {
        WalRecord::Put {
            tx_id,
            row_id,
            value,
        } => WalRecord::Put {
            tx_id,
            row_id: RowId(row_id.0 ^ (1 << 63)),
            value,
        },
        other => other,
    })
    .unwrap();
    assert_eq!(report.records, 40);
    assert!(report.segments > 1, "the log spans several segments");
    let before = read_all(&source).unwrap();
    let after = read_all(&target).unwrap();
    assert_eq!(before.len(), after.len());
    for (old, new) in before.iter().zip(&after) {
        assert_eq!(old.lsn, new.lsn);
        assert_eq!(old.next, new.next);
        match (&old.record, &new.record) {
            (WalRecord::Put { row_id: a, .. }, WalRecord::Put { row_id: b, .. }) => {
                assert_eq!(a.0 ^ (1 << 63), b.0)
            }
            (a, b) => assert_eq!(a, b),
        }
    }
}

/// A mapping that changes a record's size is refused, and so is a non-empty target.
#[test]
fn rewrite_refuses_size_changes_and_used_targets() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let mut writer = SegmentedWalWriter::open(&source, 1 << 20).unwrap();
    writer.append(&put(1, RowId::compose(1, 1))).unwrap();
    writer.sync().unwrap();
    let grown = rewrite_log(&source, &dir.path().join("grown"), |_| WalRecord::Put {
        tx_id: TxId(1),
        row_id: RowId::compose(1, 1),
        value: Row::new().with_field(1, Value::String("longer than before".into())),
    });
    assert!(matches!(grown, Err(WalError::InvalidConfiguration(_))));
    let used = rewrite_log(&source, &source, |record| record);
    assert!(matches!(used, Err(WalError::InvalidConfiguration(_))));
}
