//! Module `version_btree` for crate `adb-btree`.
use adb_btree::VersionBTree;
use adb_core::{CommitTs, Lsn, PageId, RowId, RowLocation, VersionKey};
use tempfile::tempdir;

/// Implements the `version_btree_supports_composite_keys_and_range` operation used by this subsystem.
#[test]
fn version_btree_supports_composite_keys_and_range() {
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

    {
        let tree = VersionBTree::open(&data, &meta, 16).unwrap();

        let floor = tree
            .get_floor(VersionKey::new(RowId(12), CommitTs(25)))
            .unwrap()
            .unwrap();

        assert_eq!(floor.0.begin_ts, CommitTs(20));

        let row_versions = tree.range_for_row(RowId(12)).unwrap();
        assert_eq!(row_versions.len(), 4);
    }
}
