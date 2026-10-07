//! Size counters reported by `Database::storage_stats`.

/// Row, version and page counts of both projections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageStats {
    /// Entries in the current index (live rows plus tombstones awaiting vacuum).
    pub current_rows: u64,
    /// Tombstones awaiting vacuum.
    pub current_tombstones: u64,
    /// Historical versions.
    pub historical_versions: u64,
    /// Pages of the current heap.
    pub current_heap_pages: u64,
    /// Pages of the current index.
    pub current_index_pages: u64,
    /// Pages of the version heap.
    pub version_heap_pages: u64,
    /// Pages of the version index.
    pub version_index_pages: u64,
}
