//! Buffer-pool persistence and no-steal policy tests.
use std::sync::Arc;

use adb_buffer::{BufferPool, FilePageStore, PageStore};
use adb_core::PageId;
use adb_journal::Journal;
use adb_page::PageKind;
use tempfile::tempdir;

/// Opens a pool over the page file at `path`.
fn pool(path: &std::path::Path, capacity: usize) -> BufferPool {
    BufferPool::new(Arc::new(FilePageStore::open(path).unwrap()), capacity).unwrap()
}

/// Flushed pages are readable after reopening the file.
#[test]
fn dirty_pages_survive_flush_and_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.dat");
    {
        let pool = pool(&path, 2);
        let id = pool.allocate_page(PageKind::Heap).unwrap();
        pool.write(id, |page| page.payload_mut()[0] = 77).unwrap();
        pool.flush_all().unwrap();
    }
    let pool = pool(&path, 2);
    assert_eq!(pool.read(PageId(0), |p| p.payload()[0]).unwrap(), 77);
}

/// Allocation does not touch the file; an un-checkpointed page simply never existed.
#[test]
fn allocated_pages_are_invisible_on_disk_until_checkpoint() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.dat");
    {
        let pool = pool(&path, 4);
        pool.allocate_page(PageKind::Heap).unwrap();
        assert_eq!(pool.page_count(), 1);
    }
    let store = FilePageStore::open(&path).unwrap();
    assert_eq!(store.page_count().unwrap(), 0);
}

/// No-steal: with capacity 1, dirty pages must stay cached instead of being written back.
#[test]
fn dirty_pages_are_never_evicted() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.dat");
    let pool = pool(&path, 1);
    for value in 0..8u8 {
        let id = pool.allocate_page(PageKind::Heap).unwrap();
        pool.write(id, |page| page.payload_mut()[0] = value)
            .unwrap();
    }
    assert_eq!(pool.dirty_count(), 8);
    assert_eq!(FilePageStore::open(&path).unwrap().page_count().unwrap(), 0);
    for value in 0..8u8 {
        assert_eq!(
            pool.read(PageId(value as u64), |p| p.payload()[0]).unwrap(),
            value
        );
    }
}

/// Clean pages are evicted and transparently reloaded.
#[test]
fn clean_pages_are_evicted_and_reloaded() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.dat");
    let pool = pool(&path, 2);
    for value in 0..6u8 {
        let id = pool.allocate_page(PageKind::Heap).unwrap();
        pool.write(id, |page| page.payload_mut()[0] = value)
            .unwrap();
    }
    pool.flush_all().unwrap();
    for value in (0..6u8).rev() {
        assert_eq!(
            pool.read(PageId(value as u64), |p| p.payload()[0]).unwrap(),
            value
        );
    }
}

/// Checkpoint path: dirty images go through the journal, then the pool is marked clean.
#[test]
fn journal_writes_publish_dirty_pages() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.dat");
    {
        let pool = pool(&path, 4);
        let id = pool.allocate_page(PageKind::Heap).unwrap();
        pool.write(id, |page| page.payload_mut()[5] = 9).unwrap();
        let writes = pool.journal_writes(&path).unwrap();
        assert_eq!(writes.len(), 1);
        Journal::new(dir.path()).commit(&writes).unwrap();
        pool.mark_clean();
        assert_eq!(pool.dirty_count(), 0);
    }
    let pool = pool(&path, 4);
    assert_eq!(pool.read(PageId(0), |p| p.payload()[5]).unwrap(), 9);
}

/// Pages that were never allocated cannot be read.
#[test]
fn reading_an_unallocated_page_is_an_error() {
    let dir = tempdir().unwrap();
    let pool = pool(&dir.path().join("pages.dat"), 2);
    assert!(pool.read(PageId(0), |_| ()).is_err());
}
