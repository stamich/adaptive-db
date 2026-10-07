//! Concurrent commits: correctness of the group-commit pipeline.
mod common;

use std::{sync::Arc, thread};

use adb_core::RowId;
use adb_engine::{Database, DbError};
use tempfile::tempdir;

use common::{row_with_i64, value};

/// Disjoint writers from many threads: every acknowledged commit is visible and durable,
/// commit timestamps are unique, and visibility never runs ahead of durability.
#[test]
fn concurrent_disjoint_commits_are_all_durable() {
    const THREADS: u64 = 8;
    const PER_THREAD: u64 = 200;
    let dir = tempdir().unwrap();
    let mut timestamps = Vec::new();
    {
        let db = Arc::new(Database::open(dir.path()).unwrap());
        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let db = Arc::clone(&db);
                thread::spawn(move || {
                    (0..PER_THREAD)
                        .map(|i| {
                            let mut tx = db.begin();
                            tx.put(RowId::compose(t, i), row_with_i64((t * 1000 + i) as i64));
                            db.commit(tx).unwrap()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            timestamps.extend(handle.join().unwrap());
        }
    } // crash without checkpoint
    timestamps.sort();
    timestamps.dedup();
    assert_eq!(timestamps.len() as u64, THREADS * PER_THREAD);

    let db = Database::open(dir.path()).unwrap();
    for t in 0..THREADS {
        for i in 0..PER_THREAD {
            assert_eq!(
                value(&db, RowId::compose(t, i)),
                Some((t * 1000 + i) as i64)
            );
        }
    }
}

/// Contended counter: with retries on conflict, no increment is lost.
#[test]
fn contended_increments_with_retry_are_not_lost() {
    const THREADS: usize = 6;
    const INCREMENTS: usize = 50;
    let dir = tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path()).unwrap());
    common::put(&db, RowId(1), 0);
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let mut conflicts = 0;
                for _ in 0..INCREMENTS {
                    loop {
                        let mut tx = db.begin();
                        let current =
                            common::read_i64(&db.get_in_tx(&mut tx, RowId(1)).unwrap().unwrap());
                        tx.put(RowId(1), row_with_i64(current + 1));
                        match db.commit(tx) {
                            Ok(_) => break,
                            Err(DbError::TransactionConflict(_)) => conflicts += 1,
                            Err(other) => panic!("{other}"),
                        }
                    }
                }
                conflicts
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(value(&db, RowId(1)), Some((THREADS * INCREMENTS) as i64));
}
