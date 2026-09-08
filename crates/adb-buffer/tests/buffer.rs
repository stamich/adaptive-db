//! Buffer module for the adb-buffer crate.
//!
use std::sync::Arc;

use adb_buffer::{BufferPool, FilePageStore};
use adb_page::PageKind;
use tempfile::tempdir;

/// Implements the `dirty_pages_survive_flush_and_reopen` operation used by this subsystem.
#[test]
fn dirty_pages_survive_flush_and_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.dat");
    {
        let store = Arc::new(FilePageStore::open(&path).unwrap());
        let pool = BufferPool::new(store, 2);
        let id = pool.allocate_page(PageKind::Heap).unwrap();
        pool.write(id, |page| page.payload_mut()[0] = 77).unwrap();
        pool.flush_all().unwrap();
    }
    {
        let store = Arc::new(FilePageStore::open(&path).unwrap());
        let pool = BufferPool::new(store, 2);
        assert_eq!(
            pool.read(adb_core::PageId(0), |p| p.payload()[0]).unwrap(),
            77
        );
    }
}
