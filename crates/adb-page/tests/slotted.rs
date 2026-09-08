//! Slotted module for the adb-page crate.
//!
use adb_core::PageId;
use adb_page::{Page, PageKind, SlottedPage};

/// Implements the `slotted_page_inserts_and_reads_variable_payloads` operation used by this subsystem.
#[test]
fn slotted_page_inserts_and_reads_variable_payloads() {
    let mut page = Page::new(PageId(0), PageKind::Heap);
    let mut slotted = SlottedPage::new(&mut page);
    let a = slotted.insert(b"abc").unwrap();
    let b = slotted.insert(b"a much longer row").unwrap();
    assert_eq!(slotted.get(a).unwrap(), b"abc");
    assert_eq!(slotted.get(b).unwrap(), b"a much longer row");
}
