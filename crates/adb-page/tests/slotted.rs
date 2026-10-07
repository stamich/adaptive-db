//! Slotted heap pages: insertion, deletion, reuse and compaction.
use adb_core::PageId;
use adb_page::{Page, PageKind, SlottedPage};

/// Variable-length tuples are stored and read back intact.
#[test]
fn slotted_page_inserts_and_reads_variable_payloads() {
    let mut page = Page::new(PageId(0), PageKind::Heap);
    let mut slotted = SlottedPage::new(&mut page);
    let a = slotted.insert(b"abc").unwrap();
    let b = slotted.insert(b"a much longer row").unwrap();
    assert_eq!(slotted.get(a).unwrap(), b"abc");
    assert_eq!(slotted.get(b).unwrap(), b"a much longer row");
}

/// Deleted slots are reused and their bytes reclaimed, so churn never exhausts a page.
#[test]
fn delete_then_insert_reuses_space_indefinitely() {
    let mut page = Page::new(PageId(0), PageKind::Heap);
    let mut slotted = SlottedPage::new(&mut page);
    let keep = slotted.insert(b"stable tuple").unwrap();
    let mut churn = slotted.insert(&[1u8; 1000]).unwrap();
    for round in 0..10_000u32 {
        slotted.delete(churn).unwrap();
        churn = slotted.insert(&[(round % 251) as u8; 1000]).unwrap();
    }
    assert_eq!(slotted.get(keep).unwrap(), b"stable tuple");
    assert_eq!(slotted.live_count(), 2);
}

/// Freed slots keep the ids of the remaining live tuples stable.
#[test]
fn delete_keeps_other_slot_ids_stable() {
    let mut page = Page::new(PageId(0), PageKind::Heap);
    let mut slotted = SlottedPage::new(&mut page);
    let a = slotted.insert(b"a").unwrap();
    let b = slotted.insert(b"bb").unwrap();
    let c = slotted.insert(b"ccc").unwrap();
    slotted.delete(b).unwrap();
    assert!(slotted.get(b).is_err());
    assert_eq!(slotted.get(a).unwrap(), b"a");
    assert_eq!(slotted.get(c).unwrap(), b"ccc");
    assert_eq!(
        slotted.insert(b"dd").unwrap(),
        b,
        "free slot is reused first"
    );
}

/// A page full of deleted tuples accepts a tuple that only fits after compaction.
#[test]
fn compaction_makes_fragmented_space_usable() {
    let mut page = Page::new(PageId(0), PageKind::Heap);
    let mut slotted = SlottedPage::new(&mut page);
    let mut ids = Vec::new();
    while let Ok(id) = slotted.insert(&[7u8; 500]) {
        ids.push(id);
    }
    for id in ids.iter().step_by(2) {
        slotted.delete(*id).unwrap();
    }
    let big = slotted.available_for_insert().unwrap();
    assert!(big >= 500 * (ids.len() / 2));
    let id = slotted.insert(&vec![9u8; big]).unwrap();
    assert_eq!(slotted.get(id).unwrap().len(), big);
    assert!(slotted.insert(b"x").is_err());
}

/// Pages with freed slots survive a persistence round trip and validation.
#[test]
fn page_with_free_slots_round_trips_through_validation() {
    let mut page = Page::new(PageId(3), PageKind::Heap);
    {
        let mut slotted = SlottedPage::new(&mut page);
        slotted.insert(b"one").unwrap();
        let two = slotted.insert(b"two").unwrap();
        slotted.insert(b"three").unwrap();
        slotted.delete(two).unwrap();
    }
    page.seal_checksum();
    let mut decoded = Page::from_bytes(PageId(3), *page.bytes()).unwrap();
    let slotted = SlottedPage::new(&mut decoded);
    assert_eq!(slotted.live_count(), 2);
}
