//! Adaptive DB Milestone 1.0.2 demo.
//!
//! This executable intentionally demonstrates only capabilities already present
//! in Milestone 1.0.1: MVCC transactions, transaction-local reads, snapshots,
//! durable WAL commits, write-write conflict detection, deletes, and recovery.

use std::{
    fs,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use adb_core::{FieldId, Row, RowId, Value};
use adb_engine::{Database, DbError};

const VALUE_FIELD: FieldId = 1;
const LABEL_FIELD: FieldId = 2;

fn main() {
    if let Err(error) = run() {
        eprintln!("demo failed: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = demo_path();
    let _ = fs::remove_dir_all(&path);

    println!("Adaptive DB — Milestone 1.0.2 Rust demo");
    println!("database path: {}", path.display());
    println!();

    let db = Database::open(&path)?;

    println!("1) Transaction-local read (read-your-own-write inside a transaction)");
    let mut tx = db.begin();
    tx.put(RowId(1), row(100, "first"));
    let local = db
        .get_in_tx(&tx, RowId(1))?
        .expect("transaction-local row must exist");
    println!("   before commit: local={}, global={:?}", value(&local), db.get(RowId(1)));
    let first_commit = db.commit(tx)?;
    println!("   committed at ts={}", first_commit.0);
    println!("   after commit: global={}", value(&db.get(RowId(1)).unwrap()));
    println!();

    println!("2) MVCC snapshot isolation and historical read");
    let old_snapshot = db.begin();
    let mut update = db.begin();
    update.put(RowId(1), row(200, "second"));
    let second_commit = db.commit(update)?;

    let visible_in_old_snapshot = db
        .get_in_tx(&old_snapshot, RowId(1))?
        .expect("old snapshot must see the first version");
    let current = db.get(RowId(1)).expect("current row must exist");
    let historical = db
        .get_at(RowId(1), first_commit)
        .expect("historical row must exist");

    println!("   old snapshot sees={}", value(&visible_in_old_snapshot));
    println!("   current value={}", value(&current));
    println!("   get_at(ts={})={}", first_commit.0, value(&historical));
    println!("   second commit ts={}", second_commit.0);
    db.rollback(old_snapshot)?;
    println!();

    println!("3) Write-write conflict detection");
    let mut tx_a = db.begin();
    let mut tx_b = db.begin();
    tx_a.put(RowId(1), row(300, "winner"));
    tx_b.put(RowId(1), row(400, "conflict"));
    db.commit(tx_a)?;
    match db.commit(tx_b) {
        Err(DbError::TransactionConflict) => println!("   conflict detected correctly"),
        other => return Err(format!("unexpected conflict result: {other:?}").into()),
    }
    println!();

    println!("4) Durable delete");
    let mut delete_tx = db.begin();
    delete_tx.delete(RowId(1));
    db.commit(delete_tx)?;
    println!("   after delete: {:?}", db.get(RowId(1)));
    println!();

    println!("5) WAL recovery after close/reopen");
    let mut seed = db.begin();
    seed.put(RowId(2), row(777, "survives-restart"));
    db.commit(seed)?;
    let wal = db.wal_path().to_path_buf();
    drop(db);

    let recovered = Database::open(&path)?;
    let recovered_row = recovered
        .get(RowId(2))
        .expect("committed row must be reconstructed from WAL");
    println!("   WAL: {}", wal.display());
    println!("   recovered row 2 value={}", value(&recovered_row));
    println!();

    println!("Demo completed successfully.");
    println!("Note: tables, SQL, schemas, indexes and persistent current storage are not Milestone 1.0.2 features.");

    drop(recovered);
    let _ = fs::remove_dir_all(path);
    Ok(())
}

fn row(number: i64, label: &str) -> Row {
    Row::new()
        .with_field(VALUE_FIELD, Value::Int64(number))
        .with_field(LABEL_FIELD, Value::String(label.to_owned()))
}

fn value(row: &Row) -> i64 {
    match row.get(VALUE_FIELD) {
        Some(Value::Int64(number)) => *number,
        other => panic!("demo invariant violated: expected Int64 in field {VALUE_FIELD}, got {other:?}"),
    }
}

fn demo_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("adaptive-db-demo-{}-{nonce}", process::id()))
}
