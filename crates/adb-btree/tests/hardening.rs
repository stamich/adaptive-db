//! Corruption handling of the node codec.

use adb_btree::codec::decode_node;
use adb_core::{PageId, RowId, VersionKey};
use adb_page::{Page, PageKind, PAGE_HEADER_SIZE};

/// A corrupted leaf count must yield an error, never a panic or out-of-bounds read.
#[test]
fn huge_leaf_count_is_rejected_for_every_key_type() {
    let mut page = Page::new(PageId(1), PageKind::BTreeLeaf);
    page.bytes_mut()[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + 2]
        .copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(decode_node::<VersionKey>(&page).is_err());
    assert!(decode_node::<RowId>(&page).is_err());
}

/// Heap pages are not B+Tree nodes.
#[test]
fn wrong_page_kind_is_rejected() {
    let page = Page::new(PageId(1), PageKind::Heap);
    assert!(decode_node::<RowId>(&page).is_err());
}
