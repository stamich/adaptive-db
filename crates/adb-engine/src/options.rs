//! Tunables of one database instance.

/// Engine configuration. `Default` is suitable for most uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseOptions {
    /// Clean pages cached per page file (dirty pages are never evicted).
    pub buffer_pages: usize,
    /// A checkpoint starts after a commit once this many pages are dirty. Bounds both memory
    /// held by the no-steal buffer pools and the amount of log replayed after a crash.
    pub checkpoint_dirty_pages: usize,
    /// Size of one log segment file.
    pub log_segment_bytes: u64,
}

impl Default for DatabaseOptions {
    fn default() -> Self {
        Self {
            buffer_pages: 256,
            checkpoint_dirty_pages: 4096,
            log_segment_bytes: adb_wal::DEFAULT_SEGMENT_SIZE,
        }
    }
}
