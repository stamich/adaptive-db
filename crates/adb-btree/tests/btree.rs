//! Behavioural tests of the generic B+Tree on the primary-key instantiation.

use std::ops::Bound;

use adb_btree::BTree;
use adb_core::{Lsn, PageId, RowId, RowLocation};
use adb_journal::Journal;
use tempfile::tempdir;

/// Distinct, deterministic location for key `i`.
fn loc(i: u128) -> RowLocation {
    RowLocation {
        page_id: PageId(i as u64),
        slot_id: (i % 100) as u16,
    }
}

/// Opens a tree in `dir` (8 cached pages, forcing splits) holding keys `0..n`.
fn filled(dir: &std::path::Path, n: u128) -> BTree {
    let tree = BTree::open(dir.join("tree.dat"), dir.join("tree.meta"), 8).unwrap();
    for i in 0..n {
        tree.insert(RowId(i), loc(i)).unwrap();
    }
    tree
}

/// A thousand keys survive splits, a flush and a reopen.
#[test]
fn insert_split_reopen_and_lookup() {
    let dir = tempdir().unwrap();
    filled(dir.path(), 1000).flush().unwrap();
    let tree = BTree::open(dir.path().join("tree.dat"), dir.path().join("tree.meta"), 8).unwrap();
    for i in 0..1000u128 {
        assert_eq!(tree.get(RowId(i)).unwrap(), Some(loc(i)));
    }
}

/// No-steal: changes that never reached a checkpoint/flush do not exist after a "crash".
#[test]
fn unflushed_changes_vanish_but_flushed_state_is_intact() {
    let dir = tempdir().unwrap();
    {
        let tree = filled(dir.path(), 50);
        tree.flush().unwrap();
        for i in 50..5000u128 {
            tree.insert(RowId(i), loc(i)).unwrap(); // forces root splits, never flushed
        }
    } // crash
    let tree = BTree::open(dir.path().join("tree.dat"), dir.path().join("tree.meta"), 8).unwrap();
    assert_eq!(tree.scan_all().unwrap().len(), 50);
    assert_eq!(tree.get(RowId(49)).unwrap(), Some(loc(49)));
    assert_eq!(tree.get(RowId(50)).unwrap(), None);
}

/// A journal commit publishes split pages and the new root together.
#[test]
fn journaled_checkpoint_persists_pages_and_new_root() {
    let dir = tempdir().unwrap();
    {
        let tree = filled(dir.path(), 3000);
        Journal::new(dir.path())
            .commit(&tree.journal_writes().unwrap())
            .unwrap();
        tree.mark_clean();
        assert_eq!(tree.dirty_page_count(), 0);
    }
    let tree = BTree::open(dir.path().join("tree.dat"), dir.path().join("tree.meta"), 8).unwrap();
    assert_eq!(tree.scan_all().unwrap().len(), 3000);
}

/// Range scans honour inclusive/exclusive bounds and the limit.
#[test]
fn range_scan_respects_bounds_and_limit() {
    let dir = tempdir().unwrap();
    let tree = filled(dir.path(), 1000);
    let keys = |start, end, limit| {
        tree.scan(start, end, limit)
            .unwrap()
            .into_iter()
            .map(|(key, _)| key.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        keys(Bound::Included(RowId(10)), Bound::Excluded(RowId(14)), 100),
        vec![10, 11, 12, 13]
    );
    assert_eq!(
        keys(Bound::Excluded(RowId(10)), Bound::Included(RowId(12)), 100),
        vec![11, 12]
    );
    assert_eq!(
        keys(Bound::Included(RowId(500)), Bound::Unbounded, 3),
        vec![500, 501, 502]
    );
    assert_eq!(
        keys(Bound::Excluded(RowId(998)), Bound::Unbounded, 10),
        vec![999]
    );
}

/// Removed keys disappear, empty leaves are skipped and keys can be reinserted.
#[test]
fn remove_deletes_entries_and_scans_skip_empty_leaves() {
    let dir = tempdir().unwrap();
    let tree = filled(dir.path(), 1000);
    for i in 100..900u128 {
        assert_eq!(tree.remove(RowId(i), Lsn(1)).unwrap(), Some(loc(i)));
    }
    assert_eq!(tree.remove(RowId(500), Lsn(1)).unwrap(), None);
    assert_eq!(tree.get(RowId(500)).unwrap(), None);
    let all = tree.scan_all().unwrap();
    assert_eq!(all.len(), 200);
    assert_eq!(
        tree.scan(Bound::Included(RowId(99)), Bound::Unbounded, 2)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k.0)
            .collect::<Vec<_>>(),
        vec![99, 900]
    );
    // Removed keys can be inserted again.
    tree.insert(RowId(500), loc(7)).unwrap();
    assert_eq!(tree.get(RowId(500)).unwrap(), Some(loc(7)));
}

/// Floor lookups walk left past empty leaves and report `None` when nothing is smaller.
#[test]
fn floor_finds_predecessor_across_empty_leaves() {
    let dir = tempdir().unwrap();
    let tree = filled(dir.path(), 1000);
    for i in 101..900u128 {
        tree.remove(RowId(i), Lsn(1)).unwrap();
    }
    assert_eq!(tree.get_floor(RowId(850)).unwrap().unwrap().0, RowId(100));
    assert_eq!(tree.get_floor(RowId(950)).unwrap().unwrap().0, RowId(950));
    for i in 0..=100u128 {
        tree.remove(RowId(i), Lsn(1)).unwrap();
    }
    assert!(tree.get_floor(RowId(850)).unwrap().is_none());
}
