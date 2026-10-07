//! Transaction semantics: own writes, conflicts, isolation levels.
mod common;
use adb_core::RowId;
use adb_engine::{Database, DbError};
use common::{read_i64, row_with_i64};
use tempfile::tempdir;

/// A committed put is visible to later reads.
#[test]
fn put_and_get() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut tx = db.begin();
    tx.put(RowId(1), row_with_i64(100));
    db.commit(tx).unwrap();
    assert_eq!(read_i64(&db.get(RowId(1)).unwrap().unwrap()), 100);
}

/// A transaction sees its own buffered writes; others do not.
#[test]
fn transaction_reads_its_own_writes() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut tx = db.begin();
    tx.put(RowId(1), row_with_i64(123));
    assert_eq!(
        read_i64(&db.get_in_tx(&mut tx, RowId(1)).unwrap().unwrap()),
        123
    );
    assert!(db.get(RowId(1)).unwrap().is_none());
    db.rollback(tx).unwrap();
}

/// The second of two concurrent writers of one row aborts (first committer wins).
#[test]
fn write_write_conflict_is_detected() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut seed = db.begin();
    seed.put(RowId(1), row_with_i64(100));
    db.commit(seed).unwrap();
    let mut a = db.begin();
    let mut b = db.begin();
    a.put(RowId(1), row_with_i64(150));
    b.put(RowId(1), row_with_i64(200));
    db.commit(a).unwrap();
    assert!(matches!(db.commit(b), Err(DbError::TransactionConflict(_))));
}

/// Classic write skew: two transactions each read both rows and update a different one.
/// Snapshot isolation lets both commit; serializable must abort the second.
fn write_skew(isolation: adb_engine::IsolationLevel) -> Result<(), DbError> {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    common::put(&db, RowId(1), 1);
    common::put(&db, RowId(2), 1);

    let mut a = db.begin_with(isolation);
    let mut b = db.begin_with(isolation);
    for tx in [&mut a, &mut b] {
        let both_present = db.get_in_tx(tx, RowId(1)).unwrap().is_some()
            && db.get_in_tx(tx, RowId(2)).unwrap().is_some();
        assert!(both_present);
    }
    a.put(RowId(1), row_with_i64(0));
    b.put(RowId(2), row_with_i64(0));
    db.commit(a).unwrap();
    db.commit(b).map(|_| ())
}

/// Snapshot isolation lets both write-skew transactions commit.
#[test]
fn snapshot_isolation_permits_write_skew() {
    assert!(write_skew(adb_engine::IsolationLevel::Snapshot).is_ok());
}

/// Serializable isolation aborts the second write-skew transaction with a read-write conflict.
#[test]
fn serializable_isolation_prevents_write_skew() {
    assert!(matches!(
        write_skew(adb_engine::IsolationLevel::Serializable),
        Err(DbError::TransactionConflict(
            adb_engine::Conflict::ReadWrite(_)
        ))
    ));
}

/// Reading a missing key and then seeing someone insert it is a read-write conflict
/// (prevents duplicate "check then insert" under serializable).
#[test]
fn serializable_detects_concurrent_insert_of_a_key_read_as_absent() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut checker = db.begin();
    assert!(db.get_in_tx(&mut checker, RowId(9)).unwrap().is_none());
    checker.put(RowId(10), row_with_i64(1));
    common::put(&db, RowId(9), 1);
    assert!(matches!(
        db.commit(checker),
        Err(DbError::TransactionConflict(_))
    ));
}

/// Read-only transactions always commit and report their snapshot.
#[test]
fn read_only_transactions_commit_at_their_snapshot() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let ts = common::put(&db, RowId(1), 1);
    let mut reader = db.begin();
    db.get_in_tx(&mut reader, RowId(1)).unwrap();
    common::put(&db, RowId(1), 2);
    assert_eq!(db.commit(reader).unwrap(), ts);
}

/// Uncommitted writes are invisible to other transactions and to plain reads.
#[test]
fn reads_never_see_uncommitted_or_other_transactions_writes() {
    let dir = tempdir().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let mut writer = db.begin();
    writer.put(RowId(1), row_with_i64(5));
    let mut other = db.begin();
    assert!(db.get_in_tx(&mut other, RowId(1)).unwrap().is_none());
    assert!(db.get(RowId(1)).unwrap().is_none());
    db.commit(writer).unwrap();
    assert!(
        db.get_in_tx(&mut other, RowId(1)).unwrap().is_none(),
        "snapshot is stable"
    );
    assert_eq!(common::value(&db, RowId(1)), Some(5));
}
