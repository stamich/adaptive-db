//! The temporal-key instantiation of the generic B+Tree.

use std::ops::Bound;

use adb_btree::VersionBTree;
use adb_core::{CommitTs, Lsn, PageId, RowId, RowLocation, VersionKey};
use tempfile::tempdir;

#[test]
fn composite_keys_support_floor_and_per_row_ranges() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("versions.idx");
    let meta = dir.path().join("versions.meta");
    {
        let tree = VersionBTree::open(&data, &meta, 16).unwrap();
        for row in 1..=30u128 {
            for ts in [10u64, 20, 30, 40] {
                tree.insert_at_lsn(
                    VersionKey::new(RowId(row), CommitTs(ts)),
                    RowLocation {
                        page_id: PageId(row as u64),
                        slot_id: (ts / 10) as u16,
                    },
                    Lsn(ts),
                )
                .unwrap();
            }
        }
        tree.flush().unwrap();
    }

    let tree = VersionBTree::open(&data, &meta, 16).unwrap();
    let floor = tree
        .get_floor(VersionKey::new(RowId(12), CommitTs(25)))
        .unwrap()
        .unwrap();
    assert_eq!(floor.0, VersionKey::new(RowId(12), CommitTs(20)));

    let versions = tree
        .scan(
            Bound::Included(VersionKey::new(RowId(12), CommitTs(0))),
            Bound::Included(VersionKey::new(RowId(12), CommitTs(u64::MAX))),
            usize::MAX,
        )
        .unwrap();
    assert_eq!(versions.len(), 4);

    // A floor before the first version of a row lands on the previous row.
    let before = tree
        .get_floor(VersionKey::new(RowId(12), CommitTs(5)))
        .unwrap()
        .unwrap();
    assert_eq!(before.0.row_id, RowId(11));
}
