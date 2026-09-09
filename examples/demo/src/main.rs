//! Adaptive DB Milestone 1.5.2 demonstration.
//!
//! The program deliberately exercises only functionality already present in
//! Milestone 1.5.1 Hardened. 1.5.2 adds examples and benchmarks, not engine
//! features.

use adb_btree::BTree;
use adb_core::{Lsn, PageId, Row, RowId, RowLocation, Value};
use adb_engine::{Database, DbError};
use adb_page::{Page, PageKind};
use adb_storage::CheckpointStore;
use tempfile::tempdir;

fn row(value: i64) -> Row {
    Row::new().with_field(1, Value::Int64(value))
}

fn value(row: &Row) -> i64 {
    match row.get(1) {
        Some(Value::Int64(v)) => *v,
        other => panic!("expected Int64 in demo row, got {other:?}"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Adaptive DB Milestone 1.5.2 demo ===");
    println!("Engine baseline: Milestone 1.5.1 Hardened\n");

    let dir = tempdir()?;
    let db_path = dir.path().join("database");

    println!("[1] Open persistent database");
    let db = Database::open(&db_path)?;

    println!("[2] Commit row 1 = 100");
    let mut tx1 = db.begin();
    tx1.put(RowId(1), row(100));
    let ts1 = db.commit(tx1)?;

    println!("[3] Update row 1 = 200");
    let mut tx2 = db.begin();
    tx2.put(RowId(1), row(200));
    let ts2 = db.commit(tx2)?;

    let current = db.get(RowId(1))?.expect("row 1 must exist");
    let historical = db
        .get_at(RowId(1), ts1)?
        .expect("historical row must exist");
    println!(
        "    current={} historical@{}={}",
        value(&current),
        ts1.0,
        value(&historical)
    );
    assert_eq!(value(&current), 200);
    assert_eq!(value(&historical), 100);

    println!("[4] Read-your-own-writes inside a transaction, then rollback");
    let mut local = db.begin();
    local.put(RowId(2), row(222));
    assert_eq!(
        value(&db.get_in_tx(&local, RowId(2))?.expect("local row")),
        222
    );
    assert!(db.get(RowId(2))?.is_none());
    db.rollback(local)?;

    println!("[5] Detect write/write conflict");
    let mut a = db.begin();
    let mut b = db.begin();
    a.put(RowId(1), row(300));
    b.put(RowId(1), row(400));
    db.commit(a)?;
    match db.commit(b) {
        Err(DbError::TransactionConflict) => println!("    conflict detected as expected"),
        other => return Err(format!("expected TransactionConflict, got {other:?}").into()),
    }

    println!("[6] Inspect checkpoint produced after durable current-store flush");
    let checkpoint = CheckpointStore::new(db_path.join("checkpoint.meta")).load()?;
    println!(
        "    checkpoint_lsn={} checkpoint_commit_ts={}",
        checkpoint.last_applied_commit_lsn, checkpoint.last_commit_ts
    );

    println!("[7] Drop and reopen database: persistent Current + WAL recovery");
    drop(db);
    let reopened = Database::open(&db_path)?;
    assert_eq!(value(&reopened.get(RowId(1))?.expect("recovered row")), 300);
    assert_eq!(
        value(
            &reopened
                .get_at(RowId(1), ts1)?
                .expect("recovered historical row")
        ),
        100
    );
    assert_eq!(
        value(
            &reopened
                .get_at(RowId(1), ts2)?
                .expect("recovered historical row")
        ),
        200
    );
    println!("    current and historical versions recovered successfully");

    println!("[8] Exercise persistent primary B+Tree directly");
    let tree_dir = dir.path().join("btree-demo");
    std::fs::create_dir_all(&tree_dir)?;
    let tree = BTree::open(tree_dir.join("tree.idx"), tree_dir.join("tree.meta"), 8)?;
    let location = RowLocation {
        page_id: PageId(7),
        slot_id: 3,
    };
    tree.insert_at_lsn(RowId(42), location, Lsn(1234))?;
    tree.flush()?;
    assert_eq!(tree.get(RowId(42))?, Some(location));
    drop(tree);
    let tree = BTree::open(tree_dir.join("tree.idx"), tree_dir.join("tree.meta"), 8)?;
    assert_eq!(tree.get(RowId(42))?, Some(location));
    println!("    RowId(42) -> {:?} survived B+Tree reopen", location);

    println!("[9] Verify hardened page checksum catches corruption");
    let mut page = Page::new(PageId(0), PageKind::Heap);
    page.set_page_lsn(Lsn(77));
    page.seal_for_write();
    let clean = *page.bytes();
    Page::from_bytes(PageId(0), clean)?;
    let mut corrupt = clean;
    corrupt[128] ^= 0x5a;
    assert!(Page::from_bytes(PageId(0), corrupt).is_err());
    println!("    corrupted page rejected");

    println!("\nDemo completed successfully.");
    Ok(())
}
