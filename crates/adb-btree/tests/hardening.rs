//! Regression tests for temporal B+Tree corruption handling.

use adb_btree::version_codec::decode_version_node;
use adb_core::PageId;
use adb_page::{Page, PageKind, PAGE_HEADER_SIZE};

/// Verifies that an attacker- or corruption-controlled temporal leaf count
/// returns a typed error instead of panicking while slicing the page payload.
#[test]
fn huge_version_leaf_count_is_rejected() {
    let mut page = Page::new(PageId(1), PageKind::BTreeLeaf);
    page.bytes_mut()[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + 2]
        .copy_from_slice(&u16::MAX.to_le_bytes());

    assert!(decode_version_node(&page).is_err());
}
