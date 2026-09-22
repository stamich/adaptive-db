//! Primary B+Tree split/reopen regression tests.

use adb_btree::BTree;
use adb_core::{PageId, RowId, RowLocation};
use tempfile::tempdir;

/// Verifies insertion across multiple splits, flush, reopen, and point lookup.
#[test]
fn insert_split_reopen_and_lookup() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("tree.dat");
    let meta = dir.path().join("tree.meta");

    {
        let tree = BTree::open(&data, &meta, 8).unwrap();
        for i in 0..1000u128 {
            tree.insert(
                RowId(i),
                RowLocation {
                    page_id: PageId(i as u64),
                    slot_id: (i % 100) as u16,
                },
            )
                .unwrap();
        }
        tree.flush().unwrap();
    }

    {
        let tree = BTree::open(&data, &meta, 8).unwrap();
        for i in 0..1000u128 {
            let value = tree.get(RowId(i)).unwrap().unwrap();
            assert_eq!(value.page_id.0, i as u64);
        }
    }
}
