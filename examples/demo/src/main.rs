//! Adaptive DB Milestone 1.6.2 demonstration.
//!
//! 1.6.2 adds a Rust-only demo/benchmark layer on top of Milestone 1.6.1
//! Hardened. The production engine scope remains Milestone 1.6: persistent
//! Current Store, persistent temporal Version Store, segmented WAL and
//! checkpoint v2.

use adb_btree::VersionBTree;
use adb_core::{CommitTs, Lsn, Row, RowId, RowLocation, TxId, Value, VersionKey};
use adb_engine::{Database, DbError};
use adb_storage::{HistoricalVersion, PersistentVersionStore};
use adb_wal::{lsn_offset, lsn_segment, SegmentedWalReader, SegmentedWalWriter, WalRecord};
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
    println!("=== Adaptive DB Milestone 1.6.2 demo ===");
    println!("Engine baseline: Milestone 1.6.1 Hardened\n");

    let dir = tempdir()?;
    let db_path = dir.path().join("database");

    println!("[1] Persistent temporal database: create three versions");
    let db = Database::open(&db_path)?;
    let mut tx = db.begin();
    tx.put(RowId(1), row(100));
    let ts1 = db.commit(tx)?;
    let mut tx = db.begin();
    tx.put(RowId(1), row(200));
    let ts2 = db.commit(tx)?;
    let mut tx = db.begin();
    tx.put(RowId(1), row(300));
    let ts3 = db.commit(tx)?;

    assert_eq!(value(&db.get_at(RowId(1), ts1)?.expect("v1")), 100);
    assert_eq!(value(&db.get_at(RowId(1), ts2)?.expect("v2")), 200);
    assert_eq!(value(&db.get_at(RowId(1), ts3)?.expect("v3")), 300);
    println!("    get_at() => 100 / 200 / 300");

    println!("[2] history() exposes persistent closed versions + current version");
    let history = db.history(RowId(1))?;
    assert_eq!(history.len(), 3);
    for version in &history {
        println!(
            "    [{}, {}) -> {:?}",
            version.begin_ts.0,
            version.end_ts.0,
            version.value.as_ref().map(value)
        );
    }

    println!("[3] Temporal delete preserves the earlier value");
    let mut tx = db.begin();
    tx.delete(RowId(1));
    let deleted_ts = db.commit(tx)?;
    assert!(db.get(RowId(1))?.is_none());
    assert_eq!(
        value(&db.get_at(RowId(1), ts3)?.expect("pre-delete value")),
        300
    );
    assert!(db.get_at(RowId(1), deleted_ts)?.is_none());

    println!("[4] Read-your-own-writes and write/write conflict remain intact");
    let mut local = db.begin();
    local.put(RowId(2), row(222));
    assert_eq!(value(&db.get_in_tx(&local, RowId(2))?.expect("local")), 222);
    db.rollback(local)?;

    let mut a = db.begin();
    let mut b = db.begin();
    a.put(RowId(3), row(1));
    b.put(RowId(3), row(2));
    db.commit(a)?;
    match db.commit(b) {
        Err(DbError::TransactionConflict) => println!("    conflict detected as expected"),
        other => return Err(format!("expected TransactionConflict, got {other:?}").into()),
    }

    println!("[5] Drop/reopen: temporal history survives without RAM reconstruction");
    drop(db);
    let reopened = Database::open(&db_path)?;
    assert_eq!(
        value(&reopened.get_at(RowId(1), ts1)?.expect("v1 after restart")),
        100
    );
    assert_eq!(
        value(&reopened.get_at(RowId(1), ts2)?.expect("v2 after restart")),
        200
    );
    assert_eq!(
        value(&reopened.get_at(RowId(1), ts3)?.expect("v3 after restart")),
        300
    );
    assert!(reopened.get(RowId(1))?.is_none());
    assert_eq!(reopened.history(RowId(1))?.len(), 4);
    println!("    all historical versions survived restart");

    println!("[6] Storage statistics and integrity verification");
    let stats = reopened.storage_stats()?;
    println!(
        "    current_rows={} historical_versions={}",
        stats.current_rows, stats.historical_versions
    );
    println!(
        "    current(heap={},index={}) version(heap={},index={})",
        stats.current_heap_pages,
        stats.current_index_pages,
        stats.version_heap_pages,
        stats.version_index_pages
    );
    let report = reopened.verify()?;
    if !report.is_ok() {
        return Err(format!("integrity verification failed: {:?}", report.errors).into());
    }
    println!("    verify() = OK");

    println!("[7] Persistent VersionStore directly: (RowId, begin_ts) -> RowLocation");
    let pv_dir = dir.path().join("persistent-version-demo");
    let versions = PersistentVersionStore::open(&pv_dir)?;
    versions.put_at_lsn(
        RowId(9),
        &HistoricalVersion {
            begin_ts: CommitTs(10),
            end_ts: CommitTs(20),
            value: Some(row(900)),
        },
        Lsn(100),
    )?;
    versions.put_at_lsn(
        RowId(9),
        &HistoricalVersion {
            begin_ts: CommitTs(20),
            end_ts: CommitTs(30),
            value: Some(row(901)),
        },
        Lsn(200),
    )?;
    versions.flush()?;
    assert_eq!(
        value(
            &versions
                .get_at(RowId(9), CommitTs(15))?
                .expect("history")
                .value
                .expect("row")
        ),
        900
    );
    assert_eq!(versions.history(RowId(9))?.len(), 2);
    drop(versions);
    let versions = PersistentVersionStore::open(&pv_dir)?;
    assert_eq!(versions.history(RowId(9))?.len(), 2);
    println!("    persistent temporal versions survived reopen");

    println!("[8] Temporal VersionBTree floor/range lookup");
    let vt_dir = dir.path().join("version-btree-demo");
    std::fs::create_dir_all(&vt_dir)?;
    let tree = VersionBTree::open(vt_dir.join("tree.idx"), vt_dir.join("tree.meta"), 8)?;
    let loc1 = RowLocation {
        page_id: adb_core::PageId(10),
        slot_id: 1,
    };
    let loc2 = RowLocation {
        page_id: adb_core::PageId(11),
        slot_id: 2,
    };
    tree.insert_at_lsn(VersionKey::new(RowId(42), CommitTs(100)), loc1, Lsn(10))?;
    tree.insert_at_lsn(VersionKey::new(RowId(42), CommitTs(200)), loc2, Lsn(20))?;
    tree.flush()?;
    assert_eq!(
        tree.get_floor(VersionKey::new(RowId(42), CommitTs(150)))?,
        Some((VersionKey::new(RowId(42), CommitTs(100)), loc1))
    );
    assert_eq!(tree.range_for_row(RowId(42))?.len(), 2);
    println!("    get_floor(150) -> begin_ts 100; range size=2");

    println!("[9] Segmented WAL: force rotation and decode packed LSNs");
    let wal_dir = dir.path().join("segmented-wal-demo");
    let mut wal = SegmentedWalWriter::open(&wal_dir, 220)?;
    let mut lsns = Vec::new();
    for i in 0..20u64 {
        let lsn = wal.append(&WalRecord::Begin {
            tx_id: TxId(i + 1),
            snapshot_ts: CommitTs(i),
        })?;
        lsns.push(lsn);
    }
    wal.sync()?;
    let records = SegmentedWalReader::read_all(&wal_dir)?;
    assert_eq!(records.len(), 20);
    let segments = std::fs::read_dir(&wal_dir)?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("wal"))
        .count();
    assert!(segments > 1, "small segment size should force rotation");
    let last = *lsns.last().expect("at least one LSN");
    println!(
        "    segments={} last_lsn={} => segment={} offset={}",
        segments,
        last.0,
        lsn_segment(last),
        lsn_offset(last)
    );

    println!("[10] Basic WAL retention is safe after persistent-store checkpoint");
    let removed = reopened.prune_wal_before_checkpoint()?;
    println!("    prune_wal_before_checkpoint() removed {removed} complete old segment(s)");

    println!("\nDemo completed successfully.");
    Ok(())
}
